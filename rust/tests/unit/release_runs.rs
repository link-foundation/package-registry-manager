use package_registry_manager::npm_package::PUBLISH_REJECTED;
use package_registry_manager::release_runs::{eligible_release_runs, failed_publishing_job};
use regex::Regex;
use serde_json::{json, Value};

#[test]
fn release_history_excludes_prs_tags_and_other_branches_and_sorts_events() {
    let mut runs: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/repository-transfer/release-runs.json"
    ))
    .unwrap();
    runs.push(json!({"databaseId":100, "headBranch":"v1.0.0", "event":"push"}));
    runs.push(json!({"databaseId":101, "headBranch":"main", "event":"schedule"}));
    runs.push(runs[3].clone());
    runs.reverse();
    let eligible = eligible_release_runs(runs, "main");
    assert_eq!(
        eligible
            .iter()
            .map(|run| run["databaseId"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [98, 42, 41]
    );
    assert_eq!(
        eligible_release_runs(eligible, "trunk"),
        Vec::<Value>::new()
    );
}

#[test]
fn failed_jobs_require_publish_or_preflight_failure() {
    for name in [
        "release-preflight",
        "Publish npm",
        "publish-crates",
        "PyPI publishing",
    ] {
        assert!(failed_publishing_job(
            &json!([{ "name": name, "conclusion": "failure" }])
        ));
        assert!(!failed_publishing_job(
            &json!([{ "name": name, "conclusion": "success" }, {"name":"lint", "conclusion":"failure"}])
        ));
    }
    assert!(failed_publishing_job(
        &json!([{ "name": "release", "conclusion": "failure", "steps": [{"name":"Run cargo publish", "conclusion":"failure"}] }])
    ));
    assert!(!failed_publishing_job(
        &json!([{ "name": "release", "conclusion": "failure", "steps": [{"name":"Publish", "conclusion":"success"}, {"name":"Lint", "conclusion":"failure"}] }])
    ));
}

#[test]
fn registry_oidc_rejections_include_preflight_wording() {
    let rejected = Regex::new(PUBLISH_REJECTED).unwrap();
    for log in [
        "npm error code E404", "npm error 404 Not Found", "invalid-publisher",
        "npm refused the OIDC package exchange (404) -- the trusted publisher on npmjs.com does not match",
        "crates.io refused the OIDC token exchange (403)",
        "PyPI refused the OIDC exchange (401)", "invalid_publisher", "unregistered-publisher",
    ] {
        assert!(rejected.is_match(log), "{log}");
    }
    for log in [
        "lint failed",
        "missing changeset",
        "npm refused the OIDC package exchange (500)",
    ] {
        assert!(!rejected.is_match(log), "{log}");
    }
}
