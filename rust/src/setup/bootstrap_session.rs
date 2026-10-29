//! Revalidate prerequisites and restart only causally affected failed jobs.
use super::{bail, json_text, BTreeMap, Context, Result, Session, Value};
use crate::github::{GhManager, GithubGateway};
use crate::repository_identity::{npm_publishers, publisher_matches, repository_slug};

impl Session<'_> {
    pub(super) async fn recover_bootstrap(&mut self) -> Result<()> {
        let names: Vec<String> = serde_json::from_str(
            self.values
                .get("bootstrap_names")
                .context("missing bootstrap evidence")?,
        )?;
        let publisher = self
            .plan
            .oidc_publisher
            .as_ref()
            .context("publisher identity is missing")?;
        for name in &names {
            let mut item = self.plan.package.clone();
            item.name.clone_from(name);
            if self.client.probe_package(&item).await.exists != Some(true) {
                bail!("bootstrap preflight still unresolved for {name}");
            }
            let mut step = self
                .plan
                .steps
                .iter()
                .find(|s| s.id == "verify-trusted-publisher")
                .cloned()
                .context("publisher verification command is missing")?;
            if let Some(command) = &mut step.command {
                for argument in &mut command.args {
                    if *argument == self.plan.package.name {
                        argument.clone_from(name);
                    }
                }
            }
            let result = self.capture(&step).await?;
            if result.code != 0
                || !npm_publishers(&result.stdout)
                    .iter()
                    .any(|p| publisher_matches(p, publisher))
            {
                bail!("bootstrap publisher is not verified for {name}");
            }
        }
        let selection = self
            .release_selection
            .clone()
            .context("release selection is missing")?;
        let run = selection.run.context("failed run is missing")?;
        let slug =
            repository_slug(&self.plan.repository).context("GitHub repository is missing")?;
        let sha = self
            .release_query(&[
                "api",
                &format!("repos/{slug}/commits/{}", selection.branch),
                "--jq",
                ".sha",
            ])
            .await?;
        if sha.trim() != run["headSha"].as_str().unwrap_or_default() {
            bail!("default branch changed; follow the new revision's release instead of rerunning the old SHA");
        }
        let mut ids = Vec::new();
        for job in selection
            .jobs
            .iter()
            .filter(|j| j["conclusion"] == "failure")
        {
            let output = self
                .release_query(&[
                    "run",
                    "view",
                    &json_text(&run["databaseId"]),
                    "--repo",
                    &slug,
                    "--job",
                    &json_text(&job["databaseId"]),
                    "--log",
                ])
                .await?;
            if crate::bootstrap_recovery::bootstrap_names(&output)
                .iter()
                .any(|name| names.contains(name))
            {
                ids.push(job["databaseId"].clone());
            }
        }
        if ids.is_empty() {
            bail!(
                "no causal bootstrap job could be verified; unrelated failed jobs were not retried"
            );
        }
        if !self.values.contains_key("verified_recovery_change") {
            bail!("no verified configuration change in this setup; CI was not restarted");
        }
        let request = serde_json::json!({"repo":slug,"workflow":publisher.workflow,"runId":run["databaseId"],"jobIds":ids,"changes":[{"kind":"setting","changed":true,"verified":true}],"timeout":self.options.wait_timeout.as_millis()});
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("repair.json");
        std::fs::write(&file, serde_json::to_vec(&request)?)?;
        let result = GhManager
            .call(
                &[
                    "repair".into(),
                    "verify".into(),
                    file.to_string_lossy().into_owned(),
                    "--timeout".into(),
                    self.options.wait_timeout.as_secs().to_string(),
                ],
                None,
                self.options.repository,
            )
            .await?;
        println!("{}", serde_json::json!({"bootstrap_recovery":result}));
        if result["status"] != "success" {
            bail!(
                "repaired bootstrap CI remains {}; {}",
                result["status"],
                result["runUrl"]
            );
        }
        self.values
            .insert("run_id".into(), json_text(&run["databaseId"]));
        self.conditions.remove("release-dispatch");
        self.verify_npm_release(&json_text(&result["runId"]))
            .await?;
        self.values
            .insert("bootstrap_release_verified".into(), "true".into());
        Ok(())
    }

    async fn verify_npm_release(&self, run_id: &str) -> Result<()> {
        let package = &self.plan.package;
        let metadata = package
            .audit
            .get("generated_alias")
            .or_else(|| package.audit.get("generated_artifact"));
        let names: Vec<String> = metadata
            .and_then(|value| value["names"].as_array())
            .map_or_else(
                || vec![package.name.clone()],
                |names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                },
            );
        let slug =
            repository_slug(&self.plan.repository).context("GitHub repository is missing")?;
        let output = self
            .release_query(&["run", "view", run_id, "--repo", &slug, "--json", "jobs"])
            .await?;
        let document: Value = serde_json::from_str(&output)?;
        let mut versions = BTreeMap::new();
        for job in document["jobs"]
            .as_array()
            .context("release jobs are unavailable; exact alias versions remain unverified")?
        {
            let source = self
                .release_query(&[
                    "run",
                    "view",
                    run_id,
                    "--repo",
                    &slug,
                    "--job",
                    &json_text(&job["databaseId"]),
                    "--log",
                ])
                .await?;
            versions.extend(crate::bootstrap_recovery::published_versions(
                &source, &names,
            ));
        }
        let mut evidence = Vec::new();
        for name in names {
            let version = versions.get(&name).with_context(|| {
                format!("successful release log does not establish the actual version for {name}")
            })?;
            let mut item = package.clone();
            item.name.clone_from(&name);
            item.version = Some(version.clone());
            let state = self.client.probe_package(&item).await;
            if state.audit.get("manifest_version_exists") != Some(&Value::Bool(true)) {
                bail!("exact released version {name}@{version} remains unverified");
            }
            evidence.push(serde_json::json!({"name":name,"version":version,"url":state.audit.get("version_evidence_url")}));
        }
        println!("{}", serde_json::json!({"npm_release":evidence}));
        Ok(())
    }
}
