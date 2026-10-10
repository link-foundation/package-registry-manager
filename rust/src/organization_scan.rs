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
        if package.exists_on_registry == Some(false) {
            push("unpublished", Value::Null);
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
#[derive(Debug, Serialize, Deserialize)]
pub struct ScannedRepository {
    /// Canonical GitHub owner/name.
    pub repository: String,
    /// Default branch from gh-manager discovery.
    pub default_branch: Option<String>,
    /// Parsed packages and registry evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inspection: Option<Inspection>,
    /// Failure to read or parse this repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Organization/user scan with package and release evidence.
#[derive(Debug, Serialize, Deserialize)]
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
    let repos = github
        .call(
            &["repo".into(), "list".into(), scope.into(), owner.into()],
            None,
            workspace.path(),
        )
        .await?;
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
        let slug = repo["full_name"]
            .as_str()
            .ok_or_else(|| anyhow!("repository name missing"))?;
        if !slug_pattern.is_match(slug) {
            bail!("invalid repository name");
        }
        let branch = repo["default_branch"].as_str();
        let result: Result<Inspection> = async {
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
            snapshot(
                &root,
                slug,
                &github.call(&args, None, workspace.path()).await?,
            )?;
            let discovered = crate::inspect_repository_with(
                &root,
                InspectOptions {
                    include_skipped: verbose,
                },
            )?;
            Ok(if offline {
                discovered
            } else {
                client.probe_known_repository(&discovered).await
            })
        }
        .await;
        let (inspection, error) = match result {
            Ok(inspection) => {
                report.findings.extend(findings(&inspection));
                (Some(inspection), None)
            }
            Err(error) => (None, Some(error.to_string())),
        };
        report.repositories.push(ScannedRepository {
            repository: slug.into(),
            default_branch: branch.map(str::to_owned),
            inspection,
            error,
        });
    }
    if !offline {
        if let Some(org) = org {
            let mut args = vec!["runs".into(), "failures".into(), "--org".into(), org.into()];
            for pattern in failure_patterns(None) {
                args.extend(["--grep".into(), pattern]);
            }
            let failures = github.call(&args, None, workspace.path()).await?;
            for failure in failures
                .as_array()
                .ok_or_else(|| anyhow!("invalid failures list"))?
            {
                report.findings.push(json!({"repository":failure["repository"],"type":"release-failing","evidence":failure}));
            }
        } else {
            for repo in repos {
                let slug = repo["full_name"].as_str().expect("validated repository");
                let mut args = vec!["runs".into(), "list".into(), slug.into()];
                if let Some(branch) = repo["default_branch"].as_str() {
                    args.extend(["--branch".into(), branch.into()]);
                }
                let runs = github.call(&args, None, workspace.path()).await?;
                if let Some(run) = runs
                    .as_array()
                    .and_then(|runs| runs.first())
                    .filter(|run| run["conclusion"] == "failure")
                {
                    let mut args = vec![
                        "runs".into(),
                        "logs".into(),
                        run["id"].to_string(),
                        "--repo".into(),
                        slug.into(),
                    ];
                    for pattern in failure_patterns(None) {
                        args.extend(["--grep".into(), pattern]);
                    }
                    let matches = github.call(&args, None, workspace.path()).await?;
                    if matches.as_array().is_some_and(|items| !items.is_empty()) {
                        report.findings.push(json!({"repository":slug,"type":"release-failing","evidence":{"runUrl":run["html_url"],"matches":matches}}));
                    }
                }
            }
        }
    }
    Ok(RepositoryScan { report, workspace })
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
                "{}\t{}\t{}\t{}",
                finding["repository"].as_str().unwrap_or("-"),
                finding["registry"].as_str().unwrap_or("-"),
                finding["package"].as_str().unwrap_or("-"),
                finding["type"].as_str().unwrap_or("-").replace('-', " ")
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
