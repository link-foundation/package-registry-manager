use package_registry_manager::organization_scan::{findings, snapshot};
use package_registry_manager::{inspect_repository, Registry};
use serde_json::json;

#[test]
fn api_snapshots_detect_each_package_finding_and_ignore_fixtures() {
    let directory = tempfile::tempdir().unwrap();
    snapshot(
        directory.path(),
        "team/new",
        &json!([
            {"path":"package.json","content":"{\"name\":\"demo\",\"version\":\"1.0.0\"}"},
            {"path":"tests/package.json","content":"{\"name\":\"ignored\"}"}
        ]),
    )
    .unwrap();
    let mut inspection = inspect_repository(directory.path()).unwrap();
    assert_eq!(inspection.packages.len(), 1);
    assert_eq!(inspection.packages[0].registry, Registry::Npm);
    inspection.packages[0].exists_on_registry = Some(false);
    assert_eq!(findings(&inspection)[0]["type"], "unpublished");
    inspection.packages[0].exists_on_registry = Some(true);
    inspection.packages[0].trusted_publishing = Some(false);
    assert_eq!(
        findings(&inspection)[0]["type"],
        "published-without-trusted-publishing"
    );
    inspection.packages[0].configured_publishers =
        Some(vec![package_registry_manager::model::PublisherIdentity {
            id: None,
            repository: "old/project".into(),
            workflow: Some("release.yml".into()),
            environment: None,
        }]);
    assert!(findings(&inspection)
        .iter()
        .any(|item| item["type"] == "trusted-publisher-other-repository"));
}

#[test]
fn unknown_state_is_not_unpublished_and_path_traversal_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    snapshot(
        directory.path(),
        "user/project",
        &json!([{ "path":"package.json", "content":"{\"name\":\"demo\"}" }]),
    )
    .unwrap();
    assert_eq!(
        findings(&inspect_repository(directory.path()).unwrap()),
        Vec::<serde_json::Value>::new()
    );
    assert!(snapshot(
        directory.path(),
        "user/project",
        &json!([{ "path":"../outside", "content":"unsafe" }])
    )
    .is_err());
}
