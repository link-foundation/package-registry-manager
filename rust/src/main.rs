use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use lino_arguments::Parser;
use package_registry_manager::plan::{build_plans_with, PlanOptions};
use package_registry_manager::prerequisites::{
    probe_environment, render_prerequisites, BrowserDisplay, BrowserSummary,
};
use package_registry_manager::registry_state::{Endpoints, RegistryClient};
use package_registry_manager::setup::{
    default_browser_profile, execute_plan, BrowserMode, ExecuteOptions,
};
use package_registry_manager::{inspect_repository, Inspection, PlanMode, Registry, SetupPlan};

#[derive(Parser, Debug)]
#[command(
    name = "package-registry-manager",
    about = "Inspect repositories and guide secure package-registry setup"
)]
struct Args {
    #[arg(long, global = true, default_value = ".")]
    repository: PathBuf,

    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,

    /// Print commands and their output. Disabled by default.
    #[arg(long, global = true)]
    verbose: bool,

    /// Do not look up packages on registries.
    #[arg(long, global = true)]
    offline: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Discover publishable package manifests without changing the repository.
    Inspect,
    /// Produce an ordered, machine-readable registry setup plan.
    Plan {
        /// Restrict plans to one or more registries. May be repeated.
        #[arg(long, value_parser = parse_registry)]
        registry: Vec<Registry>,

        /// Also watch the release workflow and confirm that the next version has provenance.
        #[arg(long)]
        verify_release: bool,
    },
    /// Bootstrap or attach trusted publishing for one package.
    Setup {
        #[arg(long, value_parser = parse_registry)]
        registry: Registry,

        /// Select a package when a repository has multiple packages for a registry.
        #[arg(long)]
        package: Option<String>,

        /// Print the flow without running it. This is the default.
        #[arg(long, conflicts_with = "execute")]
        dry_run: bool,

        /// Run the flow and open a visible browser. Otherwise setup is a dry run.
        #[arg(long)]
        execute: bool,

        /// Also watch the release workflow and confirm that the next version has provenance.
        #[arg(long)]
        verify_release: bool,

        /// Confirm publishing, secret changes, and form submission in advance.
        #[arg(long, requires = "execute")]
        yes: bool,

        /// Print the setup URL instead of starting a browser.
        #[arg(long, requires = "execute")]
        no_browser: bool,

        /// Chrome-family browser channel (chrome, chromium, edge, or brave).
        #[arg(long, default_value = "chrome")]
        browser_channel: String,

        /// Open sign-in and approval pages in your default browser, or in the
        /// automated profile. Forms are always filled in the automated profile.
        #[arg(long, value_enum, default_value_t = BrowserMode::Default)]
        browser: BrowserMode,

        /// Dedicated automation profile, used to fill forms (default: per-user
        /// state directory); never point this at a normal browser profile.
        #[arg(long)]
        browser_profile: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let endpoints = Endpoints::from_env();
    let discovered = inspect_repository(&args.repository)?;
    let inspection = if args.offline {
        discovered
    } else {
        RegistryClient::new(endpoints.clone(), args.verbose)
            .probe_registry_state(&discovered)
            .await
    };
    let selected = match &args.command {
        Commands::Inspect => return output_inspection(&inspection, args.format),
        Commands::Plan { registry, .. } => registry.iter().copied().collect::<BTreeSet<_>>(),
        Commands::Setup { registry, .. } => BTreeSet::from([*registry]),
    };
    let environment = probe_environment(
        args.offline,
        args.verbose,
        inspection.packages.iter().any(|package| {
            package.registry == Registry::Npm
                && package.publishable
                && (selected.is_empty() || selected.contains(&Registry::Npm))
        }),
        &endpoints,
    )
    .await;
    match args.command {
        Commands::Inspect => Ok(()),
        Commands::Plan {
            registry: _,
            verify_release,
        } => {
            let options = PlanOptions {
                verify_release,
                endpoints,
                environment: Some(environment),
                browser: BrowserDisplay {
                    mode: BrowserSummary::Default,
                    channel: "chrome".to_owned(),
                    profile: None,
                },
            };
            let plans = build_plans_with(&inspection, &selected, &options);
            if plans.is_empty() {
                bail!("no matching package manifests were found");
            }
            output_plans(&plans, args.format)
        }
        Commands::Setup {
            registry: _,
            package,
            dry_run: _,
            execute,
            verify_release,
            yes,
            no_browser,
            browser,
            browser_channel,
            browser_profile,
        } => {
            // Only a run that may start the automated browser needs the profile.
            let profile = match browser_profile {
                Some(profile) => std::path::absolute(profile)?,
                None if execute || browser == BrowserMode::Automated => default_browser_profile()?,
                None => PathBuf::new(),
            };
            let options = PlanOptions {
                verify_release,
                endpoints,
                environment: Some(environment),
                browser: browser_display(no_browser, browser, &browser_channel, &profile),
            };
            let plans = build_plans_with(&inspection, &selected, &options);
            let plan = select_plan(&plans, package.as_deref())?;
            output_plans(std::slice::from_ref(plan), args.format)?;
            execute_plan(
                plan,
                &ExecuteOptions {
                    repository: &args.repository,
                    browser,
                    browser_profile: &profile,
                    browser_channel: &browser_channel,
                    execute,
                    yes,
                    no_browser,
                    verbose: args.verbose,
                    endpoints: options.endpoints,
                    poll_interval: Duration::from_secs(5),
                    wait_timeout: Duration::from_secs(20 * 60),
                },
            )
            .await
        }
    }
}

fn browser_display(
    no_browser: bool,
    browser: BrowserMode,
    channel: &str,
    profile: &std::path::Path,
) -> BrowserDisplay {
    if no_browser {
        return BrowserDisplay {
            mode: BrowserSummary::None,
            ..BrowserDisplay::default()
        };
    }
    let automated = browser == BrowserMode::Automated;
    BrowserDisplay {
        mode: if automated {
            BrowserSummary::Automated
        } else {
            BrowserSummary::Default
        },
        channel: channel.to_owned(),
        profile: automated.then(|| profile.display().to_string()),
    }
}

fn parse_registry(value: &str) -> Result<Registry, String> {
    value
        .parse()
        .map_err(|error: anyhow::Error| error.to_string())
}

fn select_plan<'a>(plans: &'a [SetupPlan], package: Option<&str>) -> Result<&'a SetupPlan> {
    if let Some(package) = package {
        return plans
            .iter()
            .find(|plan| plan.package.name == package)
            .with_context(|| format!("package '{package}' was not found for this registry"));
    }
    if plans.is_empty() {
        bail!("no matching package manifests were found");
    }
    let publishable: Vec<&SetupPlan> = plans
        .iter()
        .filter(|plan| plan.package.publishable)
        .collect();
    let candidates: Vec<&SetupPlan> = if publishable.is_empty() {
        plans.iter().collect()
    } else {
        publishable
    };
    match candidates.as_slice() {
        [plan] => Ok(plan),
        _ => bail!("multiple packages use this registry; select one with --package <name>"),
    }
}

fn output_inspection(inspection: &Inspection, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            write_stdout(&format!("{}\n", serde_json::to_string_pretty(inspection)?))?;
        }
        OutputFormat::Text => {
            let mut output = format!("Repository: {}\n", inspection.repository.root);
            if inspection.packages.is_empty() {
                output.push_str("No supported package manifests found.\n");
            }
            for package in &inspection.packages {
                let mut details = vec![
                    package.manifest.as_str(),
                    if package.publishable {
                        "publishable"
                    } else {
                        "not publishable"
                    },
                ];
                match package.exists_on_registry {
                    Some(true) => details.push("published"),
                    Some(false) => details.push("not published yet"),
                    None => {}
                }
                if package.trusted_publishing == Some(true) {
                    details.push("trusted publishing");
                }
                writeln!(
                    output,
                    "- {}: {} ({})",
                    package.registry,
                    package.name,
                    details.join(", ")
                )?;
                for warning in &package.warnings {
                    writeln!(output, "  warning: {warning}")?;
                }
            }
            write_stdout(&output)?;
        }
    }
    Ok(())
}

fn output_plans(plans: &[SetupPlan], format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => {
            write_stdout(&format!("{}\n", serde_json::to_string_pretty(plans)?))?;
        }
        OutputFormat::Text => {
            let mut output = String::new();
            for plan in plans {
                let mode = plan
                    .mode
                    .map(|mode| format!(" ({mode})"))
                    .unwrap_or_default();
                writeln!(output, "{}: {}{mode}", plan.registry, plan.package.name)?;
                if plan.mode == Some(PlanMode::Complete) {
                    writeln!(output, "  trusted publishing is already in use")?;
                }
                if let Some(reason) = &plan.skipped_reason {
                    writeln!(output, "  skipped: {reason}")?;
                }
                for line in render_prerequisites(&plan.prerequisites) {
                    writeln!(output, "{line}")?;
                }
                for (index, step) in plan.steps.iter().enumerate() {
                    let when = step
                        .when
                        .as_ref()
                        .map(|when| format!(" [when {when}]"))
                        .unwrap_or_default();
                    writeln!(output, "  {}. {}{when}", index + 1, step.title)?;
                    if let Some(command) = &step.command {
                        let cwd = step
                            .cwd
                            .as_ref()
                            .filter(|cwd| cwd.as_str() != ".")
                            .map(|cwd| format!("(in {cwd}) "))
                            .unwrap_or_default();
                        writeln!(
                            output,
                            "     $ {cwd}{} {}",
                            command.program,
                            command.args.join(" ")
                        )?;
                    }
                    if let Some(url) = &step.url {
                        writeln!(output, "     {url}")?;
                    }
                }
            }
            write_stdout(&output)?;
        }
    }
    Ok(())
}

fn write_output(writer: &mut impl Write, output: &str) -> io::Result<()> {
    match writer
        .write_all(output.as_bytes())
        .and_then(|()| writer.flush())
    {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

fn write_stdout(output: &str) -> io::Result<()> {
    write_output(&mut io::stdout(), output)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn broken_pipe_is_a_clean_exit() {
        assert!(write_output(&mut BrokenPipeWriter, "output\n").is_ok());
    }
}
