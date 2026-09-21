use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Subcommand, ValueEnum};
use lino_arguments::Parser;
use package_registry_manager::setup::{default_browser_profile, execute_plan, ExecuteOptions};
use package_registry_manager::{
    build_plans_for, inspect_repository, Inspection, Registry, SetupPlan,
};

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
    },
    /// Validate a package and open its authenticated registry setup page.
    Setup {
        #[arg(long, value_parser = parse_registry)]
        registry: Registry,

        /// Select a package when a repository has multiple packages for a registry.
        #[arg(long)]
        package: Option<String>,

        /// Perform validation and browser steps. Otherwise setup is a dry run.
        #[arg(long)]
        execute: bool,

        /// Confirm the final npm form submission without an interactive prompt.
        #[arg(long, requires = "execute")]
        yes: bool,

        /// Print the setup URL instead of starting a browser.
        #[arg(long, requires = "execute")]
        no_browser: bool,

        /// Chrome-family browser channel (chrome, chromium, edge, or brave).
        #[arg(long, default_value = "chrome")]
        browser_channel: String,

        /// Dedicated profile; never point this at a normal browser profile.
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
    let inspection = inspect_repository(&args.repository)?;
    match args.command {
        Commands::Inspect => output_inspection(&inspection, args.format),
        Commands::Plan { registry } => {
            let selected = registry.into_iter().collect::<BTreeSet<_>>();
            let plans = build_plans_for(&inspection, &selected);
            if plans.is_empty() {
                bail!("no matching package manifests were found");
            }
            output_plans(&plans, args.format)
        }
        Commands::Setup {
            registry,
            package,
            execute,
            yes,
            no_browser,
            browser_channel,
            browser_profile,
        } => {
            let plans = build_plans_for(&inspection, &BTreeSet::from([registry]));
            let plan = select_plan(&plans, package.as_deref())?;
            output_plans(std::slice::from_ref(plan), args.format)?;
            let profile =
                browser_profile.unwrap_or_else(|| default_browser_profile(&args.repository));
            execute_plan(
                plan,
                &ExecuteOptions {
                    repository: &args.repository,
                    browser_profile: &profile,
                    browser_channel: &browser_channel,
                    execute,
                    yes,
                    no_browser,
                    verbose: args.verbose,
                },
            )
            .await
        }
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
    match plans {
        [] => bail!("no matching package manifests were found"),
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
                let status = if package.publishable {
                    "publishable"
                } else {
                    "not publishable"
                };
                writeln!(
                    output,
                    "- {}: {} ({}, {})",
                    package.registry, package.name, package.manifest, status
                )?;
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
                writeln!(output, "{}: {}", plan.registry, plan.package.name)?;
                for (index, step) in plan.steps.iter().enumerate() {
                    writeln!(output, "  {}. {}", index + 1, step.title)?;
                    if let Some(command) = &step.command {
                        writeln!(
                            output,
                            "     $ {} {}",
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
