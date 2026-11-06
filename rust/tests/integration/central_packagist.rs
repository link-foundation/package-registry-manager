//! Mirrors js/test/central-packagist-steps.test.mjs: every generated Central,
//! Packagist and `SwiftPM` step reaches its own read-only handler, states stay
//! distinct, and a settings-only repair retries only the causal jobs.
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use package_registry_manager::central_packagist::{
    central_pair_preflight, retry_causal_jobs, run_generated_step, store_central_pair,
    store_central_pair_with_expiry, CentralPairStart, StepContext, CENTRAL_PACKAGIST_STEPS,
};
use package_registry_manager::github::GithubGateway;
use package_registry_manager::registry_state::{Endpoints, RegistryClient};
use package_registry_manager::{
    build_plans_for, inspect_repository, Inspection, Registry, SetupPlan, SetupStep, StepKind,
};
use serde_json::{json, Value};

const SLUG: &str = "repos/link-foundation/links-notation";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/links-notation")
}

fn plan(registry: Registry) -> SetupPlan {
    // Inspected once: the fixture walk dominates the test time in debug builds.
    static INSPECTION: OnceLock<Inspection> = OnceLock::new();
    let inspection =
        INSPECTION.get_or_init(|| inspect_repository(&root()).expect("inspect links-notation"));
    let mut found = build_plans_for(inspection, &BTreeSet::from([registry]))
        .into_iter()
        .find(|plan| {
            plan.registry == registry
                && plan.package.publishable
                && (registry != Registry::MavenCentral || plan.package.audit.contains_key("gradle"))
        })
        .unwrap_or_else(|| panic!("{registry} plan"));
    found.repository.github_owner = Some("link-foundation".into());
    found.repository.github_repository = Some("links-notation".into());
    found
}

fn step<'a>(plan: &'a SetupPlan, id: &str) -> &'a SetupStep {
    plan.steps
        .iter()
        .find(|step| step.id == id)
        .unwrap_or_else(|| panic!("{id} step"))
}

/// Answers each path with the first matching route, else 404.
fn serve(routes: Vec<(String, u16, Value)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
                header.clear();
            }
            let path = line.split_whitespace().nth(1).unwrap_or_default();
            let (status, body) = routes
                .iter()
                .find(|(pattern, _, _)| path.contains(pattern.as_str()))
                .map_or((404, Value::Null), |(_, status, body)| {
                    (*status, body.clone())
                });
            let body = body.to_string();
            let _ = write!(
                &stream,
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    base
}

fn client(routes: Vec<(String, u16, Value)>) -> RegistryClient {
    let base = serve(routes);
    RegistryClient::new(
        Endpoints::default()
            .with(
                "PACKAGE_REGISTRY_MANAGER_MAVEN_API",
                &format!("{base}/maven2"),
            )
            .with("PACKAGE_REGISTRY_MANAGER_MAVEN_SEARCH", &base)
            .with("PACKAGE_REGISTRY_MANAGER_PACKAGIST_API", &base),
        false,
    )
}

/// A gh-manager double: secret metadata, REST documents, runs, jobs and logs.
#[derive(Default)]
struct FakeGithub {
    secrets: BTreeSet<String>,
    files: BTreeMap<String, Value>,
    runs: Value,
    jobs: Value,
    logs: BTreeMap<u64, String>,
    verifies: Mutex<Vec<Value>>,
    writes: Mutex<Vec<String>>,
    expiries: Mutex<Vec<String>>,
    expiry: Option<String>,
    /// Whether stored secrets become readable metadata.
    reads_back_writes: bool,
}

#[allow(clippy::unused_async_trait_impl)]
impl GithubGateway for FakeGithub {
    async fn call(&self, args: &[String], input: Option<&str>, _: &Path) -> Result<Value> {
        match (args[0].as_str(), args[1].as_str()) {
            ("secret", "get-metadata") if args[3] == "--org" => anyhow::bail!("forbidden"),
            ("secret", "get-metadata") => Ok(
                if self.secrets.contains(&args[2])
                    || (self.reads_back_writes
                        && self.writes.lock().expect("lock").contains(&args[2]))
                {
                    json!({"name": args[2]})
                } else {
                    Value::Null
                },
            ),
            ("variable", "get") => Ok(self
                .expiry
                .as_ref()
                .map_or(Value::Null, |expiry| json!({"value":expiry}))),
            ("secret", "set") => {
                if let Some(index) = args.iter().position(|arg| arg == "--expires-at") {
                    self.expiries
                        .lock()
                        .expect("lock")
                        .push(args[index + 1].clone());
                }
                assert!(input.is_some(), "secret values go on stdin");
                self.writes.lock().expect("lock").push(args[2].clone());
                Ok(json!({"changed": true}))
            }
            ("runs", "list") => Ok(self.runs.clone()),
            ("repair", "verify") => {
                let request: Value = serde_json::from_slice(&std::fs::read(&args[2])?)?;
                self.verifies.lock().expect("lock").push(request);
                Ok(json!({"status":"verified","runUrl":"https://github.com/x/y/actions/runs/2"}))
            }
            _ => anyhow::bail!("unexpected gh-manager call {args:?}"),
        }
    }

    async fn rest(&self, endpoint: &str, _: &Path) -> Result<Option<Value>> {
        if let Some(found) = self.files.get(endpoint) {
            return Ok(Some(found.clone()));
        }
        if endpoint == SLUG {
            return Ok(Some(json!({"default_branch":"main"})));
        }
        if endpoint.contains("/actions/runs/") && endpoint.contains("/jobs") {
            return Ok(Some(json!({"jobs": self.jobs})));
        }
        Ok(None)
    }

    async fn job_log(&self, _: &str, job: u64, _: &Path) -> Result<String> {
        self.logs
            .get(&job)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no log"))
    }
}

async fn run(
    github: &FakeGithub,
    client: &RegistryClient,
    plan: &SetupPlan,
    id: &str,
    release_verified: bool,
) -> Value {
    let context = StepContext {
        github,
        client,
        plan,
        cwd: Path::new("."),
        wait_timeout: Duration::from_secs(5),
    };
    run_generated_step(&context, step(plan, id), &[], release_verified)
        .await
        .expect(id)
}

#[test]
fn every_generated_central_packagist_and_swift_step_reaches_its_own_handler() {
    for registry in [
        Registry::MavenCentral,
        Registry::Packagist,
        Registry::Swiftpm,
    ] {
        let plan = plan(registry);
        let routed: Vec<&str> = plan
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .filter(|id| CENTRAL_PACKAGIST_STEPS.contains(id))
            .collect();
        assert!(routed.len() >= 2, "{registry}: {routed:?}");
        for step in plan
            .steps
            .iter()
            .filter(|step| !CENTRAL_PACKAGIST_STEPS.contains(&step.id.as_str()))
        {
            assert_ne!(
                step.kind,
                StepKind::Api,
                "{registry}:{} would fall through to the crates.io session",
                step.id
            );
        }
    }
}

#[tokio::test]
async fn central_namespace_and_signing_stay_incomplete_without_read_back() {
    let github = FakeGithub {
        secrets: ["GPG_PRIVATE_KEY", "GPG_PASSPHRASE"]
            .map(String::from)
            .into(),
        ..FakeGithub::default()
    };
    let client = client(Vec::new());
    let plan = plan(Registry::MavenCentral);
    for id in ["verify-namespace", "ensure-signing-key"] {
        let result = run(&github, &client, &plan, id, false).await;
        assert_eq!(result["status"], "manual-prerequisite", "{id}");
    }
}

#[tokio::test]
async fn a_root_pom_alone_never_certifies_the_kotlin_variants() {
    let base =
        "io/github/link-foundation/links-notation-kotlin/0.30.0/links-notation-kotlin-0.30.0";
    let client = client(vec![(format!("{base}.pom"), 200, Value::Null)]);
    let plan = plan(Registry::MavenCentral);
    let result = run(
        &FakeGithub::default(),
        &client,
        &plan,
        "verify-central-publication",
        false,
    )
    .await;
    assert_eq!(result["status"], "root-metadata-missing", "{result}");
}

#[test]
fn packagist_embedded_engine_requires_authentication_without_browser() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let driver = STANDARD.encode(include_str!("../../src/packagist-driver.mjs"));
    let script = format!("const {{loadPackagistEngine}} = await import('data:text/javascript;base64,{driver}'); const {{engine}} = await loadPackagistEngine({}); console.log(JSON.stringify(await engine.acquirePackagistCredential({{options:{{noBrowser:true}}}})));", package_registry_manager::packagist::engine_sources());
    let output = std::process::Command::new("node")
        .args(["--input-type=module", "-e", &script])
        .output()
        .expect("Node is required by gh-manager");
    assert!(
        output.status.success(),
        "embedded Packagist module must execute"
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("structured result");
    assert_eq!(value["status"], "authentication-required");
    assert_eq!(value["stop"], true);
}

fn encode(text: &str) -> Value {
    json!({
        "content": base64::engine::general_purpose::STANDARD.encode(text),
        "encoding": "base64",
    })
}

#[tokio::test]
async fn swiftpm_release_states_stay_distinct() {
    let manifest = r#"let package = Package(name: "LinksNotation", products: [.library(name: "LinksNotation", targets: ["LinksNotation"])])"#;
    let tag = (
        format!("{SLUG}/git/ref/tags/0.30.0"),
        json!({"object": {"type": "commit", "sha": "abc"}}),
    );
    let version = |text: &str| {
        (
            format!("{SLUG}/contents/swift/VERSION?ref=abc"),
            encode(text),
        )
    };
    let swift = |text: &str| {
        (
            format!("{SLUG}/contents/Package.swift?ref=abc"),
            encode(text),
        )
    };
    let release = (
        format!("{SLUG}/releases/tags/0.30.0"),
        json!({"draft": false, "html_url": "https://github.com/r"}),
    );
    let cases = [
        ("missing-tag", vec![]),
        ("wrong-commit", vec![tag.clone(), version("0.29.0\n")]),
        ("manifest-missing", vec![tag.clone(), version("0.30.0\n")]),
        (
            "manifest-incorrect",
            vec![
                tag.clone(),
                version("0.30.0\n"),
                swift(r#"let package = Package(name: "Other")"#),
            ],
        ),
        (
            "release-missing",
            vec![tag.clone(), version("0.30.0\n"), swift(manifest)],
        ),
        (
            "verified",
            vec![tag, version("0.30.0\n"), swift(manifest), release],
        ),
    ];
    let plan = plan(Registry::Swiftpm);
    let client = client(Vec::new());
    for (status, files) in cases {
        let github = FakeGithub {
            files: files.into_iter().collect(),
            ..FakeGithub::default()
        };
        let result = run(&github, &client, &plan, "verify-swift-release", false).await;
        assert_eq!(result["status"], status, "{result}");
    }
}

#[tokio::test]
async fn swiftpm_consumer_failed_or_skipped_keeps_the_release_unverified() {
    let plan = plan(Registry::Swiftpm);
    let client = client(Vec::new());
    let unverified = run(
        &FakeGithub::default(),
        &client,
        &plan,
        "verify-swift-consumer",
        false,
    )
    .await;
    assert_eq!(unverified["status"], "manual-prerequisite");
    for (conclusion, status) in [
        ("failure", "consumer-failed"),
        ("skipped", "consumer-skipped"),
        ("success", "verified"),
    ] {
        let github = FakeGithub {
            runs: json!([{"id": 3, "status": "completed", "conclusion": conclusion, "html_url": "https://github.com/r/3"}]),
            jobs: json!([{
                "id": 9,
                "name": "swift / publishRelease",
                "steps": [{"name": "Verify SwiftPM resolves and builds the release", "conclusion": conclusion}],
            }]),
            ..FakeGithub::default()
        };
        let result = run(&github, &client, &plan, "verify-swift-consumer", true).await;
        assert_eq!(result["status"], status, "{conclusion}: {result}");
    }
}

#[tokio::test]
async fn a_settings_only_repair_retries_only_the_causal_jobs() {
    let github = FakeGithub {
        runs: json!([{"id": 7, "conclusion": "failure", "html_url": "https://github.com/x/y/actions/runs/7"}]),
        jobs: json!([{"id": 1, "conclusion": "failure"}, {"id": 2, "conclusion": "failure"}]),
        logs: [
            (1, "Error: CENTRAL_USERNAME secret is missing".to_owned()),
            (2, "test failure unrelated to credentials".to_owned()),
        ]
        .into(),
        ..FakeGithub::default()
    };
    let client = client(Vec::new());
    let plan = plan(Registry::MavenCentral);
    let context = StepContext {
        github: &github,
        client: &client,
        plan: &plan,
        cwd: Path::new("."),
        wait_timeout: Duration::from_secs(5),
    };
    let names = ["CENTRAL_USERNAME", "CENTRAL_TOKEN"].map(String::from);
    let result = retry_causal_jobs(&context, "link-foundation/links-notation", &names)
        .await
        .expect("retry");
    assert_eq!(result["status"], "verified", "{result}");
    assert_eq!(result["jobs"], json!([1]));
    let verifies = github.verifies.lock().expect("lock");
    assert_eq!(verifies[0]["jobIds"], json!([1]));
    assert_eq!(verifies[0]["runId"], 7);
    assert!(github.writes.lock().expect("lock").is_empty());
}

async fn preflight(github: &FakeGithub, automation_allowed: bool) -> CentralPairStart {
    let plan = plan(Registry::MavenCentral);
    central_pair_preflight(
        github,
        &plan,
        step(&plan, "ensure-central-credential-pair"),
        automation_allowed,
        Path::new("."),
    )
    .await
    .expect("preflight")
}

#[tokio::test]
async fn central_pair_existing_fields_are_kept_without_a_browser_or_crates_call() {
    // The fake bails on any call other than secret metadata, so a crates.io
    // session, browser or write would fail this test.
    let github = FakeGithub {
        secrets: BTreeSet::from(["CENTRAL_USERNAME".into(), "CENTRAL_TOKEN".into()]),
        ..FakeGithub::default()
    };
    let CentralPairStart::Done(result) = preflight(&github, true).await else {
        panic!("existing pair must not open a browser");
    };
    assert_eq!(result["status"], "present", "{result}");
    assert!(github.writes.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn central_pair_missing_fields_without_automation_stop_with_a_prerequisite() {
    let github = FakeGithub {
        secrets: BTreeSet::from(["CENTRAL_USERNAME".into()]),
        ..FakeGithub::default()
    };
    let CentralPairStart::Done(result) = preflight(&github, false).await else {
        panic!("--no-browser must return before any browser");
    };
    assert_eq!(result["status"], "manual-prerequisite", "{result}");
    assert_eq!(result["secrets"], json!(["CENTRAL_TOKEN"]));
    assert_eq!(result["stop"], true);
    assert!(github.writes.lock().expect("lock").is_empty());

    let CentralPairStart::Generate {
        names,
        present,
        missing,
        url,
        ..
    } = preflight(&github, true).await
    else {
        panic!("automation may generate the missing field");
    };
    assert_eq!(names, ["CENTRAL_USERNAME", "CENTRAL_TOKEN"]);
    assert_eq!(
        (present, missing),
        (
            vec!["CENTRAL_USERNAME".to_owned()],
            vec!["CENTRAL_TOKEN".to_owned()]
        )
    );
    assert!(url.starts_with("https://central.sonatype.com/"), "{url}");
}

#[tokio::test]
async fn a_stored_central_pair_is_read_back_before_retrying_only_causal_jobs() {
    let names = ["CENTRAL_USERNAME", "CENTRAL_TOKEN"].map(String::from);
    let client = client(Vec::new());
    let plan = plan(Registry::MavenCentral);
    let unread = FakeGithub::default();
    let context = StepContext {
        github: &unread,
        client: &client,
        plan: &plan,
        cwd: Path::new("."),
        wait_timeout: Duration::from_secs(5),
    };
    let result = store_central_pair(
        &context,
        "link-foundation/links-notation",
        &names,
        ["u", "p"],
    )
    .await
    .expect("store");
    assert_eq!(result["status"], "unverified", "{result}");
    assert!(unread.verifies.lock().expect("lock").is_empty());

    let github = FakeGithub {
        reads_back_writes: true,
        runs: json!([{"id": 7, "conclusion": "failure"}]),
        jobs: json!([{"id": 1, "conclusion": "failure"}, {"id": 2, "conclusion": "failure"}]),
        logs: [
            (1, "CENTRAL_TOKEN secret is missing".to_owned()),
            (2, "unrelated test failure".to_owned()),
        ]
        .into(),
        ..FakeGithub::default()
    };
    let context = StepContext {
        github: &github,
        ..context
    };
    let result = store_central_pair(
        &context,
        "link-foundation/links-notation",
        &names,
        ["u", "p"],
    )
    .await
    .expect("store");
    assert_eq!(result["status"], "configured", "{result}");
    assert_eq!(*github.writes.lock().expect("lock"), names);
    assert_eq!(result["retry"]["jobs"], json!([1]));
    assert!(!result.to_string().contains("\"u\"") && !result.to_string().contains("\"p\""));
}

#[tokio::test]
async fn real_gradle_metadata_pointer_graph_and_publication_failures() {
    let root: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/gradle-publication/root.module.json"
    ))
    .unwrap();
    let jvm: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/gradle-publication/jvm.module.json"
    ))
    .unwrap();
    let mut root = root;
    root["variants"].as_array_mut().unwrap().retain(|v| {
        v.get("available-at").is_none()
            || v["available-at"]["module"]
                .as_str()
                .is_some_and(|m| m.ends_with("-jvm"))
    });
    let mut plan = plan(Registry::MavenCentral);
    let step = plan
        .steps
        .iter_mut()
        .find(|s| s.id == "verify-central-publication")
        .unwrap();
    step.details.insert("coordinates".into(), json!({"group":"io.github.link-foundation","artifact":"links-notation-kotlin","version":"0.31.0"}));
    step.details.insert(
        "variants".into(),
        json!(["io.github.link-foundation:links-notation-kotlin-jvm"]),
    );
    for case in [
        "verified",
        "wrong-version",
        "object-module",
        "missing-binary",
        "missing-module",
        "invalid-json",
        "transport",
        "missing-platform",
        "absent-platform-fixture",
    ] {
        let mut document = root.clone();
        if case == "wrong-version" {
            document["variants"][2]["available-at"]["version"] = json!("0.30.0");
        }
        if case == "object-module" {
            document["variants"][2]["available-at"]["module"] = json!({"name":"wrong"});
        }
        let mut routes = vec![];
        if case == "missing-binary" {
            routes.push(("-jvm-0.31.0.jar".into(), 404, Value::Null));
        }
        let module_status = match case {
            "missing-module" => 404,
            "transport" => 503,
            _ => 200,
        };
        routes.push((
            "links-notation-kotlin-0.31.0.module".into(),
            module_status,
            if case == "invalid-json" {
                json!("not metadata")
            } else {
                document
            },
        ));
        let absent: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/gradle-publication/missing-platform.json"
        ))
        .unwrap();
        routes.push((
            "-jvm-0.31.0.module".into(),
            if case == "absent-platform-fixture" {
                404
            } else {
                200
            },
            jvm.clone(),
        ));
        routes.push((".pom".into(), 200, Value::Null));
        routes.push((".jar".into(), 200, Value::Null));
        let mut selected = plan.clone();
        if case == "missing-platform" {
            selected
                .steps
                .iter_mut()
                .find(|s| s.id == "verify-central-publication")
                .unwrap()
                .details
                .insert(
                    "variants".into(),
                    json!(["io.github.link-foundation:links-notation-kotlin-js"]),
                );
        }
        let result = run(
            &FakeGithub::default(),
            &client(routes),
            &selected,
            "verify-central-publication",
            false,
        )
        .await;
        let expected = match case {
            "wrong-version" | "object-module" | "invalid-json" => "malformed-metadata",
            "missing-module" => "root-metadata-missing",
            "missing-binary" | "missing-platform" | "absent-platform-fixture" => "partial",
            "transport" => "transport-unknown",
            _ => "verified",
        };
        assert_eq!(result["status"], expected, "{case}: {result}");
        if case == "absent-platform-fixture" {
            assert_eq!(result["status"], absent["expected_status"]);
            assert!(result["missing"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value
                    .as_str()
                    .unwrap_or_default()
                    .contains(absent["platform"].as_str().unwrap())));
        }
        assert_eq!(result["evidence"]["signatures"], "unverified");
        if case == "verified" {
            assert_eq!(result["files"], 4);
        }
    }
}

#[tokio::test]
async fn central_expiry_is_preserved_for_storage_and_second_setup() {
    let names = ["CENTRAL_USERNAME", "CENTRAL_TOKEN"].map(String::from);
    let expiry = "2099-01-01T00:00:00Z";
    let github = FakeGithub {
        reads_back_writes: true,
        expiry: Some(expiry.into()),
        ..FakeGithub::default()
    };
    let client = client(Vec::new());
    let plan = plan(Registry::MavenCentral);
    let context = StepContext {
        github: &github,
        client: &client,
        plan: &plan,
        cwd: Path::new("."),
        wait_timeout: Duration::from_secs(5),
    };
    let result = store_central_pair_with_expiry(
        &context,
        "link-foundation/links-notation",
        &names,
        ["private-user", "private-token"],
        Some(expiry),
    )
    .await
    .unwrap();
    assert_eq!(result["expiry"]["expires_at"], expiry);
    assert_eq!(*github.expiries.lock().unwrap(), [expiry, expiry]);
    let CentralPairStart::Done(second) = central_pair_preflight(
        &github,
        &plan,
        step(&plan, "ensure-central-credential-pair"),
        false,
        Path::new("."),
    )
    .await
    .unwrap() else {
        panic!("existing pair must not regenerate")
    };
    assert_eq!(second["status"], "present");
    assert_eq!(second["expiry"]["expires_at"], expiry);
    assert_eq!(github.writes.lock().unwrap().len(), 2);
    assert!(
        !result.to_string().contains("private-user")
            && !result.to_string().contains("private-token")
    );
}
