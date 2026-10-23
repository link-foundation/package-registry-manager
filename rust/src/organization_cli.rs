//! Account CLI orchestration; GitHub discovery is delegated to gh-manager.
use super::{
    output_plans, parse_browser_options, write_stdout, Args, BrowserArgs, Commands, OutputFormat,
};
use anyhow::{bail, Context, Result};
use package_registry_manager::github::GhManager;
use package_registry_manager::organization_scan::{
    scan_repositories_with_inactive, ScanReport, ScannedRepository,
};
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
    rotation: Option<(&str, &BTreeSet<String>)>,
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
            rotation
                .map(|(secret, _)| secret)
                .or(template)
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
    if let Some((secret, consumers)) = rotation {
        groups.insert(
            format!("crates-io:{secret}"),
            consumers.iter().cloned().collect(),
        );
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
    if matches!(args.command, Commands::Inspect) && matches!(args.format, OutputFormat::Text) {
        write_stdout("Repository\tRegistry\tPackage\tFinding\n")?;
    }
    let endpoints = Endpoints::from_env();
    let client = RegistryClient::new(endpoints.clone(), args.verbose);
    let mut scan = scan_repositories_with_inactive(
        &GhManager,
        args.org.as_deref(),
        args.user.as_deref(),
        args.offline,
        &client,
        args.verbose,
        args.rotate_token.is_some() || args.migrate,
        |item, findings, index, total| {
            eprintln!(
                "[{index}/{total}] {}{}",
                item.repository,
                item.error
                    .as_ref()
                    .map_or_else(String::new, |error| format!(": {error}"))
            );
            if matches!(args.command, Commands::Inspect)
                && matches!(args.format, OutputFormat::Text)
            {
                let report = ScanReport {
                    schema_version: 1,
                    github_owner: String::new(),
                    repositories: vec![item.clone()],
                    findings: findings.to_vec(),
                };
                let rendered = report.render();
                write_stdout(rendered.split_once('\n').map_or("", |(_, rows)| rows))?;
            }
            Ok(())
        },
    )
    .await?;
    if let Some(days) = args.token_expiry_days {
        let tokens =
            package_registry_manager::setup::read_account_token_inventory(scan.workspace.path())
                .await?;
        let findings = package_registry_manager::crates_tokens::token_expiry_findings(
            &tokens,
            &scan.report.github_owner,
            i64::from(days),
            chrono::Utc::now(),
        )?;
        if matches!(args.command, Commands::Inspect) && matches!(args.format, OutputFormat::Text) {
            let report = ScanReport {
                schema_version: 1,
                github_owner: scan.report.github_owner.clone(),
                repositories: Vec::new(),
                findings: findings.clone(),
            };
            let rendered = report.render();
            write_stdout(rendered.split_once('\n').map_or("", |(_, rows)| rows))?;
        }
        scan.report.findings.extend(findings);
    }
    if matches!(args.command, Commands::Inspect) {
        scan.report.public_paths();
        match args.format {
            OutputFormat::Json => write_stdout(&format!(
                "{}\n",
                serde_json::to_string_pretty(&scan.report)?
            ))?,
            OutputFormat::Text => {
                let errors: Vec<_> = scan
                    .report
                    .repositories
                    .iter()
                    .filter(|repo| repo.error.is_some())
                    .collect();
                write_stdout(&format!(
                    "Scanned {} repositories; {} with errors.\n",
                    scan.report.repositories.len(),
                    errors.len()
                ))?;
                for repo in errors {
                    write_stdout(&format!(
                        "{}: {}\n",
                        repo.repository,
                        repo.error.as_deref().unwrap_or_default()
                    ))?;
                }
            }
        }
        if scan
            .report
            .repositories
            .iter()
            .any(|repo| repo.error.is_some())
        {
            bail!("some repositories could not be scanned; see scan errors");
        }
        return Ok(());
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
            if (!all && args.rotate_token.is_none() && !args.migrate) || package.is_some() {
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
    let mut rotation_consumers = BTreeSet::new();
    for repo in &mut scan.report.repositories {
        let Some(inspection) = &repo.inspection else {
            continue;
        };
        let mut inspection = inspection.clone();
        let rotation_consumer = if let Some(secret) = &args.rotate_token {
            let reference = regex::Regex::new(&format!(r"\bsecrets\.{secret}\b"))?;
            package_registry_manager::workflows::read_workflows(std::path::Path::new(
                &inspection.repository.root,
            ))?
            .iter()
            .any(|workflow| reference.is_match(&workflow.contents))
        } else {
            false
        };
        if rotation_consumer {
            rotation_consumers.insert(repo.repository.clone());
        }
        if matches!(args.command, Commands::Setup { .. }) {
            inspection.packages.retain(|package| {
                if args.migrate {
                    return matches!(
                        package.registry,
                        Registry::CratesIo | Registry::Npm | Registry::PyPi
                    );
                }
                if args.rotate_token.is_some() {
                    return package.registry == Registry::CratesIo && rotation_consumer;
                }
                scan.report.findings.iter().any(|finding| {
                    finding["repository"] == repo.repository
                        && (finding["package"].is_null() || finding["package"] == package.name)
                })
            });
        }
        let workflow = match publisher.workflow(std::path::Path::new(&inspection.repository.root)) {
            Ok(workflow) => workflow,
            Err(error) => {
                repo.error = Some(repo.error.as_ref().map_or_else(
                    || error.to_string(),
                    |previous| format!("{previous}; {error}"),
                ));
                repo.errors
                    .push(serde_json::json!({"stage":"plan","message":error.to_string()}));
                continue;
            }
        };
        let options = PlanOptions {
            migrate: args.migrate,
            rotate_token: args.rotate_token.clone(),
            python: python.clone(),
            add_publish_job: publisher.add_publish_job,
            verify_release,
            workflow,
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
    if (args.rotate_token.is_some() || args.migrate)
        && scan
            .report
            .repositories
            .iter()
            .any(|repo| repo.error.is_some())
    {
        bail!("token rotation requires a complete account scan");
    }
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
    if matches!(args.format, OutputFormat::Json) {
        let mut report = scan.report.clone();
        report.public_paths();
        let mut document = serde_json::to_value(report)?;
        document["plans"] = serde_json::to_value(&display)?;
        write_stdout(&format!("{}\n", serde_json::to_string_pretty(&document)?))?;
    } else {
        output_plans(&display, args.format)?;
    }
    if let Commands::Setup {
        execute,
        yes,
        no_browser,
        open_with,
        sign_out,
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
            if (args.rotate_token.is_some() || args.migrate)
                && scan
                    .report
                    .repositories
                    .iter()
                    .any(|repo| repo.error.is_some())
            {
                bail!("token rotation requires a complete account scan");
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
                keep_session: !*sign_out,
                verbose: args.verbose,
                secret_name: args.secret_name.as_deref(),
                quiet_browser: true,
                repository_batch: true,
                secret_repositories: Some(&secret_repositories(
                    &plans,
                    args.secret_name.as_deref(),
                    args.rotate_token
                        .as_deref()
                        .map(|secret| (secret, &rotation_consumers)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_groups_include_unplanned_readers_and_use_the_explicit_secret() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/token-rotation");
        let mut inspection = package_registry_manager::inspect_repository(&root).unwrap();
        inspection.repository.github_owner = Some("acme".into());
        inspection.repository.github_repository = Some("start".into());
        let plans = build_plans_with(
            &inspection,
            &BTreeSet::from([Registry::CratesIo]),
            &PlanOptions {
                rotate_token: Some("PRIVATE_CARGO".into()),
                ..PlanOptions::default()
            },
        );
        let readers = BTreeSet::from(["acme/start".into(), "acme/consumer".into()]);
        let groups = secret_repositories(&plans, None, Some(("PRIVATE_CARGO", &readers))).unwrap();
        assert_eq!(
            groups["crates-io:PRIVATE_CARGO"],
            vec!["acme/consumer", "acme/start"]
        );
        assert!(!groups.contains_key("crates-io:CARGO_TOKEN"));
    }
}
