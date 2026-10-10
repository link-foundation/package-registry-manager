//! Account CLI orchestration; GitHub discovery is delegated to gh-manager.
use super::{
    output_plans, parse_browser_options, write_stdout, Args, BrowserArgs, Commands, OutputFormat,
};
use anyhow::{bail, Context, Result};
use package_registry_manager::github::GhManager;
use package_registry_manager::organization_scan::{scan_account, ScannedRepository};
use package_registry_manager::{
    plan::{build_plans_with, PlanOptions},
    prerequisites::probe_environment,
    registry_state::{Endpoints, RegistryClient},
    setup::{execute_plans_with, BrowserMode, ExecuteOptions},
    Registry,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

fn secret_repositories(
    plans: &[package_registry_manager::model::SetupPlan],
    template: Option<&str>,
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for plan in plans {
        let Some(provider) =
            package_registry_manager::credential_browser::token_provider(plan.registry)
        else {
            continue;
        };
        let contents = std::fs::read_to_string(
            std::path::Path::new(&plan.repository.root).join(".package-registry-manager.json"),
        )
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok("{}".into())
            } else {
                Err(error)
            }
        })?;
        let settings: serde_json::Value = serde_json::from_str(&contents)?;
        if settings["tokens"][plan.registry.to_string()]["level"] == "repo" {
            continue;
        }
        let slug = format!(
            "{}/{}",
            plan.repository.github_owner.as_deref().unwrap_or_default(),
            plan.repository
                .github_repository
                .as_deref()
                .unwrap_or_default()
        );
        let name = package_registry_manager::ci_credential_cycle::secret_name(
            template
                .or_else(|| settings["tokens"][plan.registry.to_string()]["secret"].as_str())
                .or_else(|| plan.package.token_secrets.first().map(String::as_str))
                .unwrap_or(provider.secret),
            plan.registry,
            &slug,
        )?;
        let targets = groups
            .entry(format!("{}:{name}", plan.registry))
            .or_default();
        if !targets.contains(&slug) {
            targets.push(slug);
        }
    }
    Ok(groups)
}

async fn prepare(repo: &ScannedRepository) -> Result<()> {
    let inspection = repo.inspection.as_ref().expect("selected inspection");
    let root = std::path::Path::new(&inspection.repository.root);
    std::fs::remove_dir_all(root.join(".git"))?;
    let branch = repo
        .default_branch
        .as_deref()
        .context("repository has no default branch")?;
    let url = format!("https://github.com/{}.git", repo.repository);
    for args in [
        vec!["init", "--quiet"],
        vec!["remote", "add", "origin", &url],
        vec!["fetch", "--quiet", "--depth=1", "origin", branch],
        vec!["checkout", "--quiet", "--force", "--detach", "FETCH_HEAD"],
    ] {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .await?;
        if !output.status.success() {
            bail!("cannot prepare {} for setup", repo.repository);
        }
    }
    Ok(())
}

#[allow(clippy::future_not_send)] // Browser Commander's CDP transport is intentionally !Sync.
pub async fn run(args: &Args) -> Result<()> {
    let endpoints = Endpoints::from_env();
    let client = RegistryClient::new(endpoints.clone(), args.verbose);
    let mut scan = scan_account(
        &GhManager,
        args.org.as_deref(),
        args.user.as_deref(),
        args.offline,
        &client,
        args.verbose,
    )
    .await?;
    if matches!(args.command, Commands::Inspect) {
        scan.report.public_paths();
        return match args.format {
            OutputFormat::Json => write_stdout(&format!(
                "{}\n",
                serde_json::to_string_pretty(&scan.report)?
            ))
            .map_err(Into::into),
            OutputFormat::Text => write_stdout(&scan.report.render()).map_err(Into::into),
        };
    }
    let (registry, verify_release, package, manual, publisher) = match &args.command {
        Commands::Plan {
            registry,
            verify_release,
            package,
            manual,
            publisher,
        } => (
            registry.iter().copied().collect::<BTreeSet<_>>(),
            *verify_release,
            package.as_deref(),
            *manual,
            publisher,
        ),
        Commands::Setup {
            registry,
            all,
            package,
            verify_release,
            manual,
            publisher,
            ..
        } => {
            if !all || package.is_some() {
                bail!("account setup requires --all without --package");
            }
            (
                registry.iter().copied().collect(),
                *verify_release,
                None,
                *manual,
                publisher,
            )
        }
        Commands::Inspect => unreachable!(),
    };
    let environment = probe_environment(
        args.offline,
        args.verbose,
        scan.report
            .repositories
            .iter()
            .filter_map(|repo| repo.inspection.as_ref())
            .flat_map(|inspection| &inspection.packages)
            .any(|package| package.registry == Registry::Npm),
        &endpoints,
    )
    .await;
    let python = if scan
        .report
        .repositories
        .iter()
        .filter_map(|repo| repo.inspection.as_ref())
        .flat_map(|inspection| &inspection.packages)
        .any(|package| package.registry == Registry::PyPi)
    {
        Some(package_registry_manager::python::probe_python(args.verbose).await)
    } else {
        None
    };
    let mut plans = Vec::new();
    for repo in &scan.report.repositories {
        let Some(inspection) = &repo.inspection else {
            continue;
        };
        let mut inspection = inspection.clone();
        if matches!(args.command, Commands::Setup { .. }) {
            inspection.packages.retain(|package| {
                scan.report.findings.iter().any(|finding| {
                    finding["repository"] == repo.repository
                        && (finding["package"].is_null() || finding["package"] == package.name)
                })
            });
        }
        let options = PlanOptions {
            python: python.clone(),
            add_publish_job: publisher.add_publish_job,
            verify_release,
            workflow: publisher.workflow(std::path::Path::new(&inspection.repository.root))?,
            publisher_environment: publisher.environment()?,
            endpoints: endpoints.clone(),
            environment: Some(environment.clone()),
            manual,
            ..PlanOptions::default()
        };
        plans.extend(
            build_plans_with(&inspection, &registry, &options)
                .into_iter()
                .filter(|plan| {
                    plan.package.publishable && package.is_none_or(|name| plan.package.name == name)
                }),
        );
    }
    plans.sort_by_key(|plan| plan.registry != Registry::Npm);
    let mut display = plans.clone();
    for plan in &mut display {
        plan.repository.root = format!(
            "https://github.com/{}/{}",
            plan.repository.github_owner.as_deref().unwrap_or_default(),
            plan.repository
                .github_repository
                .as_deref()
                .unwrap_or_default()
        );
    }
    output_plans(&display, args.format)?;
    if let Commands::Setup {
        execute,
        yes,
        no_browser,
        open_with,
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
        ..
    } = &args.command
    {
        if plans.is_empty() {
            if scan
                .report
                .repositories
                .iter()
                .any(|repo| repo.error.is_some())
            {
                eprintln!("{}", scan.report.render());
                bail!("some repositories could not be scanned");
            }
            if !matches!(args.format, OutputFormat::Json) {
                println!("No package findings require setup.");
            }
            return Ok(());
        }
        let browser_options = parse_browser_options(&BrowserArgs {
            channel: browser_channel.clone(),
            executable: browser_executable.clone(),
            import: browser_import
                .clone()
                .or_else(|| browser_attach.is_none().then(|| "auto".into())),
            import_scope: browser_import_scope.clone(),
            attach: browser_attach.clone(),
            preferences: browser_pref.clone(),
            restrictions: browser_restriction.clone(),
            profile_given: browser_profile.is_some(),
        })?;
        let profile = match browser_profile {
            Some(profile) => std::path::absolute(profile)?,
            None => package_registry_manager::profile::default_browser_profile_for_channel(
                &browser_options.channel,
            )?,
        };
        if *execute {
            for repo in &mut scan.report.repositories {
                if repo.inspection.as_ref().is_some_and(|inspection| {
                    plans
                        .iter()
                        .any(|plan| plan.repository.root == inspection.repository.root)
                }) {
                    if let Err(error) = prepare(repo).await {
                        eprintln!("{}: {error:#}", repo.repository);
                        repo.error = Some(error.to_string());
                        let root = &repo
                            .inspection
                            .as_ref()
                            .expect("selected inspection")
                            .repository
                            .root;
                        plans.retain(|plan| &plan.repository.root != root);
                    }
                }
            }
            if plans.is_empty() {
                bail!("some repositories could not be prepared; see scan errors");
            }
        }
        execute_plans_with(
            &plans,
            &ExecuteOptions {
                repository: scan.workspace.path(),
                browser: if open_with.is_some() {
                    *browser
                } else {
                    BrowserMode::Automated
                },
                browser_profile: &profile,
                browser_options: &browser_options,
                execute: *execute,
                yes: *yes,
                no_browser: *no_browser,
                open_with: open_with.as_deref(),
                keep_session: *keep_session,
                verbose: args.verbose,
                secret_name: args.secret_name.as_deref(),
                quiet_browser: true,
                account_scan: true,
                secret_repositories: Some(&secret_repositories(
                    &plans,
                    args.secret_name.as_deref(),
                )?),
                endpoints,
                poll_interval: Duration::from_secs(5),
                wait_timeout: Duration::from_secs(20 * 60),
            },
            None,
            publisher.workflow.as_deref(),
            publisher.environment.as_deref(),
            true,
        )
        .await?;
    }
    if scan
        .report
        .repositories
        .iter()
        .any(|repo| repo.error.is_some())
    {
        eprintln!("{}", scan.report.render());
        bail!("some repositories could not be scanned");
    }
    Ok(())
}
