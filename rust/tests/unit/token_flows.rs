use package_registry_manager::flows::{crates_flow, pypi_flow, FlowContext};
use package_registry_manager::registry_state::Endpoints;
use package_registry_manager::{Registry, SetupStep};

use crate::tokens::{crate_package, strings};

#[test]
fn requires_verified_oidc_before_unused_secret_deletion() {
    for registry in [
        Registry::Npm,
        Registry::PyPi,
        Registry::CratesIo,
        Registry::RubyGems,
        Registry::NuGet,
        Registry::Jsr,
    ] {
        let steps = package_registry_manager::tokens::token_secret_steps(
            &crate_package(registry),
            "acme/demo",
        );
        assert_eq!(
            ids(&steps),
            [
                "audit-token-secrets",
                "confirm-oidc-cleanup",
                "delete-token-secret"
            ]
        );
        assert_eq!(steps[1].kind, package_registry_manager::StepKind::Manual);
        assert_eq!(steps[1].when.as_deref(), Some("token-secret-present"));
    }
}

fn ids(steps: &[SetupStep]) -> Vec<&str> {
    steps.iter().map(|step| step.id.as_str()).collect()
}

#[test]
fn plans_the_first_publish_token_as_a_confirmed_verified_exception() {
    let endpoints = Endpoints::default();
    let context = FlowContext {
        directory: ".",
        slug: Some("acme/demo".to_owned()),
        workflow: Some("release.yml".to_owned()),
        environment: None,
        verify_release: false,
        endpoints: &endpoints,
        trust_npm: None,
        manual: true,
    };
    let steps = crates_flow(&crate_package(Registry::CratesIo), &context);
    assert_eq!(
        ids(&steps),
        [
            "validate-package",
            "check-registry",
            "fetch-default-branch",
            "prepare-worktree",
            "create-publish-token",
            "sign-in",
            "first-publish",
            "wait-for-registry",
            "revoke-publish-token",
            "verify-token-revoked",
            "configure-trusted-publisher",
            "audit-token-secrets",
            "confirm-oidc-cleanup",
            "delete-token-secret",
            "sign-out",
            "remove-worktree",
        ]
    );
    let create = steps
        .iter()
        .find(|step| step.id == "create-publish-token")
        .expect("create-publish-token");
    assert!(create.confirm);
    assert!(create.description.contains("one-time exception"));
    let verify = steps
        .iter()
        .find(|step| step.id == "verify-token-revoked")
        .expect("verify-token-revoked");
    assert_eq!(
        verify.url.as_deref(),
        Some("https://crates.io/api/v1/me/tokens")
    );
    let delete = steps
        .iter()
        .find(|step| step.id == "delete-token-secret")
        .and_then(|step| step.command.as_ref())
        .expect("delete-token-secret command");
    assert_eq!(
        delete.args,
        strings(&["secret", "delete", "{token_secret}", "--repo", "acme/demo"])
    );

    let without_slug = FlowContext {
        slug: None,
        ..context.clone()
    };
    assert!(!ids(&crates_flow(
        &crate_package(Registry::CratesIo),
        &without_slug
    ))
    .iter()
    .any(|id| id.contains("secret")));
    assert_eq!(
        ids(&pypi_flow(&crate_package(Registry::PyPi), &context)),
        [
            "build-package",
            "check-registry",
            "create-pending-publisher",
            "trigger-release",
            "wait-for-registry",
            "configure-trusted-publisher",
            "audit-token-secrets",
            "confirm-oidc-cleanup",
            "delete-token-secret",
        ]
    );
}
