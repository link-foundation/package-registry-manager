//! Fresh default-branch and publication evidence before legacy secret deletion.
use super::{is_yes, prompt, Session};
use crate::{
    github::{GhManager, GithubGateway},
    publishers::registry_token_secrets,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;

async fn query(root: &Path, args: &[&str]) -> Result<Value> {
    Ok(serde_json::from_str(
        &crate::workflow_proposal::run(root, "gh", args, false).await?,
    )?)
}
struct Snapshot {
    slug: String,
    branch: String,
    sha: String,
    files: Vec<Value>,
    organization: bool,
}
async fn snapshot(root: &Path, slug: &str) -> Result<Snapshot> {
    let repository = query(root, &["api", &format!("repos/{slug}")]).await?;
    let branch = repository["default_branch"]
        .as_str()
        .context("cannot verify default branch")?
        .to_owned();
    let commit = query(
        root,
        &[
            "api",
            &format!(
                "repos/{slug}/commits/{}",
                crate::registry_state::encode_uri_component(&branch)
            ),
        ],
    )
    .await?;
    let sha = commit["sha"]
        .as_str()
        .context("cannot verify default-branch commit")?
        .to_owned();
    let files = GhManager
        .call(
            &[
                "repo".into(),
                "files".into(),
                slug.into(),
                "--branch".into(),
                sha.clone(),
                "--match".into(),
                ".github/workflows/*.yml".into(),
                "--match".into(),
                ".github/workflows/*.yaml".into(),
                "--content".into(),
            ],
            None,
            root,
        )
        .await?;
    Ok(Snapshot {
        slug: slug.into(),
        branch,
        sha,
        organization: repository["owner"]["type"] == "Organization",
        files: files
            .as_array()
            .context("incomplete workflow listing")?
            .clone(),
    })
}
fn reads(files: &[Value], name: &str) -> Result<bool> {
    let reference = regex::Regex::new(&format!(
        r"\bsecrets\.{name}\b|\bsecrets\s*\[|\bsecrets:\s*inherit\b"
    ))?;
    for file in files {
        let text = file["content"]
            .as_str()
            .context("incomplete workflow contents")?;
        if text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .any(|line| reference.is_match(line))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
async fn head_unchanged(root: &Path, snapshot: &Snapshot) -> Result<()> {
    let commit = query(
        root,
        &[
            "api",
            &format!(
                "repos/{}/commits/{}",
                snapshot.slug,
                crate::registry_state::encode_uri_component(&snapshot.branch)
            ),
        ],
    )
    .await?;
    if commit["sha"] != snapshot.sha {
        bail!("default branch changed during secret cleanup; secrets retained");
    }
    Ok(())
}

#[allow(clippy::future_not_send)]
impl Session<'_> {
    pub(super) async fn cleanup_migrated_secrets(&mut self) -> Result<()> {
        let root = self.options.repository;
        let owner = self
            .plan
            .repository
            .github_owner
            .as_deref()
            .context("cleanup requires GitHub coordinates")?;
        let slug = format!(
            "{owner}/{}",
            self.plan
                .repository
                .github_repository
                .as_deref()
                .context("cleanup requires GitHub coordinates")?
        );
        let mut names: Vec<String> = registry_token_secrets(self.plan.registry)
            .iter()
            .map(|name| (*name).into())
            .collect();
        if let Some(name) = self.options.secret_name {
            if !name.contains('{') && !names.iter().any(|existing| existing == name) {
                names.push(name.into());
            }
        }
        let current = snapshot(root, &slug).await?;
        for name in &names {
            if reads(&current.files, name)? {
                println!("{slug} still reads {name}; merge the conversion PR before cleanup.");
                self.values
                    .insert("migration_status".into(), "migration-required".into());
                return Ok(());
            }
        }
        let workflow = self
            .plan
            .oidc_publisher
            .as_ref()
            .map(|publisher| publisher.workflow.as_str())
            .or(self.plan.package.workflow.as_deref())
            .context("cleanup requires a publishing workflow")?;
        let runs = query(
            root,
            &[
                "run",
                "list",
                "--repo",
                &slug,
                "--workflow",
                workflow,
                "--branch",
                &current.branch,
                "--limit",
                "100",
                "--json",
                "databaseId,headSha,conclusion,event",
            ],
        )
        .await?;
        let mut verified = false;
        for run in runs
            .as_array()
            .context("invalid release run list")?
            .iter()
            .filter(|run| {
                run["headSha"] == current.sha
                    && run["conclusion"] == "success"
                    && run["event"].as_str().is_some_and(|event| {
                        ["push", "workflow_dispatch", "release"].contains(&event)
                    })
            })
        {
            let id = run["databaseId"].to_string();
            let details = query(
                root,
                &["run", "view", &id, "--repo", &slug, "--json", "jobs"],
            )
            .await?;
            for job in details["jobs"]
                .as_array()
                .context("invalid publishing job list")?
            {
                if job["conclusion"] != "success" {
                    continue;
                }
                let log = crate::workflow_proposal::run(
                    root,
                    "gh",
                    &[
                        "api",
                        &format!("repos/{slug}/actions/jobs/{}/logs", job["databaseId"]),
                    ],
                    false,
                )
                .await?;
                if crate::migration_evidence::published_through_oidc(job, &log, self.plan.registry)
                {
                    verified = true;
                    break;
                }
            }
            if verified {
                break;
            }
        }
        if !verified {
            println!("{slug}: legacy secrets retained until {workflow} publishes through OIDC at default-branch commit {}. Re-run setup --migrate after the release succeeds.",current.sha);
            self.values
                .insert("migration_status".into(), "awaiting-oidc-release".into());
            return Ok(());
        }
        let mut deletions = Vec::new();
        for name in &names {
            let args = vec![
                "secret".into(),
                "get-metadata".into(),
                name.clone(),
                "--repo".into(),
                slug.clone(),
            ];
            if !GhManager.call(&args, None, root).await?.is_null() {
                deletions.push(("--repo", slug.clone(), name.clone()));
            }
        }
        let mut snapshots = Vec::new();
        if self.options.repository_batch && current.organization {
            let repositories = GhManager
                .call(
                    &[
                        "repo".into(),
                        "list".into(),
                        "--org".into(),
                        owner.into(),
                        "--include-archived".into(),
                        "--include-forks".into(),
                    ],
                    None,
                    root,
                )
                .await?;
            for repository in repositories
                .as_array()
                .context("incomplete repository listing")?
            {
                snapshots.push(
                    snapshot(
                        root,
                        repository["full_name"]
                            .as_str()
                            .context("invalid repository coordinates")?,
                    )
                    .await?,
                );
            }
            for name in &names {
                let mut consumers = Vec::new();
                for item in &snapshots {
                    if reads(&item.files, name)? {
                        consumers.push(item.slug.clone());
                    }
                }
                if !consumers.is_empty() {
                    println!(
                        "Keep organization {name}; migrate: {}",
                        consumers.join(", ")
                    );
                    continue;
                }
                let args = vec![
                    "secret".into(),
                    "get-metadata".into(),
                    name.clone(),
                    "--org".into(),
                    owner.into(),
                ];
                if !GhManager.call(&args, None, root).await?.is_null() {
                    deletions.push(("--org", owner.into(), name.clone()));
                }
            }
        } else {
            println!("Organization secrets retained; run setup --org {owner} --registry {} --migrate to check every consumer.",self.plan.registry);
        }
        if !deletions.is_empty()
            && !self.options.yes
            && !is_yes(&prompt(&format!(
                "Delete verified legacy secrets {}? [y/N] ",
                deletions
                    .iter()
                    .map(|(_, scope, name)| format!("{scope}:{name}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))?)
        {
            return Ok(());
        }
        // Confirmation can take time, so recheck every consumer after it returns.
        for item in &snapshots {
            head_unchanged(root, item).await?;
        }
        head_unchanged(root, &current).await?;
        for (flag, scope, name) in deletions {
            GhManager
                .call(
                    &[
                        "secret".into(),
                        "delete".into(),
                        name.clone(),
                        flag.into(),
                        scope.clone(),
                    ],
                    None,
                    root,
                )
                .await?;
            let companion = format!("{name}_EXPIRES_AT");
            let args = vec![
                "secret".into(),
                "get-metadata".into(),
                companion.clone(),
                flag.into(),
                scope.clone(),
            ];
            if !GhManager.call(&args, None, root).await?.is_null() {
                GhManager
                    .call(
                        &[
                            "secret".into(),
                            "delete".into(),
                            companion,
                            flag.into(),
                            scope.clone(),
                        ],
                        None,
                        root,
                    )
                    .await?;
            }
            println!("Deleted {scope}:{name} after verified OIDC publication.");
        }
        self.values
            .insert("migration_status".into(), "migrated".into());
        Ok(())
    }
}
