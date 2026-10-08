use package_registry_manager::manifest_proposal::manifest_repository_proposal;
use package_registry_manager::repository_identity::{
    compare_repositories, inspect_manifest_repositories, manifest_urls, npm_publishers,
    provenance_repositories, publisher_identities, publisher_matches,
};
use package_registry_manager::{build_plans, Inspection};
use serde_json::{json, Value};

fn fixture() -> Inspection {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/repository-transfer/inspection.json"
    ))
    .unwrap()
}

fn evidence() -> Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/repository-transfer/evidence.json"
    ))
    .unwrap()
}

#[test]
fn offline_manifest_warns_even_when_last_release_was_trusted() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("package.json"),
        evidence()["manifest"].to_string(),
    )
    .unwrap();
    let mut inspection = fixture();
    inspection.repository.root = root.path().to_string_lossy().into_owned();
    inspection.packages[0].repository_mismatches.clear();
    inspect_manifest_repositories(&mut inspection);
    assert!(inspection.packages[0].warnings[0].contains("manifest still names konard"));
    assert_eq!(
        build_plans(&inspection)[0].mode,
        Some(package_registry_manager::PlanMode::Repair)
    );
}

#[test]
fn historical_sources_and_configurations_are_independent_findings() {
    use base64::Engine as _;
    let evidence = evidence();
    let payload =
        base64::engine::general_purpose::STANDARD.encode(evidence["npm_statement"].to_string());
    let npm = json!({"attestations":[{"bundle":{"dsseEnvelope":{"payload":payload}}}]});
    let old = "konard/disk-space-saviour";
    assert_eq!(provenance_repositories(&npm), [old]);
    assert_eq!(provenance_repositories(&evidence["pypi_provenance"]), [old]);
    let mut inspection = fixture();
    let package = &mut inspection.packages[0];
    package.provenance_repositories = provenance_repositories(&npm);
    package.configured_publishers = Some(publisher_identities(&evidence["publishers"][0]));
    compare_repositories(package, &inspection.repository, &[]);
    let sources: Vec<_> = package
        .repository_mismatches
        .iter()
        .map(|finding| finding.source.as_str())
        .collect();
    assert_eq!(sources, ["provenance", "trusted publisher"]);
}

#[test]
fn publisher_matching_requires_the_exact_workflow_and_environment() {
    let plan = build_plans(&fixture()).remove(0);
    let expected = plan.oidc_publisher.unwrap();
    let evidence = evidence();
    let output = evidence["publishers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|publisher| {
            format!(
                "type: github\nid: {}\nrepository: {}\nfile: {}\n",
                publisher["id"].as_str().unwrap(),
                publisher["repository"].as_str().unwrap(),
                publisher["file"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut publishers = npm_publishers(&output);
    assert_eq!(publishers.len(), 3);
    assert!(!publisher_matches(&publishers[0], &expected));
    assert!(publisher_matches(&publishers[1], &expected));
    assert!(!publisher_matches(&publishers[2], &expected));
    publishers[1].environment = Some("production".into());
    assert!(!publisher_matches(&publishers[1], &expected));
}

#[test]
fn unavailable_registry_settings_do_not_make_a_trusted_release_complete() {
    let mut inspection = fixture();
    inspection.packages[0].repository_mismatches.clear();
    inspection.packages[0].publisher_settings_verified = Some(false);
    assert_eq!(
        build_plans(&inspection)[0].mode,
        Some(package_registry_manager::PlanMode::Repair)
    );
}

#[test]
fn missing_packages_fix_metadata_before_bootstrap() {
    let mut inspection = fixture();
    inspection.packages[0].exists_on_registry = Some(false);
    let plan = build_plans(&inspection).remove(0);
    assert_eq!(plan.steps[0].id, "fix-manifest-repository");
    assert!(plan.steps.iter().any(|step| step.id == "first-publish"));
}

#[test]
fn manifest_proposals_preserve_unrelated_metadata() {
    let evidence = evidence();
    let current = "link-foundation/disk-space-saviour";
    let proposal =
        manifest_repository_proposal(&evidence["manifest"].to_string(), "package.json", current)
            .unwrap();
    let npm: Value = serde_json::from_str(&proposal).unwrap();
    assert_eq!(npm["repository"]["directory"], "js");
    assert_eq!(
        npm["repository"]["url"],
        format!("git+https://github.com/{current}.git")
    );
    for (file, text) in [
        ("Cargo.toml", "[package]\nname='tool'\nrepository='https://github.com/konard/disk-space-saviour'\n"),
        ("pyproject.toml", "[project.urls]\nSource='https://github.com/konard/disk-space-saviour'\nDocs='https://docs.test'\n"),
    ] {
        assert!(manifest_urls(text, file).iter().any(|url| url.contains("konard/disk-space-saviour")));
        let result = manifest_repository_proposal(text, file, current).unwrap();
        assert_eq!(result.replace(current, "konard/disk-space-saviour"), text);
    }
}

#[test]
fn transferred_repository_requires_ordered_repairs() {
    let inspection: Inspection = serde_json::from_str(include_str!(
        "../../../tests/fixtures/repository-transfer/inspection.json"
    ))
    .unwrap();
    let plans = build_plans(&inspection);
    let plan = &plans[0];
    assert_eq!(serde_json::to_value(plan.mode).unwrap(), "repair");
    let ids: Vec<_> = plan.steps.iter().map(|step| step.id.as_str()).collect();
    for id in [
        "attach-trusted-publisher",
        "verify-repository-publisher",
        "remove-old-publisher",
        "fix-manifest-repository",
        "rerun-release",
    ] {
        assert!(ids.contains(&id), "{id}");
    }
    let index = |id| ids.iter().position(|item| *item == id).unwrap();
    assert!(index("attach-trusted-publisher") < index("verify-repository-publisher"));
    assert!(index("verify-repository-publisher") < index("remove-old-publisher"));
    assert!(index("fix-manifest-repository") < index("rerun-release"));
    assert!(!ids.contains(&"first-publish"));
}
