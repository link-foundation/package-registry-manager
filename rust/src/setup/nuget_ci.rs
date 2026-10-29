//! Follow current main, causal authentication repairs and exact public consumers.
use super::{
    bail, json_list, json_text, Context, Instant, Regex, Registry, Result, Session, SetupStep,
    StepKind, Value,
};
use crate::github::{GhManager, GithubGateway};
use crate::repository_identity::repository_slug;

impl Session<'_> {
    pub(super) async fn verify_nuget_release(&mut self) -> Result<()> {
        let slug =
            repository_slug(&self.plan.repository).context("GitHub repository is missing")?;
        let workflow = &self
            .plan
            .oidc_publisher
            .as_ref()
            .context("publisher is missing")?
            .workflow;
        let branch = self
            .release_query(&["api", &format!("repos/{slug}"), "--jq", ".default_branch"])
            .await?;
        let branch = branch.trim();
        let sha = self
            .release_query(&[
                "api",
                &format!("repos/{slug}/commits/{branch}"),
                "--jq",
                ".sha",
            ])
            .await?;
        let sha = sha.trim();
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
                    branch,
                    "--event",
                    event,
                    "--limit",
                    "100",
                    "--json",
                    crate::release_runs::RELEASE_RUN_FIELDS,
                ])
                .await?;
            runs.extend(json_list(&output)?);
        }
        let runs = crate::release_runs::eligible_release_runs(runs, branch);
        let current = runs.iter().find(|run| run["headSha"] == sha);
        let failed = runs.iter().find(|run| run["conclusion"] == "failure");
        let id = if let Some(failed) = failed.filter(|_| {
            self.values.contains_key("verified_recovery_change")
                && current.is_none_or(|run| run["conclusion"] != "success")
        }) {
            let run_id = json_text(&failed["databaseId"]);
            let output = self
                .release_query(&["run", "view", &run_id, "--repo", &slug, "--json", "jobs"])
                .await?;
            let document: Value = serde_json::from_str(&output)?;
            let auth = Regex::new(&format!(
                "(?i){}",
                crate::github::failure_patterns(Some(Registry::NuGet)).join("|")
            ))?;
            let mut causal = Vec::new();
            for job in document["jobs"]
                .as_array()
                .context("jobs are unavailable")?
                .iter()
                .filter(|job| job["conclusion"] == "failure")
            {
                let source = self
                    .release_query(&[
                        "run",
                        "view",
                        &run_id,
                        "--repo",
                        &slug,
                        "--job",
                        &json_text(&job["databaseId"]),
                        "--log",
                    ])
                    .await?;
                if auth.is_match(&source) && source.to_lowercase().contains("nuget") {
                    causal.push(job["databaseId"].clone());
                }
            }
            if causal.is_empty() {
                bail!("NuGet settings are verified but no causal authentication failure was established; unrelated CI was not restarted");
            }
            let mut changes =
                vec![serde_json::json!({"kind":"setting","changed":true,"verified":true})];
            let mut request = serde_json::json!({"repo":slug,"workflow":workflow,"runId":failed["databaseId"],"jobIds":causal,"timeout":self.options.wait_timeout.as_millis()});
            if failed["headSha"] != sha {
                let current = current.context("NuGet code repair has no run at current main; merge the reviewed workflow and follow its push run")?;
                changes.push(serde_json::json!({"kind":"workflow","changed":true,"verified":true}));
                request["revision"] = serde_json::json!({"ref":branch,"sha":sha});
                request["newRunId"] = current["databaseId"].clone();
            }
            request["changes"] = serde_json::json!(changes);
            request["requiredPermissions"] = serde_json::json!(self
                .plan
                .package
                .workflow_jobs
                .iter()
                .map(|job| serde_json::json!({"job":job,"permission":"id-token","level":"write"}))
                .collect::<Vec<_>>());
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
            println!("{}", serde_json::json!({"nuget_ci":result}));
            if result["status"] != "success" {
                bail!(
                    "NuGet repaired CI remains {}; {}",
                    result["status"],
                    result["runUrl"]
                );
            }
            Some(json_text(&result["runId"]))
        } else {
            current.map(|run| json_text(&run["databaseId"]))
        };
        let id = id.context("NuGet current-main release has not executed; PR checks and settings readback do not verify publication")?;
        let deadline = Instant::now() + self.options.wait_timeout;
        let run: Value = loop {
            let output = self
                .release_query(&[
                    "run",
                    "view",
                    &id,
                    "--repo",
                    &slug,
                    "--json",
                    "status,conclusion,headSha,url,attempt,jobs",
                ])
                .await?;
            let run: Value = serde_json::from_str(&output)?;
            if run["status"] == "completed" {
                break run;
            }
            if Instant::now() >= deadline {
                bail!(
                    "NuGet current-main pipeline remains pending: {}",
                    run["url"]
                );
            }
            tokio::time::sleep(self.options.poll_interval).await;
        };
        let jobs = run["jobs"].as_array().context("CI jobs are unavailable")?;
        if run["headSha"] != sha
            || run["status"] != "completed"
            || run["conclusion"] != "success"
            || jobs.is_empty()
            || jobs.iter().any(|job| job["conclusion"] != "success")
        {
            bail!(
                "NuGet current-main pipeline has incomplete gates: {}",
                run["url"]
            );
        }
        let mut methods = Vec::new();
        for job in jobs {
            let source = self
                .release_query(&[
                    "run",
                    "view",
                    &id,
                    "--repo",
                    &slug,
                    "--job",
                    &json_text(&job["databaseId"]),
                    "--log",
                ])
                .await?;
            for line in source.lines() {
                if let Some(start) = line.find("{\"selected_method\"") {
                    if let Ok(evidence) = serde_json::from_str::<Value>(&line[start..]) {
                        if evidence["publication"] == "accepted"
                            && evidence["package_id"].as_str().is_some_and(|name| {
                                name.eq_ignore_ascii_case(&self.plan.package.name)
                            })
                            && evidence["version"].as_str().is_some_and(|version| {
                                !version.is_empty()
                                    && version
                                        .bytes()
                                        .all(|c| c.is_ascii_alphanumeric() || b".+-".contains(&c))
                            })
                            && matches!(
                                evidence["selected_method"].as_str(),
                                Some("trusted-publishing" | "token-fallback")
                            )
                        {
                            methods
                                .push(serde_json::json!({"job":job["name"],"evidence":evidence}));
                        }
                    }
                }
            }
        }
        if methods.is_empty() {
            bail!("NuGet authentication method and actual upload are unverified; use the reviewed publication controller in current-main CI");
        }
        let fresh = self
            .release_query(&[
                "api",
                &format!("repos/{slug}/commits/{branch}"),
                "--jq",
                ".sha",
            ])
            .await?;
        if fresh.trim() != sha {
            bail!("main advanced during NuGet verification; follow its new run before claiming completion");
        }
        let versions: std::collections::BTreeSet<_> = methods
            .iter()
            .filter_map(|method| method["evidence"]["version"].as_str())
            .collect();
        if versions.len() != 1 {
            bail!("NuGet actual released version is ambiguous");
        }
        let version = *versions
            .first()
            .context("exact NuGet public version is unresolved")?;
        let directory = tempfile::tempdir()?;
        let file = directory.path().join("verify-nuget.py");
        std::fs::write(&file, include_str!("../nuget-publish.py"))?;
        let result = self
            .capture(
                &SetupStep::new(
                    "verify-public-nuget",
                    "Verify exact NuGet public consumer",
                    StepKind::Check,
                    "Download and restore/build the exact public package.",
                )
                .command(
                    "python3",
                    &[
                        &file.to_string_lossy(),
                        "--verify-public",
                        &self.plan.package.name,
                        version,
                    ],
                )
                .cwd("."),
            )
            .await?;
        if result.code != 0 {
            bail!("exact NuGet public download and consumer restore/build remain unverified");
        }
        println!(
            "{}",
            serde_json::json!({"status":"verified","url":run["url"],"ci":{"run_id":id,"attempt":run["attempt"],"sha":sha,"jobs":jobs,"authentication":methods},"public_version":version,"consumer":"passed"})
        );
        self.values
            .insert("nuget_release_verified".into(), "true".into());
        Ok(())
    }
}
