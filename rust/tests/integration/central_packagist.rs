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
    CentralPairStart, StepContext, CENTRAL_PACKAGIST_STEPS,
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
            ("secret", "set") => {
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
    assert_eq!(result["status"], "partial", "{result}");
}

#[tokio::test]
async fn packagist_exact_stable_tag_states_stay_distinct() {
    let name = "link-foundation/links-notation";
    let tagged = format!("/p2/{name}.json");
    let dev = format!("/p2/{name}~dev.json");
    let versions = |versions: &[&str]| json!({"packages": {name: versions.iter().map(|v| json!({"version": v})).collect::<Vec<_>>()}});
    let cases = [
        ("unregistered", vec![]),
        ("auth-rejected", vec![(tagged.clone(), 403, Value::Null)]),
        (
            "transport-unknown",
            vec![(tagged.clone(), 503, Value::Null)],
        ),
        (
            "dev-only",
            vec![
                (dev.clone(), 200, versions(&["dev-main"])),
                (tagged.clone(), 200, versions(&[])),
            ],
        ),
        (
            "stable-version-missing",
            vec![(tagged.clone(), 200, versions(&["0.29.0"]))],
        ),
        (
            "verified",
            vec![(tagged.clone(), 200, versions(&["0.30.0"]))],
        ),
    ];
    let plan = plan(Registry::Packagist);
    assert_eq!(plan.package.version.as_deref(), Some("0.30.0"));
    for (status, routes) in cases {
        let client = client(routes);
        let result = run(
            &FakeGithub::default(),
            &client,
            &plan,
            "verify-packagist-package",
            false,
        )
        .await;
        assert_eq!(result["status"], status, "{result}");
    }
    let hook = run(
        &FakeGithub::default(),
        &client(Vec::new()),
        &plan,
        "packagist-github-hook",
        false,
    )
    .await;
    assert_eq!(hook["status"], "manual-prerequisite");
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
