//! Execute transfer repairs without trusting a historical publisher identity.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

use super::{is_yes, prompt, Session};
use crate::crates_api::page_fetch_script;
use crate::manifest_proposal::{offer_manifest_repository, verify_remote_manifest};
use crate::model::{PublisherIdentity, Registry, SetupStep, StepKind};
use crate::repository_identity::{
    npm_publishers, publisher_identities, publisher_matches, repository_slug,
};

const PYPI_PUBLISHERS_SCRIPT: &str = r##"(() => [...document.querySelectorAll(".table--publisher-list tbody tr")].flatMap(row => {
  const details = row.querySelector("small");
  const link = details?.querySelector('a[href^="https://github.com/"]');
  const remove = row.querySelector('a[href^="#remove-publisher-"]');
  if (!link || !remove) return [];
  const field = label => {
    const heading = [...details.querySelectorAll("b")].find(item => item.textContent.trim() === label);
    if (!heading) return undefined;
    let value = "";
    for (let node = heading.nextSibling; node && node.nodeName !== "BR"; node = node.nextSibling) {
      if (node.nodeName === "I") return null;
      value += node.textContent;
    }
    return value.trim() || null;
  };
  const workflow = field("Workflow:");
  const environment = field("Environment name:");
  if (!workflow || environment === undefined) return [];
  return [{ id: remove.getAttribute("href").slice(18), repository: link.href, workflow, environment }];
}))()"##;

#[allow(clippy::future_not_send)] // Native browser automation is intentionally !Sync.
impl Session<'_> {
    async fn repository_publishers(&mut self) -> Result<Vec<PublisherIdentity>> {
        match self.plan.registry {
            Registry::Npm => {
                let step = self
                    .plan
                    .steps
                    .iter()
                    .find(|step| step.id == "check-trust")
                    .ok_or_else(|| anyhow!("missing npm trust lookup"))?;
                let result = self.run_process(step, false).await?;
                if result.code != 0 {
                    bail!("could not read npm trusted publishers; repository repair stopped");
                }
                Ok(npm_publishers(&result.stdout))
            }
            Registry::CratesIo => {
                let mut publishers = Vec::new();
                let mut query = format!(
                    "?crate={}",
                    crate::registry_state::encode_uri_component(&self.plan.package.name)
                );
                let mut seen = std::collections::BTreeSet::new();
                while !query.is_empty() {
                    if !query.starts_with('?') || !seen.insert(query.clone()) {
                        bail!("invalid crates.io publisher pagination");
                    }
                    let response = self
                        .automated()
                        .await?
                        .evaluate(&page_fetch_script(
                            "GET",
                            &format!("/api/v1/trusted_publishing/github_configs{query}"),
                            None,
                        ))
                        .await?;
                    if response["status"] != 200 || !response["body"]["github_configs"].is_array() {
                        bail!("could not read crates.io trusted publishers");
                    }
                    publishers.extend(publisher_identities(&response["body"]));
                    response["body"]["meta"]["next_page"]
                        .as_str()
                        .unwrap_or_default()
                        .clone_into(&mut query);
                }
                Ok(publishers)
            }
            _ => {
                let url = format!(
                    "https://pypi.org/manage/project/{}/settings/publishing/",
                    crate::registry_state::encode_uri_component(&self.plan.package.name)
                );
                self.open(&url, true).await?;
                prompt("Sign in and show the project's trusted publishers, then press Enter...")?;
                let rows = self
                    .automated()
                    .await?
                    .evaluate(PYPI_PUBLISHERS_SCRIPT)
                    .await?;
                if !rows.is_array() {
                    bail!("could not read PyPI trusted publishers");
                }
                Ok(publisher_identities(&rows))
            }
        }
    }

    async fn verify_repository_publisher(&mut self) -> Result<Vec<PublisherIdentity>> {
        let publishers = self.repository_publishers().await?;
        let expected = self
            .plan
            .oidc_publisher
            .as_ref()
            .ok_or_else(|| anyhow!("unknown replacement publisher"))?;
        if !publishers
            .iter()
            .any(|publisher| publisher_matches(publisher, expected))
        {
            bail!("registry settings do not list the new repository's workflow and environment; old publishers were kept");
        }
        self.conditions.insert("repository-publisher-verified");
        self.conditions.remove("trust-missing");
        Ok(publishers)
    }

    async fn remove_old_publishers(&mut self) -> Result<()> {
        if !self.holds("repository-publisher-verified") {
            bail!("verify the replacement publisher before removing old trust");
        }
        let publishers = self.verify_repository_publisher().await?;
        let current = repository_slug(&self.plan.repository).unwrap_or_default();
        let old: std::collections::BTreeSet<_> = publishers
            .iter()
            .filter(|publisher| !publisher.repository.eq_ignore_ascii_case(&current))
            .map(|publisher| publisher.repository.to_lowercase())
            .collect();
        for publisher in publishers
            .iter()
            .filter(|publisher| old.contains(&publisher.repository.to_lowercase()))
        {
            let id = publisher
                .id
                .as_deref()
                .filter(|id| {
                    !id.is_empty()
                        && id
                            .bytes()
                            .all(|ch| ch.is_ascii_alphanumeric() || b"_-".contains(&ch))
                })
                .ok_or_else(|| anyhow!("the old publisher has no usable registry identifier"))?;
            if !self.options.yes
                && !is_yes(&prompt(&format!(
                    "Remove trusted publisher {} ({})? [y/N] ",
                    publisher.repository,
                    publisher.workflow.as_deref().unwrap_or_default()
                ))?)
            {
                bail!("old publisher removal declined; release retry stopped");
            }
            match self.plan.registry {
                Registry::Npm => {
                    let mut step = self
                        .plan
                        .steps
                        .iter()
                        .find(|step| step.id == "check-trust")
                        .ok_or_else(|| anyhow!("missing npm trust lookup"))?
                        .clone();
                    let command = step
                        .command
                        .as_mut()
                        .ok_or_else(|| anyhow!("missing npm trust command"))?;
                    let index = command
                        .args
                        .iter()
                        .position(|arg| arg == "list")
                        .ok_or_else(|| anyhow!("missing npm list argument"))?;
                    command.args[index] = "revoke".into();
                    command
                        .args
                        .extend(["--id".into(), id.into(), "--yes".into()]);
                    if self.run_process(&step, true).await?.code != 0 {
                        bail!("npm did not revoke the old publisher");
                    }
                }
                Registry::CratesIo => {
                    let response = self
                        .automated()
                        .await?
                        .evaluate(&page_fetch_script(
                            "DELETE",
                            &format!("/api/v1/trusted_publishing/github_configs/{id}"),
                            None,
                        ))
                        .await?;
                    if !response["status"]
                        .as_u64()
                        .is_some_and(|status| (200..300).contains(&status))
                    {
                        bail!("crates.io did not remove the old publisher");
                    }
                }
                _ => {
                    let script = format!(
                        r#"(async () => {{
                        const input = [...document.querySelectorAll('input[name="publisher_id"]')].find(input => input.value === {});
                        const form = input?.closest("form");
                        if (!form || form.method.toLowerCase() !== "post" || form.action !== location.href.split("?")[0]) throw new Error("PyPI publisher removal form unavailable");
                        const response = await fetch(form.action, {{ method: "POST", body: new FormData(form), credentials: "same-origin" }});
                        return {{ status: response.status }};
                    }})()"#,
                        serde_json::to_string(id)?
                    );
                    let result = self.automated().await?.evaluate(&script).await?;
                    if !result["status"]
                        .as_u64()
                        .is_some_and(|status| (200..300).contains(&status))
                    {
                        bail!("PyPI did not remove the old publisher");
                    }
                }
            }
        }
        if self
            .verify_repository_publisher()
            .await?
            .iter()
            .any(|publisher| old.contains(&publisher.repository.to_lowercase()))
        {
            bail!("the old repository still has a trusted publisher; release retry stopped");
        }
        Ok(())
    }

    pub(super) async fn repository_repair_step(&mut self, step: &SetupStep) -> Result<()> {
        match step.id.as_str() {
            "check-repository-publisher" => {
                let publishers = self.repository_publishers().await?;
                let expected = self
                    .plan
                    .oidc_publisher
                    .as_ref()
                    .ok_or_else(|| anyhow!("unknown replacement publisher"))?;
                if publishers
                    .iter()
                    .any(|publisher| publisher_matches(publisher, expected))
                {
                    self.conditions.remove("trust-missing");
                } else {
                    self.conditions.insert("trust-missing");
                }
            }
            "verify-repository-publisher" => {
                self.verify_repository_publisher().await?;
            }
            "remove-old-publisher" => self.remove_old_publishers().await?,
            "fix-manifest-repository" => {
                match offer_manifest_repository(
                    self.plan,
                    self.options.repository,
                    self.options.yes,
                    self.options.verbose,
                )
                .await?
                {
                    Some(url) if url.is_empty() => {}
                    url => {
                        self.values
                            .insert("manifest_pr".into(), url.unwrap_or_default());
                    }
                }
            }
            _ => {
                if !self.holds("repository-publisher-verified") {
                    bail!("verify repository repairs before retrying release jobs");
                }
                let slug = repository_slug(&self.plan.repository).unwrap_or_default();
                let expected = self
                    .plan
                    .oidc_publisher
                    .as_ref()
                    .ok_or_else(|| anyhow!("unknown replacement publisher"))?;
                let lookup = SetupStep::new(
                    "release-lookup",
                    "Read failed release jobs",
                    StepKind::Check,
                    "Read the latest workflow run",
                )
                .command(
                    "gh",
                    &[
                        "run",
                        "list",
                        "--repo",
                        &slug,
                        "--workflow",
                        &expected.workflow,
                        "--limit",
                        "1",
                        "--json",
                        "databaseId,conclusion,headSha",
                    ],
                )
                .cwd(".");
                let result = self.capture(&lookup).await?;
                if result.code != 0 {
                    bail!("could not inspect release jobs after repository repair");
                }
                let runs: Value = serde_json::from_str(&result.stdout)?;
                let run = &runs[0];
                if run["conclusion"] != "failure" {
                    return Ok(());
                }
                let corrected =
                    if let Some(sha) = run["headSha"].as_str().filter(|sha| !sha.is_empty()) {
                        verify_remote_manifest(
                            self.plan,
                            self.options.repository,
                            Some(sha),
                            self.options.verbose,
                        )
                        .await?
                    } else {
                        false
                    };
                if !corrected
                    && !verify_remote_manifest(
                        self.plan,
                        self.options.repository,
                        None,
                        self.options.verbose,
                    )
                    .await?
                {
                    bail!("merge the manifest repository URL fix before retrying release jobs");
                }
                let id = run["databaseId"].to_string();
                let args = if corrected {
                    vec!["run", "rerun", &id, "--repo", &slug, "--failed"]
                } else {
                    vec!["workflow", "run", &expected.workflow, "--repo", &slug]
                };
                let retry = SetupStep::new(
                    "retry-repaired-release",
                    &step.title,
                    StepKind::Command,
                    &step.description,
                )
                .command("gh", &args)
                .cwd(".")
                .confirmed();
                self.command(&retry).await?;
            }
        }
        Ok(())
    }
}
