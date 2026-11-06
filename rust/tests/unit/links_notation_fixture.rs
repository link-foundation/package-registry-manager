//! Mirrors js/test/links-notation-fixture.test.mjs, nuget-guarded-migration.test.mjs,
//! the required/unmet `NuGet` CI job gate and the Go proxy exact-version lookup.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use package_registry_manager::nuget_migration::{convert_step, PrerequisiteError};
use package_registry_manager::registry_state::Endpoints;
use package_registry_manager::registry_state::RegistryClient;
use package_registry_manager::setup::{required_jobs, unmet_jobs};
use package_registry_manager::workflow_migration::workflow_migration_proposal;
use package_registry_manager::workflows::Workflow;
use package_registry_manager::{
    build_plans_for, build_plans_with, inspect_repository, Inspection, Package, PlanOptions,
    Registry, SetupPlan,
};
use serde_json::{json, Value};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/links-notation")
}

fn inspection() -> Inspection {
    inspect_repository(&root()).expect("inspect links-notation")
}

fn summary(package: &Package) -> Value {
    json!({
        "registry": package.registry,
        "name": package.name,
        "version": package.version,
        "manifest": package.manifest,
        "publishable": package.publishable,
        "workflow": package.workflow,
        "workflow_jobs": package.workflow_jobs,
        "workflow_callees": package.workflow_callees,
        "publishing_policy": package.publishing_policy,
    })
}

fn plans(registries: &[Registry], options: &PlanOptions) -> Vec<SetupPlan> {
    build_plans_with(
        &inspection(),
        &registries.iter().copied().collect::<BTreeSet<_>>(),
        options,
    )
}

fn step(plan: &SetupPlan, id: &str) -> Value {
    serde_json::to_value(
        plan.steps
            .iter()
            .find(|step| step.id == id)
            .unwrap_or_else(|| panic!("{id} step")),
    )
    .expect("step json")
}

fn secrets(step: &Value) -> Vec<&str> {
    step["secrets"]
        .as_array()
        .expect("secrets")
        .iter()
        .map(|entry| entry["secret"].as_str().expect("secret"))
        .collect()
}

#[test]
fn links_notation_identities_match_the_contract() {
    let inspection = inspection();
    let expected: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("expected-packages.json")).expect("expected"),
    )
    .expect("expected json");
    let actual: Vec<Value> = inspection.packages.iter().map(summary).collect();
    assert_eq!(Value::Array(actual), expected);
    for item in inspection.packages.iter().filter(|p| p.publishable) {
        assert_eq!(item.workflow.as_deref(), Some("js.yml"), "{}", item.name);
        assert!(!item.workflow_callees.is_empty() || item.registry != Registry::Cmake);
    }
    assert!(
        inspection
            .skipped
            .iter()
            .any(|s| s.manifest == "php/composer.json")
            || !inspection
                .packages
                .iter()
                .any(|p| p.manifest == "php/composer.json")
    );
    assert!(!inspection
        .packages
        .iter()
        .any(|p| p.manifest.to_lowercase().contains("bench") && p.publishable));
}

#[test]
fn links_notation_has_no_false_add_publishing_job_request() {
    let options = PlanOptions {
        add_publish_job: true,
        ..PlanOptions::default()
    };
    for plan in plans(&[], &options) {
        let reason = plan.skipped_reason.clone().unwrap_or_default();
        assert!(
            !reason.contains("no workflow publishes"),
            "{}: {reason}",
            plan.package.registry
        );
    }
}

#[test]
fn links_notation_maven_and_packagist_expose_credential_states() {
    let all = build_plans_for(
        &inspection(),
        &BTreeSet::from([Registry::MavenCentral, Registry::Packagist]),
    );
    let maven = all
        .iter()
        .find(|p| p.package.registry == Registry::MavenCentral)
        .expect("maven plan");
    let pair = step(maven, "ensure-central-credential-pair");
    assert_eq!(secrets(&pair), ["CENTRAL_USERNAME", "CENTRAL_TOKEN"]);
    assert_eq!(pair["oidc"], "unsupported");
    assert_eq!(
        secrets(&step(maven, "ensure-signing-key")),
        ["GPG_PRIVATE_KEY", "GPG_PASSPHRASE"]
    );
    assert_eq!(
        step(maven, "verify-namespace")["namespace"],
        "io.github.link-foundation"
    );
    assert!(!maven.steps.iter().any(|s| {
        let id = s.id.to_lowercase();
        id.contains("rotate") || id.contains("revoke")
    }));
    let packagist = all
        .iter()
        .find(|p| p.package.registry == Registry::Packagist)
        .expect("packagist plan");
    assert_eq!(step(packagist, "packagist-update-api")["kind"], "api");
    assert_eq!(step(packagist, "packagist-github-hook")["kind"], "api");
}

#[test]
fn links_notation_nuget_migration_keeps_pack_guard_fallback_and_id_token() {
    // Upstream now uses NuGet trusted publishing; the API-key workflows it
    // replaced are frozen next to the fixture.
    let dir = root()
        .parent()
        .expect("fixtures")
        .join("links-notation-legacy-nuget/.github/workflows");
    let workflows: Vec<Workflow> = std::fs::read_dir(&dir)
        .expect("workflows")
        .map(|entry| {
            let entry = entry.expect("entry");
            Workflow {
                name: entry.file_name().to_string_lossy().into_owned(),
                contents: std::fs::read_to_string(entry.path()).expect("workflow"),
            }
        })
        .collect();
    let nuget = inspection()
        .packages
        .into_iter()
        .find(|p| p.registry == Registry::NuGet)
        .expect("nuget package");
    let files = workflow_migration_proposal(&root(), &[nuget], &workflows).expect("proposal");
    let csharp = &files["publish-csharp.yml"];
    assert!(regex::Regex::new(
        r"- name: Pack\n\s+if: steps.version-check.outputs.should_publish == 'true'"
    )
    .unwrap()
    .is_match(csharp));
    assert!(csharp.contains("NuGet/login@"));
    assert!(csharp.contains("secrets.NUGET_API_KEY || secrets.NUGET_TOKEN"));
    assert!(csharp.contains("published=skipped"));
    assert_eq!(csharp.matches("dotnet pack").count(), 1);
    for name in ["js.yml", "release.yml", "publish-csharp.yml"] {
        assert!(files[name].contains("id-token: write"), "{name}");
    }
}

#[test]
fn links_notation_variable_gated_crates_flip_the_variable() {
    let options = PlanOptions {
        migrate: true,
        ..PlanOptions::default()
    };
    let found = plans(&[Registry::CratesIo], &options);
    for plan in found.iter().filter(|p| p.package.publishable) {
        let ids: Vec<_> = plan.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["enable-oidc-variable"]);
        assert!(plan.steps[0].title.contains("CRATES_IO_TRUSTED_PUBLISHING"));
    }
}

const GUARDED: &[&str] = &[
    "set -euo pipefail",
    r#"if [ "${{ steps.version-check.outputs.should_publish }}" != "true" ]; then"#,
    r#"  echo "published=skipped" >> "$GITHUB_OUTPUT""#,
    "  exit 0",
    "fi",
    r#"API_KEY="${NUGET_API_KEY:-$NUGET_TOKEN}""#,
    "# Report presence, never the value.",
    r#"if [ -z "$API_KEY" ]; then"#,
    r#"  echo "published=failed" >> "$GITHUB_OUTPUT""#,
    "  exit 1",
    "fi",
    r#"dotnet nuget push "artifacts/*.nupkg" \"#,
    r#"  --api-key "$API_KEY" \"#,
    "  --source https://api.nuget.org/v3/index.json \\",
    "  --skip-duplicate",
    r#"echo "published=true" >> "$GITHUB_OUTPUT""#,
];

fn guarded_step(body: &[String]) -> Vec<String> {
    [
        "      - name: Publish NuGet package",
        "        id: publish",
        "        env:",
        "          NUGET_API_KEY: ${{ secrets.NUGET_API_KEY }}",
        "          NUGET_TOKEN: ${{ secrets.NUGET_TOKEN }}",
        "        run: |",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .chain(body.iter().map(|line| format!("          {line}")))
    .collect()
}

fn nuget_package() -> Vec<Package> {
    vec![Package::new(
        Registry::NuGet,
        "A.Lib".into(),
        None,
        "csharp/A.Lib/A.Lib.csproj".into(),
    )]
}

fn guarded() -> Vec<String> {
    GUARDED.iter().map(|line| (*line).to_owned()).collect()
}

#[test]
fn guarded_multiline_nuget_push_becomes_oidc_first() {
    let text = convert_step(&guarded_step(&guarded()), &nuget_package(), 3)
        .expect("converted")
        .join("\n");
    assert!(text.contains("NuGet/login@"));
    assert!(text.contains("PRM_NUGET_FALLBACK: ${{ secrets.NUGET_API_KEY || secrets.NUGET_TOKEN"));
    assert!(text.contains("PRM_ARTIFACT_GLOB: 'artifacts/*.nupkg'"));
    assert!(text.contains("if: (steps.version-check.outputs.should_publish == 'true')"));
    assert!(text.contains("id: publish\n"));
    assert!(text.contains("published=skipped"));
    assert!(!regex::Regex::new(r"dotnet (pack|test|build)")
        .unwrap()
        .is_match(&text));
}

#[test]
fn unreviewed_nuget_push_commands_return_the_prerequisite_edit() {
    for extra in ["dotnet pack x.csproj", "dotnet test", "curl https://evil"] {
        let mut body = guarded();
        body.insert(5, extra.to_owned());
        let error = convert_step(&guarded_step(&body), &nuget_package(), 0).expect_err(extra);
        let prerequisite = error
            .downcast_ref::<PrerequisiteError>()
            .expect("prerequisite");
        assert_eq!(prerequisite.code, "nuget-prerequisite-edit");
        assert!(error.to_string().contains("Split the NuGet publish step"));
    }
    let other: Vec<String> = guarded()
        .into_iter()
        .map(|line| {
            line.replace(
                "https://api.nuget.org/v3/index.json",
                "https://example.test/v3/index.json",
            )
        })
        .collect();
    let error = convert_step(&guarded_step(&other), &nuget_package(), 0).expect_err("source");
    assert!(error.to_string().contains("nuget.org destination"));
}

#[test]
fn nuget_ci_skipped_jobs_block_only_when_required() {
    let mut package = nuget_package().remove(0);
    package.workflow_jobs = vec!["release".into()];
    package.workflow_callees = vec!["release.yml -> publish-csharp.yml -> pushToNuget".into()];
    let required = required_jobs(&package);
    let mut jobs = vec![
        json!({"name": "dependency-review", "conclusion": "skipped"}),
        json!({"name": "benchmark", "conclusion": "skipped"}),
        json!({"name": "release / publish-csharp / pushToNuget", "conclusion": "success"}),
    ];
    assert_eq!(unmet_jobs(&jobs, &required).len(), 0);
    jobs[2]["conclusion"] = json!("skipped");
    assert_ne!(unmet_jobs(&jobs, &required).len(), 0);
    jobs[2]["conclusion"] = json!("success");
    jobs[0]["conclusion"] = json!("failure");
    assert_ne!(unmet_jobs(&jobs, &required).len(), 0);
    assert_ne!(
        unmet_jobs(&[json!({"name": "x", "conclusion": "skipped"})], &[]).len(),
        0
    );
}

/// Serves the Go proxy routes by suffix, 404 otherwise, for `requests` connections.
fn go_proxy(requests: usize) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = [
        ("/@v/list", "v0.27.0\nv0.27.1\n"),
        ("/@latest", r#"{"Version":"v0.27.1"}"#),
        (
            "/@v/v0.27.1.info",
            r#"{"Version":"v0.27.1","Time":"2026-01-01T00:00:00Z"}"#,
        ),
    ];
    let handle = std::thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 8192];
            let received = stream.read(&mut request).unwrap();
            let head = String::from_utf8_lossy(&request[..received]);
            let path = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let (status, body) = served
                .iter()
                .find(|(suffix, _)| path.ends_with(suffix))
                .map_or(("404 Not Found", "not found"), |(_, body)| ("200 OK", body));
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (base, handle)
}

#[tokio::test]
async fn go_proxy_exact_version_uses_the_info_endpoint() {
    let (base, server) = go_proxy(6);
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_GO_PROXY", &base),
        false,
    );
    let item = |version: &str| {
        Package::new(
            Registry::GoModules,
            "github.com/link-foundation/links-notation/go".into(),
            Some(version.into()),
            "go/go.mod".into(),
        )
    };
    let found = client.distribution_state(&item("v0.27.1")).await;
    assert_eq!(found.audit["manifest_version_exists"], true);
    assert!(found.audit["version_evidence_url"]
        .as_str()
        .unwrap()
        .ends_with("links-notation/go/@v/v0.27.1.info"));
    let missing = client.distribution_state(&item("v0.28.0")).await;
    assert_eq!(missing.audit["manifest_version_exists"], false);
    assert_eq!(missing.audit["version_state"], "manifest-version-absent");
    server.join().unwrap();
}

#[test]
fn links_notation_swiftpm_shares_the_bare_tag_and_skips_consumer_examples() {
    let inspection = inspection();
    let swift: Vec<Value> = inspection
        .packages
        .iter()
        .filter(|p| p.registry == Registry::Swiftpm)
        .map(|p| serde_json::to_value(p).expect("package json"))
        .collect();
    assert_eq!(swift.len(), 1, "{swift:?}");
    let package = &swift[0];
    assert_eq!(package["manifest"], "Package.swift");
    assert_eq!(package["name"], "LinksNotation");
    assert_eq!(package["version"], "0.30.0");
    assert_eq!(package["version_source"], "swift/VERSION");
    assert_eq!(
        package["workflow_callees"],
        json!(["release.yml -> publish-swift.yml -> publishRelease"])
    );
    assert_eq!(package["swift_package"]["tag"], "0.30.0");
    assert_eq!(
        package["swift_package"]["shared_tag"]["with"],
        json!(["packagist:link-foundation/links-notation"])
    );
    let plans = plans(&[Registry::Swiftpm], &PlanOptions::default());
    let ids: Vec<&str> = plans[0].steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "describe-package",
            "build-package",
            "test-package",
            "verify-swift-release",
            "verify-swift-consumer",
        ]
    );
    // Git tags via the workflow's GITHUB_TOKEN: no secret, token or OIDC setup.
    for step in &plans[0].steps {
        assert!(!step.details.contains_key("secrets") && !step.details.contains_key("oidc"));
        let id = step.id.to_lowercase();
        assert!(!id.contains("token") && !id.contains("oidc"), "{id}");
    }
}

#[test]
fn links_notation_kotlin_multiplatform_stays_in_central_with_gradle_steps() {
    let inspection = inspection();
    let kotlin = inspection
        .packages
        .iter()
        .find(|p| p.audit.contains_key("gradle"))
        .expect("kotlin package");
    assert_eq!(kotlin.registry, Registry::MavenCentral);
    assert_eq!(
        kotlin.name,
        "io.github.link-foundation:links-notation-kotlin"
    );
    assert_eq!(kotlin.version.as_deref(), Some("0.30.0"));
    let gradle = &kotlin.audit["gradle"];
    assert_eq!(gradle["publish_task"], "publishToMavenCentral");
    assert_eq!(gradle["multiplatform"], true);
    let variants = kotlin.audit["variants"].as_array().expect("variants");
    assert!(variants.len() >= 6, "{variants:?}");
    assert!(
        variants.iter().any(|v| v["host"] == "macos"),
        "Apple targets need a macOS runner"
    );
    let java = inspection
        .packages
        .iter()
        .find(|p| p.manifest == "java/pom.xml")
        .expect("java package");
    assert_eq!(java.registry, Registry::MavenCentral);
    assert!(!java.audit.contains_key("gradle"));
    let plan = plans(&[Registry::MavenCentral], &PlanOptions::default())
        .into_iter()
        .find(|p| p.package.audit.contains_key("gradle"))
        .expect("gradle plan");
    let commands = serde_json::to_string(
        &plan
            .steps
            .iter()
            .map(|s| s.command.clone())
            .collect::<Vec<_>>(),
    )
    .expect("commands");
    assert!(!commands.contains("\"mvn\""), "{commands}");
    assert!(commands.contains("publishToMavenLocal"), "{commands}");
    assert!(plan.steps.iter().any(|s| s.id == "verify-gradle-toolchain"));
    assert_eq!(
        step(&plan, "verify-central-publication")["variants"]
            .as_array()
            .map(Vec::len),
        Some(variants.len())
    );
}
