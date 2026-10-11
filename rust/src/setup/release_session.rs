//! Select one release run for both trust attachment and repository repair.

use anyhow::{bail, Result};
use serde_json::Value;

use super::{json_list, json_text, Session};
use crate::model::{SetupStep, StepKind};
use crate::release_runs::{eligible_release_runs, failed_publishing_job, RELEASE_RUN_FIELDS};
use crate::repository_identity::repository_slug;

#[derive(Clone)]
pub(super) struct ReleaseSelection {
    pub run: Option<Value>,
    pub publish_failure: bool,
    pub branch: String,
}

#[allow(clippy::future_not_send)] // Session includes native browser automation.
impl Session<'_> {
    async fn release_query(&self, args: &[&str]) -> Result<String> {
        let step = SetupStep::new(
            "release-lookup",
            "Read release runs",
            StepKind::Check,
            "Read default-branch release history",
        )
        .command("gh", args)
        .cwd(".");
        let result = self.capture(&step).await?;
        if result.code != 0 {
            bail!("could not inspect default-branch release runs with gh");
        }
        Ok(result.stdout)
    }

    pub(super) async fn select_release_run(&mut self) -> Result<ReleaseSelection> {
        if let Some(selection) = &self.release_selection {
            return Ok(selection.clone());
        }
        let slug = repository_slug(&self.plan.repository).unwrap_or_default();
        let workflow = self
            .plan
            .oidc_publisher
            .as_ref()
            .map_or("", |publisher| publisher.workflow.as_str());
        let result = self
            .release_query(&["api", &format!("repos/{slug}"), "--jq", ".default_branch"])
            .await?;
        let branch = result.trim().to_owned();
        if branch.is_empty()
            || branch.starts_with('-')
            || branch
                .chars()
                .any(|character| character.is_whitespace() || character == ':')
        {
            bail!("could not determine the release workflow's default branch");
        }
        let mut runs = Vec::new();
        for event in ["push", "workflow_dispatch"] {
            let output = self
                .release_query(&[
                    "run",
                    "list",
                    "--repo",
                    &slug,
                    "--workflow",
                    workflow,
                    "--branch",
                    &branch,
                    "--event",
                    event,
                    "--limit",
                    "100",
                    "--json",
                    RELEASE_RUN_FIELDS,
                ])
                .await?;
            runs.extend(json_list(&output)?);
        }
        let eligible = eligible_release_runs(runs, &branch);
        let mut selection = ReleaseSelection {
            run: eligible.first().cloned(),
            publish_failure: false,
            branch,
        };
        for run in eligible.iter().filter(|run| run["conclusion"] == "failure") {
            let output = self
                .release_query(&[
                    "run",
                    "view",
                    &json_text(&run["databaseId"]),
                    "--repo",
                    &slug,
                    "--json",
                    "jobs",
                ])
                .await?;
            let details: Value = serde_json::from_str(&output)?;
            if failed_publishing_job(&details["jobs"]) {
                selection.run = Some(run.clone());
                selection.publish_failure = true;
                break;
            }
        }
        self.release_selection = Some(selection.clone());
        Ok(selection)
    }

    pub(super) async fn inspect_release_run(&mut self) -> Result<()> {
        let selection = match self.select_release_run().await {
            Ok(selection) => selection,
            Err(error) => {
                eprintln!("warning: {error}");
                return Ok(());
            }
        };
        let Some(run) = selection.run else {
            println!("  The release workflow has not run yet.");
            return Ok(());
        };
        let outcome = run["conclusion"]
            .as_str()
            .filter(|text| !text.is_empty())
            .map_or_else(|| json_text(&run["status"]), str::to_owned);
        let label = if selection.publish_failure {
            "selected failed"
        } else {
            "latest"
        };
        println!(
            "  The {label} release run {outcome}: {}",
            json_text(&run["url"])
        );
        if selection.publish_failure {
            self.values
                .insert("failed_run_id".to_owned(), json_text(&run["databaseId"]));
            self.conditions.insert("release-failed");
        }
        Ok(())
    }
}
