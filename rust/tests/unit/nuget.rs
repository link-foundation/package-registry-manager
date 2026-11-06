use anyhow::Result;
use package_registry_manager::nuget_policy::{self, Login, PublishAdapter};
use serde_json::{json, Value};

fn policy() -> Value {
    json!({"username":"profile","package_owner":"Org","organization":"acme","repository":"libraries","owner_id":"12","repository_id":"34","workflow":"ci.yml","package_ids":["Org.Lib","Org.Tool"],"operations":["package:pushversion"]})
}

#[test]
fn policy_is_exact_idempotent_and_never_treats_unknown_activation_as_verified() {
    let expected = policy();
    let mut active = expected.clone();
    active["activation"] = json!("active");
    assert_eq!(
        nuget_policy::state(&[active.clone()], &expected, "profile").unwrap()["verified"],
        true
    );
    assert_eq!(
        nuget_policy::state(&[active.clone()], &expected, "other").unwrap()["status"],
        "wrong-account"
    );
    assert_eq!(
        nuget_policy::state(&[active.clone(), active.clone()], &expected, "profile").unwrap()
            ["status"],
        "duplicate"
    );
    for (field, value) in [
        ("workflow", json!("CI.yml")),
        ("environment", json!("production")),
        ("package_owner", json!("profile")),
        ("package_ids", json!(["Org.Lib"])),
        ("activation", json!("temporarily-active")),
        ("repository_id", json!("35")),
    ] {
        let mut changed = active.clone();
        changed[field] = value;
        assert_eq!(
            nuget_policy::state(&[changed], &expected, "profile").unwrap()["verified"],
            false,
            "{field}"
        );
    }
    let mut invalid = expected.clone();
    invalid["package_ids"] = json!(["MyPackage"]);
    assert!(nuget_policy::validate(&invalid).is_err());
    invalid = expected;
    invalid["operations"] = json!(["package:push", "package:pushversion"]);
    assert!(nuget_policy::validate(&invalid).is_err());
    let evidence =
        nuget_policy::verification_evidence(400, "package:verify secret", &["secret"], false);
    assert_eq!(evidence["authorization"], "unknown");
    assert!(!evidence.to_string().contains(" secret"));
    assert!(
        nuget_policy::verification_evidence(200, "ok", &[], true)["reason"]
            .as_str()
            .unwrap()
            .contains("original policy package scope is unproven")
    );
}

struct Adapter {
    username: bool,
    oidc: bool,
    fail: &'static str,
    probe: Option<bool>,
    calls: Vec<String>,
}

#[test]
fn oldest_identity_match_shadows_new_package_scopes_until_repaired() {
    let mut expected = policy();
    expected["package_ids"] = json!(["links-notation"]);
    expected["operations"] = json!(["package:push"]);
    let mut older = policy();
    older["id"] = json!(24222);
    older["evaluation_order"] = json!(0);
    older["activation"] = json!("active");
    let mut newer = expected.clone();
    newer["id"] = json!(24255);
    newer["evaluation_order"] = json!(1);
    newer["activation"] = json!("active");
    let mut unrelated = older.clone();
    unrelated["repository"] = json!("another-repository");
    unrelated["evaluation_order"] = json!(2);
    let state = nuget_policy::state(
        &[newer.clone(), unrelated.clone(), older.clone()],
        &expected,
        "profile",
    )
    .unwrap();
    assert_eq!(state["status"], "shadowed");
    assert_eq!(state["verified"], false);
    assert_eq!(state["policy_id"], 24222);
    assert_eq!(state["shadowed_policy_ids"], json!([24255]));
    let mut repaired = older.clone();
    repaired["package_ids"] = expected["package_ids"].clone();
    repaired["operations"] = expected["operations"].clone();
    assert_eq!(
        nuget_policy::state(&[repaired, newer.clone(), unrelated], &expected, "profile").unwrap()
            ["verified"],
        true
    );
    older["package_owner"] = json!("another-owner");
    assert_eq!(
        nuget_policy::state(&[older.clone(), newer.clone()], &expected, "profile").unwrap()
            ["status"],
        "owner-conflict"
    );
    older["activation"] = json!("inactive");
    assert_eq!(
        nuget_policy::state(&[older, newer], &expected, "profile").unwrap()["policy_id"],
        24255
    );
}
// Immediate mock responses implement the production async interface.
#[allow(clippy::unused_async_trait_impl)]
impl PublishAdapter for Adapter {
    fn username(&self) -> Option<&str> {
        self.username.then_some("profile")
    }
    fn fallback(&self) -> Option<String> {
        Some("provided".into())
    }
    async fn login(&mut self) -> Result<Login> {
        Ok(Login {
            key: self.oidc.then_some("temporary".into()),
            status: "exchange-result".into(),
            reason: None,
        })
    }
    async fn fingerprint(&mut self, _: &str) -> Result<String> {
        Ok("same-sha".into())
    }
    async fn push(&mut self, artifact: &str, key: &str) -> Result<Value> {
        assert_eq!(artifact, "same.nupkg");
        self.calls.push(key.into());
        Ok(json!({"status":if key == "temporary" { self.fail } else { "accepted" }}))
    }
    async fn exact_version_exists(&mut self, _: &str) -> Result<Option<bool>> {
        Ok(self.probe)
    }
}
#[tokio::test]
async fn authentication_order_and_bounded_same_artifact_fallback_match_javascript() {
    let mut adapter = Adapter {
        username: true,
        oidc: true,
        fail: "accepted",
        probe: Some(false),
        calls: Vec::new(),
    };
    assert_eq!(
        nuget_policy::publish("same.nupkg", &mut adapter)
            .await
            .unwrap()["selected_method"],
        "trusted-publishing"
    );
    assert_eq!(adapter.calls, ["temporary"]);
    adapter.calls.clear();
    adapter.fail = "authentication-failed";
    let result = nuget_policy::publish("same.nupkg", &mut adapter)
        .await
        .unwrap();
    assert_eq!(result["selected_method"], "token-fallback");
    assert_eq!(result["trust_repaired"], false);
    assert_eq!(adapter.calls, ["temporary", "provided"]);
    adapter.calls.clear();
    adapter.username = false;
    assert_eq!(
        nuget_policy::publish("same.nupkg", &mut adapter)
            .await
            .unwrap()["selected_method"],
        "token-fallback"
    );
    assert_eq!(adapter.calls, ["provided"]);
    for failure in [
        "transport-failed",
        "build-failed",
        "verification-unsupported",
    ] {
        adapter.calls.clear();
        adapter.username = true;
        adapter.fail = failure;
        assert_eq!(
            nuget_policy::publish("same.nupkg", &mut adapter)
                .await
                .unwrap()["publication"],
            "unverified"
        );
        assert_eq!(adapter.calls.len(), 1);
    }
    adapter.calls.clear();
    adapter.fail = "authentication-failed";
    adapter.probe = None;
    assert_eq!(
        nuget_policy::publish("same.nupkg", &mut adapter)
            .await
            .unwrap()["publication"],
        "unknown"
    );
    assert_eq!(adapter.calls.len(), 1);
}

#[test]
fn policy_and_controller_assets_are_identical_in_both_ports() {
    assert_eq!(
        include_str!("../../src/nuget-page.mjs"),
        include_str!("../../../js/src/nuget-page.mjs")
    );
    assert_eq!(
        include_str!("../../src/nuget-publish.py"),
        include_str!("../../../js/src/nuget-publish.py")
    );
}

#[test]
fn public_nuget_identity_ignores_build_metadata_and_zero_revision() {
    use package_registry_manager::publication_state::normalized_nuget_version;
    assert_eq!(
        normalized_nuget_version("01.2.003.0+build.9").as_deref(),
        Some("1.2.3")
    );
    assert_eq!(normalized_nuget_version("1.0").as_deref(), Some("1.0.0"));
    assert_eq!(
        normalized_nuget_version("1.2.3.4-Preview.2").as_deref(),
        Some("1.2.3.4-preview.2")
    );
    for invalid in ["unknown", "1.2.3-", "1.2.3-?", "1.2.3-a..b", "1.2.3+"] {
        assert_eq!(normalized_nuget_version(invalid), None, "{invalid}");
    }
}

#[test]
fn nuget_uses_nested_publishing_callee_and_rejects_ambiguous_identity() {
    let chains = vec!["publish.yml -> publish-csharp.yml -> upload".into()];
    assert_eq!(
        nuget_policy::workflow_identity("js.yml", &chains, &[]).unwrap(),
        "publish-csharp.yml"
    );
    assert_eq!(
        nuget_policy::workflow_identity("js.yml", &[], &[]).unwrap(),
        "js.yml"
    );
    assert!(nuget_policy::workflow_identity(
        "js.yml",
        &["a.yml -> push".into(), "b.yml -> push".into()],
        &[]
    )
    .is_err());
    assert!(nuget_policy::workflow_identity(
        "js.yml",
        &chains,
        &["the publishing jobs use different environments".into()]
    )
    .is_err());
}
