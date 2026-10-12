use package_registry_manager::{build_plans_with, inspect_repository, PlanOptions, Registry};
use std::{collections::BTreeSet, path::Path};

#[test]
fn scripted_cargo_variable_and_reusable_caller() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let inspection = inspect_repository(&root).unwrap();
    assert_eq!(inspection.packages[0].workflow.as_deref(), Some("js.yml"));
    assert_eq!(inspection.packages[0].workflow_jobs, ["rust-stage"]);
}

#[test]
fn explicit_token_rotation_keeps_default_oidc_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let inspection = inspect_repository(&root).unwrap();
    let plan = build_plans_with(
        &inspection,
        &BTreeSet::from([Registry::CratesIo]),
        &PlanOptions {
            rotate_token: Some("CARGO_TOKEN".into()),
            ..PlanOptions::default()
        },
    );
    assert_eq!(plan[0].steps[0].id, "rotate-crates-token");
    assert_eq!(plan[0].credential_policy.mode, "token");
    assert_eq!(
        package_registry_manager::credential_cycle::credential_policy(Registry::CratesIo).mode,
        "trusted"
    );
}

#[test]
fn private_inventory_matching_scopes_and_expiry_boundaries() {
    use package_registry_manager::crates_tokens::{
        replacement_token_request, select_crates_token, token_expiry_findings, token_inventory,
    };
    use serde_json::json;
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let token = json!({"id":10,"name":"CARGO_TOKEN","created_at":"2026-10-01T00:00:00Z","expired_at":"2026-10-08T00:00:00Z","crate_scopes":["start-command"],"endpoint_scopes":null,"token":"must-strip"});
    let tokens = token_inventory(&json!({"status":200,"body":{"api_tokens":[token]}})).unwrap();
    assert!(tokens[0].get("token").is_none());
    let metadata = json!({"updated_at":"2026-10-01T00:00:02Z"});
    assert_eq!(
        select_crates_token(&tokens, "CARGO_TOKEN", &metadata, None).unwrap()["id"],
        10
    );
    assert!(select_crates_token(
        &[tokens[0].clone(), tokens[0].clone()],
        "CARGO_TOKEN",
        &metadata,
        None
    )
    .is_err());
    let replacement = replacement_token_request(&tokens[0], 30, now).unwrap();
    assert_eq!(
        replacement["api_token"]["crate_scopes"],
        json!(["start-command"])
    );
    assert!(replacement["api_token"]["endpoint_scopes"].is_null());
    assert!(replacement_token_request(&json!({"crate_scopes":null}), 30, now).is_err());
    assert_eq!(
        token_expiry_findings(&tokens, "acme", 6, now).unwrap(),
        Vec::<serde_json::Value>::new()
    );
    assert_eq!(
        token_expiry_findings(&tokens, "acme", 7, now).unwrap()[0]["type"],
        "registry-token-expiring"
    );
    assert_eq!(
        token_expiry_findings(&tokens, "acme", 7, now + chrono::Duration::days(7)).unwrap()[0]
            ["type"],
        "registry-token-expired"
    );
}
