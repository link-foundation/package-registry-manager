//! Mirrors js/test/scan-sidecars.test.mjs (#80): account scans fetch the
//! version sidecars local discovery reads, bounded and at one commit, so Go
//! and C++ keep 0.30.0; unreadable sidecars keep every peer package.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use package_registry_manager::github::GithubGateway;
use package_registry_manager::organization_scan::scan_repositories;
use package_registry_manager::registry_state::{Endpoints, RegistryClient};
use package_registry_manager::scan_sidecars::version_sidecars;
use package_registry_manager::{inspect_repository, Registry};
use regex::Regex;
use serde_json::{json, Value};

const SHA: &str = "5ce8287c880239ae4202c9071daa4429342b4a09";
const SLUG: &str = "link-foundation/links-notation";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/links-notation")
}

/// `**/name`, `dir/*.yml` and exact paths, as gh-manager matches them.
fn glob(pattern: &str) -> Regex {
    let mut expression = String::from("^");
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("**/") {
            expression.push_str("(?:.*/)?");
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix('*') {
            expression.push_str("[^/]*");
            rest = tail;
        } else {
            let next = rest.chars().next().expect("non-empty");
            expression.push_str(&regex::escape(&next.to_string()));
            rest = &rest[next.len_utf8()..];
        }
    }
    Regex::new(&format!("{expression}$")).expect("glob")
}

/// Serves the fixture tree; records every `--branch` and can deny sidecars.
#[derive(Default)]
struct FixtureGithub {
    deny_sidecars: bool,
    truncated: bool,
    branches: Mutex<Vec<String>>,
}

#[allow(clippy::unused_async_trait_impl)]
impl GithubGateway for FixtureGithub {
    async fn call(&self, args: &[String], _: Option<&str>, _: &Path) -> Result<Value> {
        if args[1] == "list" {
            return Ok(json!([{"full_name": SLUG, "default_branch": "main"}]));
        }
        assert_eq!(args[..2], ["repo", "files"]);
        let value_of = |flag: &str| -> Vec<String> {
            args.windows(2)
                .filter(|pair| pair[0] == flag)
                .map(|pair| pair[1].clone())
                .collect()
        };
        self.branches
            .lock()
            .expect("lock")
            .extend(value_of("--branch"));
        let patterns: Vec<Regex> = value_of("--match").iter().map(|p| glob(p)).collect();
        let sidecars = value_of("--match")
            .iter()
            .all(|pattern| !pattern.contains('*'));
        if !sidecars && self.truncated {
            anyhow::bail!("GitHub truncated a nonrecursive tree.");
        }
        if sidecars && self.deny_sidecars {
            anyhow::bail!("HTTP 403 Resource not accessible");
        }
        let root = root();
        let files: Vec<Value> = walkdir::WalkDir::new(&root)
            .into_iter()
            .flatten()
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| {
                let path = entry
                    .path()
                    .strip_prefix(&root)
                    .ok()?
                    .to_string_lossy()
                    .replace('\\', "/");
                (path != "expected-packages.json"
                    && patterns.iter().any(|pattern| pattern.is_match(&path)))
                .then(|| {
                    json!({"path": path, "content": std::fs::read_to_string(entry.path()).unwrap_or_default()})
                })
            })
            .collect();
        Ok(json!(files))
    }

    async fn head_commit(&self, slug: &str, branch: &str, _: &Path) -> Option<String> {
        assert_eq!((slug, branch), (SLUG, "main"));
        Some(SHA.into())
    }
}

#[tokio::test]
async fn account_scan_and_local_inspection_agree_on_versions_at_one_commit() {
    let github = FixtureGithub::default();
    let client = RegistryClient::new(Endpoints::default(), false);
    let scan = scan_repositories(&github, Some("link-foundation"), None, true, &client, false)
        .await
        .expect("scan");
    let repository = &scan.report.repositories[0];
    assert!(repository.error.is_none(), "{:?}", repository.error);
    assert_eq!(repository.commit.as_deref(), Some(SHA));
    let branches = github.branches.lock().expect("lock").clone();
    assert!(!branches.is_empty() && branches.iter().all(|branch| branch == SHA));
    let sidecars = repository.version_sidecars.as_ref().expect("sidecars");
    assert!(sidecars.requested.iter().any(|path| path == "go/VERSION"));
    assert!(sidecars.requested.iter().any(|path| path == "cpp/VERSION"));

    let identity = |packages: &[package_registry_manager::Package]| {
        packages
            .iter()
            .map(|item| {
                (
                    item.registry,
                    item.name.clone(),
                    item.manifest.clone(),
                    item.version.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    let scanned = &repository.inspection.as_ref().expect("inspection").packages;
    let local = inspect_repository(&root()).expect("local").packages;
    assert_eq!(identity(scanned), identity(&local));
    let go = scanned
        .iter()
        .find(|item| item.registry == Registry::GoModules)
        .expect("go");
    assert_eq!(go.version.as_deref(), Some("v0.30.0"));
    let cpp = scanned
        .iter()
        .find(|item| item.manifest == "cpp/CMakeLists.txt")
        .expect("cpp");
    assert_eq!(cpp.version.as_deref(), Some("0.30.0"));
    let swift = scanned
        .iter()
        .find(|item| item.registry == Registry::Swiftpm)
        .expect("swift");
    assert_eq!(swift.version.as_deref(), Some("0.30.0"));
}

#[tokio::test]
async fn unreadable_sidecars_keep_peers_and_leave_versions_unknown_with_an_error() {
    let github = FixtureGithub {
        deny_sidecars: true,
        ..FixtureGithub::default()
    };
    let client = RegistryClient::new(Endpoints::default(), false);
    let scan = scan_repositories(&github, Some("link-foundation"), None, true, &client, false)
        .await
        .expect("scan");
    let repository = &scan.report.repositories[0];
    assert!(
        repository.errors.iter().any(|error| error["message"]
            .as_str()
            .is_some_and(|message| message.contains("version files unavailable (HTTP 403"))),
        "{:?}",
        repository.errors
    );
    let scanned = &repository.inspection.as_ref().expect("inspection").packages;
    let go = scanned
        .iter()
        .find(|item| item.registry == Registry::GoModules)
        .expect("go");
    assert_eq!(go.version, None);
    let local = inspect_repository(&root()).expect("local").packages;
    assert_eq!(scanned.len(), local.len());
}

#[test]
fn an_absent_exact_version_is_reported_even_when_older_versions_exist() {
    let mut inspection = inspect_repository(&root()).expect("local");
    inspection.repository.github_owner = Some("link-foundation".into());
    inspection.repository.github_repository = Some("links-notation".into());
    inspection
        .packages
        .retain(|item| item.registry == Registry::GoModules);
    let go = &mut inspection.packages[0];
    go.exists_on_registry = Some(true);
    go.audit.insert("latest_version".into(), json!("v0.29.0"));
    go.audit
        .insert("manifest_version_exists".into(), json!(false));
    let found: Vec<(Value, Value)> =
        package_registry_manager::organization_scan::findings(&inspection)
            .into_iter()
            .map(|finding| {
                (
                    finding["type"].clone(),
                    finding["evidence"]["expected_version"].clone(),
                )
            })
            .collect();
    assert_eq!(
        found,
        [(json!("manifest-version-absent"), json!("v0.30.0"))]
    );
}

#[tokio::test]
async fn truncated_trees_surface_as_a_repository_error_instead_of_a_silent_empty_scan() {
    let github = FixtureGithub {
        truncated: true,
        ..FixtureGithub::default()
    };
    let client = RegistryClient::new(Endpoints::default(), false);
    let scan = scan_repositories(&github, Some("link-foundation"), None, true, &client, false)
        .await
        .expect("scan");
    let error = scan.report.repositories[0]
        .error
        .clone()
        .unwrap_or_default();
    assert!(error.contains("truncated"), "{error:?}");
}

#[test]
fn unsafe_and_escaping_version_references_are_rejected_not_followed() {
    let selection = version_sidecars(&[
        json!({"path": "cpp/CMakeLists.txt", "content": "file(READ \"${CMAKE_CURRENT_SOURCE_DIR}/../../etc/VERSION\" V)\nfile(READ \"${CMAKE_CURRENT_SOURCE_DIR}/version.txt\" W)"}),
        json!({"path": ".github/workflows/r.yml", "content": "run: cat ../VERSION && cat go/VERSION"}),
        json!({"path": "go/go.mod", "content": "module x"}),
    ]);
    assert_eq!(
        selection.paths,
        ["cpp/VERSION", "cpp/version.txt", "go/VERSION"]
    );
    assert_eq!(selection.rejected, ["../VERSION", "../etc/VERSION"]);
}
