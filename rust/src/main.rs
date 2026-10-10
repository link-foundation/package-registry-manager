use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use lino_arguments::Parser;
use package_registry_manager::browser_catalogue::launch_channels;
use package_registry_manager::browser_options::{
    parse_browser_options, BrowserArgs, BrowserOptions,
};
use package_registry_manager::plan::{build_plans_with, PlanOptions};
use package_registry_manager::prerequisites::{
    probe_environment, render_prerequisites, BrowserDisplay, BrowserSummary,
};
use package_registry_manager::registry_state::{Endpoints, RegistryClient};
use package_registry_manager::setup::{execute_plans_with, BrowserMode, ExecuteOptions};
use package_registry_manager::sign_in_import::import_sources;
use package_registry_manager::{
    inspect_repository_with, InspectOptions, Inspection, Package, PlanMode, Registry, SetupPlan,
};
mod organization_cli;

fn browser_channel_help() -> String {
    format!(
        "Installed browser channel: {}.",
        launch_channels().join(", ")
    )
}

fn browser_import_help() -> String {
    format!(
        "Copy data from an installed profile (<browser>[:<profile>]): {}. default takes the system default browser, auto the first browser signed in to the registry.",
        import_sources().join(", ")
    )
}

#[derive(Parser, Debug)]
#[command(
    name = "package-registry-manager",
    version,
    about = "Inspect repositories and guide secure package-registry setup"
)]
struct Args {
    #[arg(long, global = true, default_value = ".")]
    repository: PathBuf,

    /// Scan all repositories of this organization through gh-manager.
    #[arg(long, global = true, conflicts_with_all = ["user", "repository", "bootstrap_ref"])]
    org: Option<String>,

    /// Scan all repositories of this user through gh-manager.
    #[arg(long, global = true, conflicts_with_all = ["org", "repository", "bootstrap_ref"])]
    user: Option<String>,

    /// Registry secret name; supports {REGISTRY}, {REPO}, {ORG}, and {OWNER}.
    #[arg(long, global = true)]
    secret_name: Option<String>,

    /// Bootstrap a pushed branch or pull-request head before merge.
    #[arg(long = "ref", global = true)]
    bootstrap_ref: Option<String>,

    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,

    /// Print commands and their output, and list the manifests inspection
    /// skipped. Disabled by default.
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

        /// Plan only the packages with this name.
        #[arg(long)]
        package: Option<String>,

        /// crates.io: create, use, and revoke the first-publish token by hand
        /// instead of through the crates.io API.
        #[arg(long)]
        manual: bool,

        #[command(flatten)]
        publisher: PublisherArgs,
    },
    /// Bootstrap or attach trusted publishing for one package or the whole repository.
    Setup {
        #[arg(long, value_parser = parse_registry, required_unless_present = "all")]
        registry: Option<Registry>,

        /// Set up every publishable package in one browser session.
        #[arg(long, conflicts_with = "package")]
        all: bool,

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

        #[command(flatten)]
        publisher: PublisherArgs,

        /// Confirm publishing, secret changes, and form submission in advance.
        #[arg(long, requires = "execute")]
        yes: bool,

        /// Print the setup URL instead of starting a browser.
        #[arg(long, requires = "execute")]
        no_browser: bool,

        /// Open sign-in and approval pages in this application instead of the
        /// default browser, such as "Google Chrome" (macOS) or firefox.
        #[arg(long, value_name = "APP", conflicts_with = "no_browser")]
        open_with: Option<String>,

        /// crates.io: create, use, and revoke the first-publish token by hand
        /// instead of through the crates.io API.
        #[arg(long)]
        manual: bool,

        /// Stay signed in after setup: npm keeps its session token in npm's
        /// user configuration until npm logout; crates.io keeps only the
        /// automated profile's browser session.
        #[arg(long)]
        keep_session: bool,

        #[arg(long, default_value = "chrome", help = browser_channel_help())]
        browser_channel: String,

        /// Installed browser executable to launch instead of the channel's.
        #[arg(long)]
        browser_executable: Option<PathBuf>,

        /// Open sign-in and approval pages in your default browser, or in the
        /// automated profile. Forms are always filled in the automated profile.
        #[arg(long, value_enum, default_value_t = BrowserMode::Default)]
        browser: BrowserMode,

        /// Dedicated automation profile, used to fill forms (default: per-user
        /// state directory); never point this at a normal browser profile.
        #[arg(long)]
        browser_profile: Option<PathBuf>,

        #[arg(long, value_name = "BROWSER[:PROFILE]|default|auto", help = browser_import_help())]
        browser_import: Option<String>,

        /// Import the whole profile, or only the registry's sign-in cookies
        /// (default: full for a named browser, domains otherwise).
        #[arg(long, value_name = "full|domains")]
        browser_import_scope: Option<String>,

        /// Fill forms in your own browser instead: snapshot[:<profile>]
        /// launches a temporary copy of your profile, extension drives your
        /// running browser through the Browser Commander extension.
        #[arg(long, value_name = "MODE")]
        browser_attach: Option<String>,

        #[arg(
            long,
            value_name = "KEY=VALUE",
            help = "Browser preference for the automated profile, such as intl.accept_languages=en; may be repeated."
        )]
        browser_pref: Vec<String>,

        /// Launch restriction or preset, such as no-extensions; may be repeated.
        #[arg(long, value_name = "NAME")]
        browser_restriction: Vec<String>,
    },
}

/// Overrides for the detected trusted publisher.
#[derive(Debug, Clone, ClapArgs)]
struct PublisherArgs {
    /// Trusted-publisher workflow file in .github/workflows, when detection
    /// finds several or the wrong one.
    #[arg(long, value_name = "FILE")]
    workflow: Option<String>,

    /// Propose missing publishing jobs in a draft PR; --workflow chooses its target.
    #[arg(long)]
    add_publish_job: bool,

    /// GitHub environment of the trusted publisher.
    #[arg(long, value_name = "NAME")]
    environment: Option<String>,
}

impl PublisherArgs {
    fn workflow(&self, repository: &std::path::Path) -> Result<Option<String>> {
        let Some(workflow) = &self.workflow else {
            return Ok(None);
        };
        let name = regex::Regex::new(r"^[A-Za-z0-9_.-]+\.ya?ml$").expect("valid expression");
        if !name.is_match(workflow) {
            bail!(
                "--workflow must be a workflow file name in .github/workflows, such as release.yml"
            );
        }
        if !self.add_publish_job
            && !repository
                .join(".github/workflows")
                .join(workflow)
                .is_file()
        {
            bail!("--workflow: .github/workflows/{workflow} does not exist");
        }
        Ok(Some(workflow.clone()))
    }

    fn environment(&self) -> Result<Option<String>> {
        match self.environment.as_deref().map(str::trim) {
            Some("") => bail!("--environment must not be empty"),
            environment => Ok(environment.map(str::to_owned)),
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.org.is_some() || args.user.is_some() {
        return organization_cli::run(&args).await;
    }
    let endpoints = Endpoints::from_env();
    let inspect_options = InspectOptions {
        include_skipped: args.verbose,
    };
    let bootstrap_ref = args
        .bootstrap_ref
        .as_deref()
        .map(package_registry_manager::bootstrap_reference::bootstrap_ref)
        .transpose()?;
    let discovered = if let Some(reference) = &bootstrap_ref {
        package_registry_manager::bootstrap_reference::inspect_reference(
            &args.repository,
            reference,
            inspect_options,
        )?
    } else {
        inspect_repository_with(&args.repository, inspect_options)?
    };
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
        Commands::Setup { registry, .. } => registry.iter().copied().collect::<BTreeSet<_>>(),
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
    let python = if inspection.packages.iter().any(|package| {
        package.registry == Registry::PyPi
            && package.publishable
            && (selected.is_empty() || selected.contains(&Registry::PyPi))
    }) {
        Some(package_registry_manager::python::probe_python(args.verbose).await)
    } else {
        None
    };
    match args.command {
        Commands::Inspect => Ok(()),
        Commands::Plan {
            registry: _,
            verify_release,
            package,
            manual,
            publisher,
        } => {
            let options = PlanOptions {
                bootstrap_ref: bootstrap_ref.clone(),
                python,
                add_publish_job: publisher.add_publish_job,
                verify_release,
                workflow: publisher.workflow(&args.repository)?,
                publisher_environment: publisher.environment()?,
                endpoints,
                environment: Some(environment),
                browser: BrowserDisplay {
                    mode: BrowserSummary::Default,
                    channel: "chrome".to_owned(),
                    ..BrowserDisplay::default()
                },
                manual,
            };
            let plans = filter_plans(
                build_plans_with(&inspection, &selected, &options),
                package.as_deref(),
            )?;
            output_plans(&plans, args.format)
        }
        Commands::Setup {
            registry: _,
            all,
            package,
            dry_run: _,
            execute,
            verify_release,
            publisher,
            yes,
            no_browser,
            open_with,
            manual,
            keep_session,
            browser,
            browser_channel,
            browser_executable,
            browser_profile,
            browser_import,
            browser_import_scope,
            browser_attach,
            browser_pref,
            browser_restriction,
        } => {
            let open_with = open_with.map(|app| app.trim().to_owned());
            if open_with.as_deref().is_some_and(str::is_empty) {
                bail!("--open-with must not be empty");
            }
            let browser_options = parse_browser_options(&BrowserArgs {
                channel: browser_channel,
                executable: browser_executable,
                import: browser_import,
                import_scope: browser_import_scope,
                attach: browser_attach,
                preferences: browser_pref,
                restrictions: browser_restriction,
                profile_given: browser_profile.is_some(),
            })?;
            // Only a run that may start the automated browser needs the profile.
            let profile = match browser_profile {
                Some(profile) => std::path::absolute(profile)?,
                None if execute || browser == BrowserMode::Automated => {
                    package_registry_manager::profile::default_browser_profile_for_channel(
                        &browser_options.channel,
                    )?
                }
                None => PathBuf::new(),
            };
            let options = PlanOptions {
                bootstrap_ref: bootstrap_ref.clone(),
                python,
                add_publish_job: publisher.add_publish_job,
                verify_release,
                workflow: publisher.workflow(&args.repository)?,
                publisher_environment: publisher.environment()?,
                endpoints,
                environment: Some(environment),
                browser: browser_display(no_browser, browser, &browser_options, &profile),
                manual,
            };
            let plans = build_plans_with(&inspection, &selected, &options);
            let selected_plans = if all {
                plans
                    .into_iter()
                    .filter(|plan| plan.package.publishable)
                    .collect::<Vec<_>>()
            } else {
                vec![select_plan(&plans, package.as_deref())?.clone()]
            };
            output_plans(&selected_plans, args.format)?;
            execute_plans_with(
                &selected_plans,
                &ExecuteOptions {
                    repository: &args.repository,
                    browser,
                    browser_profile: &profile,
                    browser_options: &browser_options,
                    execute,
                    yes,
                    no_browser,
                    open_with: open_with.as_deref(),
                    keep_session,
                    verbose: args.verbose,
                    secret_name: args.secret_name.as_deref(),
                    repository_batch: false,
                    secret_repositories: None,
                    quiet_browser: selected_plans
                        .iter()
                        .any(|plan| plan.credential_policy.mode == "token"),
                    endpoints: options.endpoints.clone(),
                    poll_interval: Duration::from_secs(5),
                    wait_timeout: Duration::from_secs(20 * 60),
                },
                Some(&inspection),
                options.workflow.as_deref(),
                options.publisher_environment.as_deref(),
                all,
            )
            .await
        }
    }
}

fn browser_display(
    no_browser: bool,
    browser: BrowserMode,
    options: &BrowserOptions,
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
        channel: options.channel.clone(),
        profile: (automated && options.attach.is_none()).then(|| profile.display().to_string()),
        import: options.import.clone(),
        attach: options.attach.clone(),
    }
}

fn parse_registry(value: &str) -> Result<Registry, String> {
    value
        .parse()
        .map_err(|error: anyhow::Error| error.to_string())
}

/// Keep the plans of `package`, or every plan without one.
fn filter_plans(plans: Vec<SetupPlan>, package: Option<&str>) -> Result<Vec<SetupPlan>> {
    if plans.is_empty() {
        bail!("no matching package manifests were found");
    }
    let Some(package) = package else {
        return Ok(plans);
    };
    let named: Vec<SetupPlan> = plans
        .into_iter()
        .filter(|plan| plan.package.name == package)
        .collect();
    if named.is_empty() {
        bail!("package '{package}' was not found");
    }
    Ok(named)
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
                if let (Some(workflow), true) = (&package.workflow, package.publishable) {
                    writeln!(
                        output,
                        "  workflow: {}",
                        publisher_summary(
                            workflow,
                            &package.workflow_jobs,
                            package.environment.as_deref()
                        )
                    )?;
                }
                for warning in &package.warnings {
                    writeln!(output, "  warning: {warning}")?;
                }
            }
            for item in &inspection.skipped {
                writeln!(output, "skipped {}: {}", item.manifest, item.reason)?;
            }
            write_stdout(&output)?;
        }
    }
    Ok(())
}

/// Describe a trusted-publisher workflow with its jobs and environment.
fn publisher_summary(workflow: &str, jobs: &[String], environment: Option<&str>) -> String {
    let mut details = Vec::new();
    if !jobs.is_empty() {
        let label = if jobs.len() > 1 { "jobs" } else { "job" };
        details.push(format!("{label} {}", jobs.join(", ")));
    }
    if let Some(environment) = environment {
        details.push(format!("environment {environment}"));
    }
    if details.is_empty() {
        workflow.to_owned()
    } else {
        format!("{workflow} ({})", details.join("; "))
    }
}

/// The detected jobs, unless `--workflow` chose a different workflow file.
fn trusted_jobs<'a>(package: &'a Package, workflow: &str) -> &'a [String] {
    if package.workflow.as_deref() == Some(workflow) {
        &package.workflow_jobs
    } else {
        &[]
    }
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
                writeln!(
                    output,
                    "  credentials: {}",
                    plan.credential_policy.description
                )?;
                if plan.mode == Some(PlanMode::Complete) {
                    writeln!(output, "  trusted publishing is already in use")?;
                }
                if let Some(reason) = &plan.skipped_reason {
                    writeln!(output, "  skipped: {reason}")?;
                }
                if let Some(publisher) = &plan.oidc_publisher {
                    let jobs = trusted_jobs(&plan.package, &publisher.workflow);
                    writeln!(
                        output,
                        "  trusted publisher: {}",
                        publisher_summary(
                            &publisher.workflow,
                            jobs,
                            publisher.environment.as_deref()
                        )
                    )?;
                }
                for warning in &plan.package.warnings {
                    writeln!(output, "  warning: {warning}")?;
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
