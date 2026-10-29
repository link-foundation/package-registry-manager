//! Organization/user discovery from gh-manager API snapshots, without cloning repositories.
use crate::{
    github::{failure_patterns, GithubGateway},
    registry_state::RegistryClient,
    InspectOptions, Inspection,
};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::Path;

/// Manifest, workflow, and configuration globs supplied to gh-manager.
#[must_use]
pub fn scan_matches() -> Vec<String> {
    let mut patterns: Vec<_> = [
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "setup.py",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "composer.json",
        "jsr.json",
        "deno.json",
        "*.csproj",
        "*.fsproj",
        "*.vbproj",
        "*.gemspec",
        "Dockerfile*",
        "Containerfile*",
        "docker-compose*.yml",
        "docker-compose*.yaml",
        "compose*.yml",
        "compose*.yaml",
    ]
    .iter()
    .map(|name| format!("**/{name}"))
    .collect();
    patterns.extend(
        [
            ".github/workflows/*.yml",
            ".github/workflows/*.yaml",
            ".package-registry-manager.json",
        ]
        .map(str::to_owned),
    );
    patterns
}

/// Materialize API file contents for the existing parser, rejecting unsafe paths.
pub fn snapshot(root: &Path, slug: &str, files: &Value) -> Result<()> {
    for file in files
        .as_array()
        .ok_or_else(|| anyhow!("gh-manager returned an invalid file list"))?
    {
        let name = file["path"]
            .as_str()
            .ok_or_else(|| anyhow!("file path missing"))?;
        if name.contains(['\\', ':'])
            || name
                .split('/')
                .any(|part| matches!(part, "" | "." | ".." | ".git"))
        {
            bail!("unsafe repository file path");
        }
        let contents = file["content"]
            .as_str()
            .ok_or_else(|| anyhow!("gh-manager returned no file content"))?;
        let target = root.join(name);
        std::fs::create_dir_all(target.parent().expect("file parent"))?;
        std::fs::write(target, contents)?;
    }
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::write(
        root.join(".git/config"),
        format!("[remote \"origin\"]\n  url = https://github.com/{slug}.git\n"),
    )?;
    Ok(())
}

/// Package findings keep unknown registry answers distinct from unpublished packages.
#[must_use]
pub fn findings(inspection: &Inspection) -> Vec<Value> {
    let slug =
        crate::repository_identity::repository_slug(&inspection.repository).unwrap_or_default();
    let mut results = Vec::new();
    for package in inspection
        .packages
        .iter()
        .filter(|package| package.publishable)
    {
        let common = json!({"repository":slug,"registry":package.registry,"package":package.name,"manifest":package.manifest});
        let mut push = |kind: &str, evidence: Value| {
            let mut finding = common.clone();
            finding["type"] = json!(kind);
            if !evidence.is_null() {
                finding["evidence"] = evidence;
            }
            results.push(finding);
        };
        if let Some(kind) = package.audit.get("version_state").and_then(Value::as_str) {
            push(
                kind,
                json!({"expected_version":package.version,"latest_version":package.audit.get("latest_version"),"url":package.audit.get("version_evidence_url").or_else(||package.audit.get("registry_evidence_url"))}),
            );
        }
        if package.exists_on_registry.is_none()
            && package.audit.contains_key("registry_state_reason")
        {
            push(
                "registry-state-unknown",
                json!({"reason":package.audit.get("registry_state_reason")}),
            );
        }
        if package.exists_on_registry == Some(false) {
            push("unpublished", Value::Null);
        }
        if package.exists_on_registry.is_none() && package.registry_http_status.is_some() {
            push(
                "registry-state-unknown",
                json!({"http_status":package.registry_http_status}),
            );
        }
        if package.exists_on_registry == Some(true)
            && (package.trusted_publishing == Some(false)
                || crate::credential_cycle::credential_policy(package.registry).mode == "token")
        {
            push("published-without-trusted-publishing", Value::Null);
        }
        let others: Vec<_> = package
            .configured_publishers
            .iter()
            .flatten()
            .filter(|publisher| !publisher.repository.eq_ignore_ascii_case(&slug))
            .collect();
        if !others.is_empty() {
            push("trusted-publisher-other-repository", json!(others));
        }
    }
    results
}

/// One discovered repository or a recoverable scan error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannedRepository {
    /// Canonical GitHub owner/name.
    pub repository: String,
    /// Default branch from gh-manager discovery.
    pub default_branch: Option<String>,
    /// Parsed packages and registry evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inspection: Option<Inspection>,
    /// Recoverable package, repository, and release-check failures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Value>,
    /// Failure to read or parse this repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Organization/user scan with package and release evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    /// JSON contract version.
    pub schema_version: u32,
    /// Public GitHub organization or user login.
    #[serde(rename = "account")]
    pub github_owner: String,
    /// Discovered repositories, including scan errors.
    pub repositories: Vec<ScannedRepository>,
    /// Findings with registry identity or CI evidence.
    pub findings: Vec<Value>,
}

/// Retains temporary manifests until planning/setup ends; drops them on every exit.
pub struct RepositoryScan {
    /// Structured scan result.
    pub report: ScanReport,
    /// Owns the snapshot directories for the duration of setup.
    pub workspace: tempfile::TempDir,
}

/// Scan through the supplied GitHub gateway, then query registries independently.
#[allow(clippy::future_not_send)] // Injectable GitHub gateways need not implement Send.
pub async fn scan_repositories(
    github: &impl GithubGateway,
    org: Option<&str>,
    user: Option<&str>,
    offline: bool,
    client: &RegistryClient,
    verbose: bool,
) -> Result<RepositoryScan> {
    scan_repositories_with_progress(github, org, user, offline, client, verbose, |_, _, _, _| {
        Ok(())
    })
    .await
}

/// Scan and report each completed repository before starting the next one.
#[allow(clippy::future_not_send, clippy::too_many_arguments)]
pub async fn scan_repositories_with_progress(
    github: &impl GithubGateway,
    org: Option<&str>,
    user: Option<&str>,
    offline: bool,
    client: &RegistryClient,
    verbose: bool,
    progress: impl FnMut(&ScannedRepository, &[Value], usize, usize) -> Result<()>,
) -> Result<RepositoryScan> {
    scan_repositories_with_inactive(github, org, user, offline, client, verbose, false, progress)
        .await
}

/// Include archived and forked repositories when maintaining shared credentials.
/// Normal inspection keeps the existing active-repository scope.
#[allow(clippy::future_not_send, clippy::too_many_arguments)]
pub async fn scan_repositories_with_inactive(
    github: &impl GithubGateway,
    org: Option<&str>,
    user: Option<&str>,
    offline: bool,
    client: &RegistryClient,
    verbose: bool,
    include_inactive: bool,
    mut progress: impl FnMut(&ScannedRepository, &[Value], usize, usize) -> Result<()>,
) -> Result<RepositoryScan> {
    if org.is_some() == user.is_some() {
        bail!("choose exactly one --org or --user");
    }
    let owner = org.or(user).expect("one GitHub owner");
    if !regex::Regex::new(r"^[\w.-]+$")
        .expect("static pattern")
        .is_match(owner)
    {
        bail!("invalid account name");
    }
    let workspace = tempfile::tempdir()?;
    let scope = if org.is_some() { "--org" } else { "--user" };
    let mut args = vec!["repo".into(), "list".into(), scope.into(), owner.into()];
    if include_inactive {
        args.extend(["--include-archived".into(), "--include-forks".into()]);
    }
    let repos = github.call(&args, None, workspace.path()).await?;
    let repos = repos
        .as_array()
        .ok_or_else(|| anyhow!("gh-manager returned an invalid repository list"))?;
    let mut report = ScanReport {
        schema_version: 1,
        github_owner: owner.into(),
        repositories: Vec::new(),
        findings: Vec::new(),
    };
    let slug_pattern = regex::Regex::new(r"^[\w.-]+/[\w.-]+$").expect("static pattern");
    for (index, repo) in repos.iter().enumerate() {
        let slug = repo["full_name"].as_str().unwrap_or("<invalid repository>");
        let branch = repo["default_branch"].as_str();
        let mut errors = Vec::new();
        let finding_start = report.findings.len();
        let result: Result<Inspection> = async {
            if !slug_pattern.is_match(slug) {
                bail!("invalid repository name");
            }
            let mut args = vec![
                "repo".into(),
                "files".into(),
                slug.into(),
                "--content".into(),
            ];
            if let Some(branch) = branch {
                args.extend(["--branch".into(), branch.into()]);
            }
            for pattern in scan_matches() {
                args.extend(["--match".into(), pattern]);
            }
            let root = workspace.path().join(index.to_string());
            let mut files = github.call(&args, None, workspace.path()).await?;
            let lines: Vec<String> = files
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|file| {
                    file["content"]
                        .as_str()
                        .unwrap_or_default()
                        .lines()
                        .map(str::to_owned)
                })
                .collect();
            let scripts: Vec<String> = crate::publishers::script_references(&lines)
                .into_iter()
                .map(|reference| reference.trim_start_matches("./").to_owned())
                .filter(|reference| !reference.split('/').any(|part| part == ".."))
                .collect();
            if !scripts.is_empty() {
                let mut script_args = vec![
                    "repo".into(),
                    "files".into(),
                    slug.into(),
                    "--content".into(),
                ];
                if let Some(branch) = branch {
                    script_args.extend(["--branch".into(), branch.into()]);
                }
                for script in scripts {
                    script_args.extend(["--match".into(), format!("**/{script}")]);
                }
                let scripts = github.call(&script_args, None, workspace.path()).await?;
                files
                    .as_array_mut()
                    .ok_or_else(|| anyhow!("invalid repository file list"))?
                    .extend(
                        scripts
                            .as_array()
                            .ok_or_else(|| anyhow!("invalid repository script list"))?
                            .iter()
                            .cloned(),
                    );
            }
            snapshot(&root, slug, &files)?;
            let discovered = crate::discovery::inspect_repository_recovering(
                &root,
                InspectOptions {
                    include_skipped: verbose,
                },
                Some(&mut |manifest, message| {
                    errors.push(json!({"stage":"manifest","manifest":manifest,"message":message}));
                }),
            )?;
            Ok(if offline {
                discovered
            } else {
                client.probe_known_repository(&discovered).await
            })
        }
        .await;
        let inspection = match result {
            Ok(inspection) => {
                report.findings.extend(findings(&inspection));
                if !offline {
                    for package in &inspection.packages {
                        if package.publishable
                            && package.exists_on_registry.is_none()
                            && client.endpoints().state_url(package).is_some()
                        {
                            let mut error = json!({"stage":"registry","manifest":package.manifest,"package":package.name,"message":"registry lookup failed; package state unknown"});
                            if let Some(status) = package.registry_http_status {
                                error["http_status"] = json!(status);
                            }
                            errors.push(error);
                        }
                    }
                    release_health(
                        github,
                        slug,
                        branch,
                        &inspection,
                        workspace.path(),
                        &mut report.findings,
                        &mut errors,
                    )
                    .await;
                }
                Some(inspection)
            }
            Err(error) => {
                errors.push(json!({"stage":"repository","message":error.to_string()}));
                None
            }
        };
        let error = (!errors.is_empty()).then(|| {
            errors
                .iter()
                .map(|error| {
                    let message = error["message"].as_str().unwrap_or("scan failed");
                    error["manifest"].as_str().map_or_else(
                        || message.to_owned(),
                        |manifest| format!("{manifest}: {message}"),
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        });
        let item = ScannedRepository {
            repository: slug.into(),
            default_branch: branch.map(str::to_owned),
            inspection,
            error,
            errors,
        };
        progress(
            &item,
            &report.findings[finding_start..],
            index + 1,
            repos.len(),
        )?;
        report.repositories.push(item);
    }
    let mut inspections = report
        .repositories
        .iter_mut()
        .filter_map(|item| item.inspection.as_mut())
        .collect::<Vec<_>>();
    for conflict in crate::identity_conflicts::mark(&mut inspections) {
        report.findings.push(json!({"type":"package-identity-conflict","repository":conflict["sources"].as_array().unwrap_or(&Vec::new()).iter().filter_map(|s|s["repository"].as_str()).collect::<Vec<_>>().join(", "),"registry":conflict["identity"].as_str().unwrap_or_default().split(':').next(),"package":conflict["identity"].as_str().unwrap_or_default().split_once(':').map(|(_,name)|name),"evidence":conflict}));
    }
    Ok(RepositoryScan { report, workspace })
}

#[allow(clippy::too_many_arguments, clippy::future_not_send)]
async fn release_health(
    github: &impl GithubGateway,
    slug: &str,
    branch: Option<&str>,
    inspection: &Inspection,
    root: &Path,
    findings: &mut Vec<Value>,
    errors: &mut Vec<Value>,
) {
    let workflows: std::collections::BTreeSet<_> = inspection
        .repository
        .release_workflow
        .iter()
        .chain(
            inspection
                .packages
                .iter()
                .flat_map(|package| package.workflow.iter().chain(&package.workflow_candidates)),
        )
        .collect();
    let expired = regex::Regex::new(r"GitHub log download (404|410)").expect("static pattern");
    for workflow in workflows {
        let result: Result<()> = async {
            let branch = branch.ok_or_else(|| anyhow!("repository has no default branch for release inspection"))?;
            let runs = github.release_runs(slug, workflow, branch, root).await?;
            let mut eligible: Vec<_> = runs.as_array().ok_or_else(|| anyhow!("invalid release runs list"))?.iter()
                .filter(|run| run["head_branch"] == branch && matches!(run["event"].as_str(), Some("push" | "workflow_dispatch")))
                .collect();
            eligible.sort_by(|left, right| right["created_at"].as_str().cmp(&left["created_at"].as_str()).then_with(|| right["id"].as_u64().cmp(&left["id"].as_u64())));
            let Some(run) = eligible.first().filter(|run| run["conclusion"] == "failure") else { return Ok(()); };
            let mut evidence = json!({"repository":slug,"workflow":workflow,"runId":run["id"],"runUrl":run["html_url"],"conclusion":run["conclusion"]});
            findings.push(json!({"repository":slug,"type":"release-failing","evidence":evidence}));
            let mut args = vec!["runs".into(), "logs".into(), run["id"].to_string(), "--repo".into(), slug.into()];
            for pattern in failure_patterns(None) { args.extend(["--grep".into(), pattern]); }
            evidence["matches"] = match github.call(&args, None, root).await {
                Ok(matches) => matches,
                Err(error) if expired.is_match(&error.to_string()) => json!({"status":"logs-expired","matches":[]}),
                Err(error) => return Err(error),
            };
            findings.last_mut().expect("release finding")["evidence"] = evidence;
            Ok(())
        }.await;
        if let Err(error) = result {
            errors.push(json!({"stage":"release","workflow":workflow,"message":error.to_string()}));
        }
    }
}

impl ScanReport {
    /// Remove temporary local paths before printing machine-readable inspection.
    pub fn public_paths(&mut self) {
        for repo in &mut self.repositories {
            if let Some(inspection) = &mut repo.inspection {
                inspection.repository.root = format!("https://github.com/{}", repo.repository);
            }
        }
    }
    /// Human-readable table of findings and errors.
    #[must_use]
    pub fn render(&self) -> String {
        let mut output = "Repository\tRegistry\tPackage\tFinding\n".to_owned();
        for finding in &self.findings {
            let _ = writeln!(
                output,
                "{}\t{}\t{}\t{}{}",
                finding["repository"].as_str().unwrap_or("-"),
                finding["registry"].as_str().unwrap_or("-"),
                finding["package"].as_str().unwrap_or("-"),
                finding["type"].as_str().unwrap_or("-").replace('-', " "),
                finding["evidence"]["expires_at"].as_str().map_or_else(
                    String::new,
                    |expiry| format!(
                        ": {} expires {expiry}",
                        finding["evidence"]["name"].as_str().unwrap_or("token")
                    )
                )
            );
        }
        for repo in &self.repositories {
            if let Some(error) = &repo.error {
                let _ = writeln!(output, "{}\t-\t-\tscan failed: {error}", repo.repository);
            }
            if let Some(inspection) = &repo.inspection {
                for package in &inspection.packages {
                    if !package.publishable
                        || self.findings.iter().any(|finding| {
                            finding["repository"] == repo.repository
                                && finding["package"] == package.name
                                && finding["registry"] == package.registry.to_string()
                        })
                    {
                        continue;
                    }
                    let status = if package.exists_on_registry.is_none() {
                        "registry state unknown"
                    } else if package.publishing_policy.as_deref()
                        == Some("oidc-with-token-fallback")
                    {
                        "trusted publishing + token fallback"
                    } else if package.trusted_publishing == Some(true) {
                        "published with trusted publishing"
                    } else {
                        "published; trusted publishing unknown"
                    };
                    let _ = writeln!(
                        output,
                        "{}\t{}\t{}\t{status}",
                        repo.repository, package.registry, package.name
                    );
                }
            }
        }
        output
    }
}
