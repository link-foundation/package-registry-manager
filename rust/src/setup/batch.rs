//! Single-package and repository-wide execution with shared browser/sign-in state.

use anyhow::{bail, Result};

use super::{ExecuteOptions, Session};
use crate::approvals::oidc_release_note;
use crate::model::{Inspection, PlanMode, SetupPlan};
use crate::profile::protect_legacy_profile;
use crate::workflow_proposal::offer_workflow;

fn validate_plan(plan: &SetupPlan, execute: bool) -> Result<()> {
    if !plan.package.publishable {
        bail!(
            "{} is not publishable: {}",
            plan.package.name,
            plan.package.problems.join("; ")
        );
    }
    if let Some(reason) = &plan.skipped_reason {
        if !plan
            .steps
            .iter()
            .any(|step| step.id == "add-publishing-workflow")
            && execute
        {
            bail!("{reason}");
        }
    }
    if execute {
        if let Some(item) = plan
            .prerequisites
            .iter()
            .find(|item| item.id == "python" && item.ok == Some(false))
        {
            bail!("{}; {}", item.detected, item.required);
        }
    }
    Ok(())
}

/// Runs one package, using the repository-wide executor and its cleanup guarantees.
#[allow(clippy::future_not_send)]
pub async fn execute_plan(plan: &SetupPlan, options: &ExecuteOptions<'_>) -> Result<()> {
    execute_plans_with(std::slice::from_ref(plan), options, None, None, None, false).await
}

/// Runs all packages in one browser session, deferring sign-out until the end.
#[allow(clippy::future_not_send)]
pub async fn execute_plans(plans: &[SetupPlan], options: &ExecuteOptions<'_>) -> Result<()> {
    execute_plans_with(plans, options, None, None, None, true).await
}

/// Executes plans with the complete inspection and workflow choice for CI proposals.
#[allow(clippy::future_not_send)]
pub async fn execute_plans_with(
    plans: &[SetupPlan],
    options: &ExecuteOptions<'_>,
    inspection: Option<&Inspection>,
    workflow: Option<&str>,
    environment: Option<&str>,
    summary: bool,
) -> Result<()> {
    if plans.is_empty() {
        bail!("no publishable package manifests were found");
    }
    let mut ready = Vec::new();
    let mut blocked = None;
    for plan in plans {
        if let Err(error) = validate_plan(plan, options.execute) {
            if !options.account_scan {
                return Err(error);
            }
            eprintln!(
                "{}/{}: {}: {}: blocked: {error:#}",
                plan.repository.github_owner.as_deref().unwrap_or_default(),
                plan.repository
                    .github_repository
                    .as_deref()
                    .unwrap_or_default(),
                plan.registry,
                plan.package.name
            );
            if blocked.is_none() {
                blocked = Some(error);
            }
        } else {
            ready.push(plan.clone());
        }
    }
    if let Some(error) = blocked {
        if !ready.is_empty() {
            Box::pin(execute_plans_with(
                &ready,
                options,
                inspection,
                workflow,
                environment,
                summary,
            ))
            .await?;
        }
        return Err(error);
    }
    for plan in plans {
        if plan.mode == Some(PlanMode::Complete) {
            let remaining = if plan.steps.is_empty() {
                "nothing to do."
            } else {
                "only the repository checks remain."
            };
            println!(
                "{} already publishes through trusted publishing; {remaining}",
                plan.package.name
            );
        }
    }
    if plans
        .iter()
        .all(|plan| plan.mode == Some(PlanMode::Complete) && plan.steps.is_empty())
    {
        if summary {
            println!("\nSetup summary:");
            for plan in plans {
                println!("- {}: {}: complete", plan.registry, plan.package.name);
            }
        }
        return Ok(());
    }
    if !options.execute {
        println!("Dry run only. Re-run with --execute to run these steps and open the registry.");
        return Ok(());
    }
    let missing: Vec<_> = plans
        .iter()
        .filter(|plan| {
            plan.steps
                .iter()
                .any(|step| step.id == "add-publishing-workflow")
        })
        .collect();
    if !missing.is_empty() {
        if options.account_scan {
            let roots: std::collections::BTreeSet<_> =
                missing.iter().map(|plan| &plan.repository.root).collect();
            for root in roots {
                let group: Vec<_> = missing
                    .iter()
                    .copied()
                    .filter(|plan| &plan.repository.root == root)
                    .collect();
                let inspection = Inspection {
                    schema_version: 1,
                    repository: group[0].repository.clone(),
                    packages: group.iter().map(|plan| plan.package.clone()).collect(),
                    skipped: Vec::new(),
                };
                let url = offer_workflow(
                    &inspection,
                    &group,
                    std::path::Path::new(root),
                    options.yes,
                    options.verbose,
                    crate::workflow_proposal::WorkflowOptions {
                        workflow,
                        environment,
                    },
                )
                .await?;
                if summary {
                    for plan in group {
                        println!(
                            "- {}/{}: {}: {}: {}",
                            plan.repository.github_owner.as_deref().unwrap_or_default(),
                            plan.repository
                                .github_repository
                                .as_deref()
                                .unwrap_or_default(),
                            plan.registry,
                            plan.package.name,
                            if url.is_some() {
                                "workflow-pr"
                            } else {
                                "blocked"
                            }
                        );
                    }
                }
            }
            let ready: Vec<_> = plans
                .iter()
                .filter(|plan| !missing.contains(plan))
                .cloned()
                .collect();
            if !ready.is_empty() {
                Box::pin(execute_plans_with(
                    &ready,
                    options,
                    None,
                    workflow,
                    environment,
                    summary,
                ))
                .await?;
            }
            return Ok(());
        }
        let fallback = Inspection {
            schema_version: 1,
            repository: plans[0].repository.clone(),
            packages: plans.iter().map(|plan| plan.package.clone()).collect(),
            skipped: Vec::new(),
        };
        let url = offer_workflow(
            inspection.unwrap_or(&fallback),
            &missing,
            options.repository,
            options.yes,
            options.verbose,
            crate::workflow_proposal::WorkflowOptions {
                workflow,
                environment,
            },
        )
        .await?;
        if summary {
            println!("\nSetup summary:");
            for plan in plans {
                let status = if missing.contains(&plan) && url.is_some() {
                    "workflow-pr"
                } else {
                    "blocked"
                };
                println!("- {}: {}: {status}", plan.registry, plan.package.name);
            }
        }
        return Ok(());
    }
    protect_legacy_profile(options.repository, options.browser_profile, options.verbose).await?;
    let mut browser = None;
    let mut sessions = Vec::new();
    let mut outcomes = Vec::new();
    let mut failure = None;
    let plan_options: Vec<_> = plans
        .iter()
        .map(|plan| ExecuteOptions {
            repository: std::path::Path::new(&plan.repository.root),
            ..options.clone()
        })
        .collect();
    let domains: Vec<_> = plans
        .iter()
        .flat_map(|plan| {
            crate::sign_in_import::sign_in_domains(plan.registry)
                .iter()
                .copied()
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for (plan, options) in plans.iter().zip(&plan_options) {
        let mut session = Session::new(plan, options);
        session.browser = browser.take();
        session.domains.clone_from(&domains);
        let result = session.run().await;
        let deferred = session.cleanup(plans.len() > 1, false).await;
        browser = session.browser.take();
        let details = session
            .values
            .get("secret_scope")
            .map_or(String::new(), |scope| {
                format!(
                    "; {scope} secret{}",
                    session
                        .values
                        .get("fallback_reason")
                        .map_or(String::new(), |reason| format!("; {reason}"))
                )
            });
        if result.is_ok() {
            let status = if session.values.contains_key("manifest_pr") {
                if session.values["manifest_pr"].is_empty() {
                    "blocked"
                } else {
                    "manifest-pr"
                }
            } else if plan.steps.is_empty() {
                "complete"
            } else {
                "configured"
            };
            outcomes.push(format!("{status}{details}"));
            if let Some(prefill) = &plan.oidc_publisher {
                println!("\n{}", oidc_release_note(&prefill.workflow));
            }
        } else {
            outcomes.push(format!("failed{details}"));
            if failure.is_none() {
                failure = result.err();
            }
        }
        sessions.push((session, deferred));
        if failure.is_some() && !options.account_scan {
            break;
        }
    }
    // One logout per registry/tool, including on failure. Short-lived crates.io
    // tokens and temporary release worktrees are already cleaned per package.
    let mut signed_out = std::collections::BTreeSet::new();
    for (mut session, deferred) in sessions {
        session.browser = browser.take();
        for step in deferred {
            if signed_out.insert((session.plan.registry, step.id.clone())) {
                if let Err(error) = session.run_step(&step).await {
                    eprintln!("warning: {} failed: {error:#}", step.id);
                }
            }
        }
        session.shim = None;
        browser = session.browser.take();
    }
    if let Some(browser) = browser {
        browser.close().await;
    }
    if summary {
        println!("\nSetup summary:");
        for (index, plan) in plans.iter().enumerate() {
            println!(
                "- {}{}: {}: {}",
                if options.account_scan {
                    format!(
                        "{}/{}: ",
                        plan.repository.github_owner.as_deref().unwrap_or_default(),
                        plan.repository
                            .github_repository
                            .as_deref()
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                },
                plan.registry,
                plan.package.name,
                outcomes.get(index).map_or("not-run", String::as_str)
            );
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}
