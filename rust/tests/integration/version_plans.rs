use super::mock_registry::MockRegistry;
use package_registry_manager::registry_state::{Endpoints, RegistryClient};
use package_registry_manager::{build_plans, Inspection, Package, PlanMode, Registry, StepKind};
use serde_json::json;

#[tokio::test]
async fn plans_use_exact_manifest_version_evidence() {
    let registry = MockRegistry::start(|path| {
        let body = match path {
            "/npm/library/latest" => r#"{"version":"2.0.0","_npmUser":{"trustedPublisher":{}}}"#,
            "/maven/org/example/library/maven-metadata.xml" => {
                "<metadata><versioning><release>2.0.0</release></versioning></metadata>"
            }
            "/nuget/library/index.json" => r#"{"versions":["2.0.0"]}"#,
            "/packagist/p2/org/library.json" => {
                r#"{"packages":{"org/library":[{"version":"v2.0.0"}]}}"#
            }
            "/go/example.com/library/@v/list" => "v2.0.0\n",
            "/go/example.com/library/@latest" => r#"{"Version":"v2.0.0"}"#,
            "/npm/library/4.0.0" => "null",
            "/maven/org/example/library/4.0.0/library-4.0.0.pom" => "<html>invalid metadata</html>",
            "/nuget/invalid/index.json" => "{}",
            "/packagist/p2/org/invalid.json" => r#"{"packages":{"org/invalid":[null]}}"#,
            "/go/example.com/library/@v/v4.0.0.info" => r#"{"Version":null}"#,
            _ => return None,
        };
        Some(body.to_owned())
    });

    let mut endpoints = Endpoints::default();
    let targets = [
        (Registry::Npm, "NPM_REGISTRY", "npm", "library", "library"),
        (
            Registry::MavenCentral,
            "MAVEN_API",
            "maven",
            "org.example:library",
            "org.example:library",
        ),
        (Registry::NuGet, "NUGET_API", "nuget", "Library", "Invalid"),
        (
            Registry::Packagist,
            "PACKAGIST_API",
            "packagist",
            "org/library",
            "org/invalid",
        ),
        (
            Registry::GoModules,
            "GO_PROXY",
            "go",
            "example.com/library",
            "example.com/library",
        ),
    ];
    for (_, key, prefix, _, _) in targets {
        endpoints = endpoints.with(
            &format!("PACKAGE_REGISTRY_MANAGER_{key}"),
            &format!("{}/{prefix}", registry.base),
        );
    }
    let client = RegistryClient::new(endpoints, false);

    for (registry, _, _, name, invalid_name) in targets {
        for (version, expected, step_id) in [
            ("3.0.0", PlanMode::Release, "publish-version"),
            ("1.0.0", PlanMode::StaleVersion, "review-manifest-version"),
            (
                "4.0.0",
                PlanMode::UnknownVersion,
                "recheck-manifest-version",
            ),
        ] {
            let unknown = expected == PlanMode::UnknownVersion;
            let mut package = Package::new(
                registry,
                if unknown { invalid_name } else { name }.into(),
                Some(if registry == Registry::GoModules {
                    format!("v{version}")
                } else {
                    version.into()
                }),
                "manifest".into(),
            );
            package.workflow = Some("release.yml".into());
            let state = client.probe_package(&package).await;
            package.exists_on_registry = state.exists;
            package.trusted_publishing = state.trusted;
            package.audit = state.audit;

            assert_eq!(
                package.audit.get("manifest_version_exists"),
                (!unknown).then_some(&json!(false)),
                "{registry} {version}"
            );
            let inspection: Inspection = serde_json::from_value(json!({
                "schema_version": 1,
                "repository": { "root": ".", "github_owner": "org", "github_repository": "repo" },
                "packages": [package],
                "skipped": []
            }))
            .unwrap();
            let plans = build_plans(&inspection);
            let plan = &plans[0];
            assert_eq!(plan.mode, Some(expected), "{registry} {version}");
            assert!(
                plan.steps.iter().any(|step| {
                    step.id == step_id && step.kind == StepKind::Manual && step.command.is_none()
                }),
                "{registry} {version}"
            );
            assert!(plan
                .steps
                .iter()
                .all(|step| !matches!(step.id.as_str(), "first-publish" | "publish-tag")));
            assert_eq!(
                plan.steps.iter().any(|step| step.id == "publish-version"),
                expected == PlanMode::Release
            );
        }
    }
}
