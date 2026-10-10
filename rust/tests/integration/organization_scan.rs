use anyhow::Result;
use package_registry_manager::{
    github::GithubGateway,
    organization_scan::scan_account,
    registry_state::{Endpoints, RegistryClient},
};
use serde_json::{json, Value};
use std::path::Path;

struct FakeGithub;

#[tokio::test]
async fn account_registry_probes_keep_publisher_configuration_unknown() {
    let registry = super::mock_registry::MockRegistry::start(|path| {
        if path.contains("missing") {
            None
        } else {
            Some("{}".into())
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
                    {"path":"Cargo.toml","content":format!("[package]\nname='{name}'\nversion='1.0.0'\nrepository='https://github.com/old/project'\n")},
                    {"path":"tests/Cargo.toml","content":"[package]\nname='ignored'\nversion='1.0.0'\n"}
                ]))
            }
            ("runs", "failures") => {
                assert!(args.contains(&"No Trusted Publishing config found".into()));
                Ok(
                    json!([{"repository":"team/failing","runUrl":"https://github.com/team/failing/actions/runs/1","matches":[{"lineNumber":3,"line":"No Trusted Publishing config found"}]}]),
                )
            }
            _ => panic!("unexpected GitHub call: {args:?}"),
        }
    }
}

#[tokio::test]
async fn account_scan_reads_matched_files_and_reports_all_four_findings() {
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
    let scan = scan_account(&FakeGithub, Some("team"), None, false, &client, false)
        .await
        .unwrap();
    assert_eq!(scan.report.repositories.len(), 4);
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
            "trusted-publisher-other-repository",
            "release-failing"
        ])
    );
    assert!(scan.report.render().contains("team/failing"));
    let root = scan.workspace.path().to_owned();
    drop(scan);
    assert!(!root.exists());
}
