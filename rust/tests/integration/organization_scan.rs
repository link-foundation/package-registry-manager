use anyhow::Result;
use package_registry_manager::{
    github::GithubGateway,
    organization_scan::{scan_repositories, scan_repositories_with_progress},
    registry_state::{Endpoints, RegistryClient},
};
use serde_json::{json, Value};
use std::path::Path;

struct FakeGithub;

struct ScriptedGithub;

struct InactiveGithub;
#[allow(clippy::unused_async_trait_impl)]
impl GithubGateway for InactiveGithub {
    async fn call(&self, args: &[String], _: Option<&str>, _: &Path) -> Result<Value> {
        assert!(args.iter().any(|arg| arg == "--include-archived"));
        assert!(args.iter().any(|arg| arg == "--include-forks"));
        Ok(json!([]))
    }
}

#[tokio::test]
async fn shared_credential_maintenance_includes_archived_and_forked_repositories() {
    let client = RegistryClient::new(Endpoints::default(), false);
    package_registry_manager::organization_scan::scan_repositories_with_inactive(
        &InactiveGithub,
        Some("team"),
        None,
        true,
        &client,
        false,
        true,
        |_, _, _, _| Ok(()),
    )
    .await
    .unwrap();
}

#[allow(clippy::unused_async_trait_impl)]
impl GithubGateway for ScriptedGithub {
    async fn call(&self, args: &[String], _: Option<&str>, _: &Path) -> Result<Value> {
        if args[1] == "list" {
            return Ok(json!([{"full_name":"team/start","default_branch":"main"}]));
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
        let scripted = args
            .windows(2)
            .any(|pair| pair == ["--match", "**/scripts/publish-to-crates.mjs"]);
        let paths = if scripted {
            vec!["scripts/publish-to-crates.mjs"]
        } else {
            vec![
                "Cargo.toml",
                ".github/workflows/js.yml",
                ".github/workflows/rust.yml",
            ]
        };
        Ok(json!(paths.into_iter().map(|path| json!({"path":path,"content":std::fs::read_to_string(root.join(path)).unwrap()})).collect::<Vec<_>>()))
    }
}

#[tokio::test]
async fn account_snapshot_follows_referenced_script_to_reusable_caller() {
    let client = RegistryClient::new(Endpoints::default(), false);
    let scan = scan_repositories(&ScriptedGithub, Some("team"), None, true, &client, false)
        .await
        .unwrap();
    let repo = &scan.report.repositories[0];
    assert!(repo.error.is_none(), "{:?}", repo.error);
    assert_eq!(
        repo.inspection.as_ref().unwrap().packages[0]
            .workflow
            .as_deref(),
        Some("js.yml")
    );
}

struct ExpiredGithub;

#[allow(clippy::unused_async_trait_impl)]
impl GithubGateway for ExpiredGithub {
    async fn call(&self, args: &[String], _: Option<&str>, _: &Path) -> Result<Value> {
        match (args[0].as_str(), args[1].as_str()) {
            ("repo", "list") => Ok(json!(["expired", "healthy", "broken", "last"].map(
                |name| json!({"full_name":format!("team/{name}"),"default_branch":"main"})
            ))),
            ("repo", "files") => {
                if args[2] == "team/broken" {
                    anyhow::bail!("files forbidden");
                }
                Ok(json!([
                    {"path":"Cargo.toml","content":"[package]\nname='demo'\nversion='1.0.0'"},
                    {"path":"sub/package.json","content":"invalid ["},
                    {"path":".github/workflows/release.yml","content":"jobs:\n  publish:\n    permissions:\n      id-token: write\n    steps:\n      - run: cargo publish"}
                ]))
            }
            ("runs", "list") => {
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["--workflow", "release.yml"]));
                Ok(json!([
                    {"id":99,"head_branch":"feature","event":"pull_request","conclusion":"failure"},
                    {"id":3,"head_branch":"main","event":"push","created_at":"2026-10-11","conclusion":if args[2] == "team/expired" {"failure"} else {"success"},"html_url":format!("https://github.com/{}/actions/runs/3",args[2])},
                    {"id":1,"head_branch":"main","event":"push","created_at":"2026-10-10","conclusion":"failure"}
                ]))
            }
            ("runs", "logs") => {
                assert_eq!(args[2], "3");
                assert!(args.contains(&"team/expired".into()));
                anyhow::bail!("GitHub log download 410");
            }
            _ => panic!("unexpected GitHub call: {args:?}"),
        }
    }
}

#[tokio::test]
async fn expired_logs_and_invalid_packages_preserve_other_results() {
    let registry = super::mock_registry::MockRegistry::start(|_| None);
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_CRATES_IO_API", &registry.base),
        false,
    );
    for (org, user) in [(Some("team"), None), (None, Some("team"))] {
        let mut completed = Vec::new();
        let scan = scan_repositories_with_progress(
            &ExpiredGithub,
            org,
            user,
            false,
            &client,
            false,
            |item, _, index, total| {
                assert_eq!(index, completed.len() + 1);
                assert_eq!(total, 4);
                completed.push(item.repository.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(
            completed,
            ["team/expired", "team/healthy", "team/broken", "team/last"]
        );
        assert_eq!(scan.report.repositories.len(), 4);
        assert_eq!(
            scan.report.repositories[0]
                .inspection
                .as_ref()
                .unwrap()
                .packages
                .len(),
            1
        );
        let error = scan.report.repositories[0].error.as_ref().unwrap();
        assert!(!error.contains("410"), "{error}");
        assert!(error.contains("sub/package.json"), "{error}");
        assert!(scan.report.repositories[2]
            .error
            .as_ref()
            .unwrap()
            .contains("files forbidden"));
        assert_eq!(
            scan.report
                .findings
                .iter()
                .filter(|finding| finding["type"] == "unpublished")
                .count(),
            3
        );
        assert!(!scan.report.render().contains("410"));
    }
}

#[tokio::test]
async fn failed_registry_lookup_retains_successful_peer_findings() {
    let registry = super::mock_registry::MockRegistry::start(|path| {
        if path == "/crates/token" {
            Some("invalid JSON".into())
        } else {
            None
        }
    });
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_CRATES_IO_API", &registry.base),
        false,
    );
    let scan = scan_repositories(&FakeGithub, Some("team"), None, false, &client, false)
        .await
        .unwrap();
    let repo = &scan.report.repositories[1];
    assert_eq!(repo.inspection.as_ref().unwrap().packages.len(), 1);
    assert_eq!(
        repo.inspection.as_ref().unwrap().packages[0].exists_on_registry,
        None
    );
    assert_eq!(repo.errors[0]["stage"], "registry");
    assert_eq!(repo.errors[0]["package"], "token");
    assert_eq!(
        scan.report
            .findings
            .iter()
            .filter(|finding| finding["type"] == "unpublished")
            .count(),
        3
    );
}

#[tokio::test]
async fn account_registry_probes_keep_publisher_configuration_unknown() {
    let registry = super::mock_registry::MockRegistry::start(|path| {
        if path.contains("missing") {
            None
        } else {
            Some(r#"{"versions":["1.0.0"]}"#.into())
        }
    });
    let endpoints = Endpoints::default()
        .with("PACKAGE_REGISTRY_MANAGER_NUGET_API", &registry.base)
        .with("PACKAGE_REGISTRY_MANAGER_RUBYGEMS_API", &registry.base)
        .with("PACKAGE_REGISTRY_MANAGER_JSR_API", &registry.base);
    let client = RegistryClient::new(endpoints, false);
    for (kind, name, suffix) in [
        (
            package_registry_manager::Registry::NuGet,
            "Team.Library",
            "/team.library/index.json",
        ),
        (
            package_registry_manager::Registry::RubyGems,
            "team-tool",
            "/gems/team-tool.json",
        ),
        (
            package_registry_manager::Registry::Jsr,
            "@team/tool",
            "/@team/tool/meta.json",
        ),
    ] {
        let package =
            package_registry_manager::Package::new(kind, name.into(), None, "manifest".into());
        assert!(client
            .endpoints()
            .state_url(&package)
            .unwrap()
            .ends_with(suffix));
        let state = client.probe_package(&package).await;
        assert_eq!(state.exists, Some(true));
        assert_eq!(state.trusted, None);
        assert_eq!(
            client
                .probe_package(&package_registry_manager::Package::new(
                    kind,
                    "missing".into(),
                    None,
                    "manifest".into()
                ))
                .await
                .exists,
            Some(false)
        );
    }
}
#[tokio::test]
async fn nuget_registry_normalizes_exact_versions_and_rejects_invalid_metadata() {
    use package_registry_manager::{Package, Registry};
    for (body, exists) in [
        (r#"{"versions":["1.2.3"]}"#, Some(true)),
        (r#"{"versions":[]}"#, Some(false)),
        (r#"{"versions":["invalid"]}"#, None),
        (r#"{"versions":[null]}"#, None),
        ("{}", None),
    ] {
        let registry = super::mock_registry::MockRegistry::start(move |_| Some(body.into()));
        let client = RegistryClient::new(
            Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_NUGET_API", &registry.base),
            false,
        );
        let state = client
            .probe_package(&Package::new(
                Registry::NuGet,
                "Org.Lib".into(),
                Some("01.2.3.0+build.9".into()),
                "project.csproj".into(),
            ))
            .await;
        assert_eq!(state.exists, exists, "{body}");
        if exists == Some(true) {
            assert_eq!(state.audit["manifest_version_exists"], true);
        }
    }
}

#[allow(clippy::unused_async_trait_impl)] // Matches the asynchronous CLI transport contract.
impl GithubGateway for FakeGithub {
    async fn call(&self, args: &[String], input: Option<&str>, _: &Path) -> Result<Value> {
        assert!(input.is_none());
        match (args[0].as_str(), args[1].as_str()) {
            ("repo", "list") => {
                assert_eq!(&args[2..], ["--org", "team"]);
                Ok(json!(["missing", "token", "transferred", "failing"].map(
                    |name| json!({"full_name":format!("team/{name}"),"default_branch":"main"})
                )))
            }
            ("repo", "files") => {
                assert!(args
                    .windows(2)
                    .any(|pair| pair == ["--match", "**/Cargo.toml"]));
                assert!(args.windows(2).any(|pair| pair == ["--branch", "main"]));
                let name = args[2].split('/').nth(1).unwrap();
                Ok(json!([
                    {"path":".github/workflows/release.yml","content":"jobs:\n  publish:\n    permissions:\n      id-token: write\n    steps:\n      - run: cargo publish"},
                    {"path":"Cargo.toml","content":format!("[package]\nname='{name}'\nversion='1.0.0'\nrepository='https://github.com/old/project'\n")},
                    {"path":"tests/Cargo.toml","content":"[package]\nname='ignored'\nversion='1.0.0'\n"}
                ]))
            }
            ("runs", "list") => Ok(
                json!([{ "id":1,"head_branch":"main","event":"push","conclusion":if args[2] == "team/failing" {"failure"} else {"success"},"html_url":"https://github.com/team/failing/actions/runs/1" }]),
            ),
            ("runs", "logs") => {
                Ok(json!([{ "lineNumber":3,"line":"No Trusted Publishing config found" }]))
            }
            _ => panic!("unexpected GitHub call: {args:?}"),
        }
    }
}

#[tokio::test]
async fn repository_batch_reads_matched_files_and_reports_all_four_findings() {
    let registry = super::mock_registry::MockRegistry::start(|path| {
        if path == "/crates/missing" {
            return None;
        }
        if let Some(name) = path.strip_prefix("/crates/") {
            let publisher = if name == "transferred" {
                json!({"repository":"old/project"})
            } else {
                Value::Null
            };
            return Some(json!({"crate":{"max_version":"1.0.0"},"versions":[{"num":"1.0.0","trustpub_data":publisher}]}).to_string());
        }
        Some(
            if path.ends_with("crate=transferred") {
                json!({"github_configs":[{"repository":"old/project","workflow":"release.yml"}]})
            } else {
                json!({"github_configs":[]})
            }
            .to_string(),
        )
    });
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_CRATES_IO_API", &registry.base),
        false,
    );
    let scan = scan_repositories(&FakeGithub, Some("team"), None, false, &client, false)
        .await
        .unwrap();
    assert_eq!(scan.report.repositories.len(), 4);
    assert_eq!(
        serde_json::to_value(&scan.report).unwrap()["account"],
        "team"
    );
    for repo in &scan.report.repositories {
        let inspection = repo.inspection.as_ref().unwrap();
        assert_eq!(inspection.packages.len(), 1);
        assert_eq!(inspection.repository.github_owner.as_deref(), Some("team"));
    }
    let kinds: std::collections::BTreeSet<_> = scan
        .report
        .findings
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        std::collections::BTreeSet::from([
            "unpublished",
            "published-without-trusted-publishing",
            "release-failing",
            "manifest-version-unknown"
        ])
    );
    assert!(scan.report.render().contains("team/failing"));
    let root = scan.workspace.path().to_owned();
    drop(scan);
    assert!(!root.exists());
}
