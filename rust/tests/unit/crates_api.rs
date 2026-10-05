// The fake crates.io host answers synchronously and tracks its state in flags.
#![allow(
    unknown_lints,
    clippy::struct_excessive_bools,
    clippy::unused_async_trait_impl
)]

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{bail, Result};
use browser_commander::BrowserProfile;
use chrono::{DateTime, Utc};
use package_registry_manager::automation::expired_cookies;
use package_registry_manager::browser_options::{parse_import_scope, ImportScope};
use package_registry_manager::crates_api::{
    crate_version, first_publish, first_publish_question, github_config_request, page_fetch_script,
    sign_in, token_request, CratesHost, LOGIN_CLICK_SCRIPT, TOKEN_VARIABLE,
};
use package_registry_manager::flows::{crates_flow, FlowContext};
use package_registry_manager::model::TrustedPublisherPrefill;
use package_registry_manager::registry_state::Endpoints;
use package_registry_manager::sign_in_import::{
    choose_sign_in_source, find_sign_in_sources, sign_in_domains, CookieStores, SignInSource,
};
use package_registry_manager::tokens::TokenState;
use package_registry_manager::Registry;
use serde_json::{json, Value};

use crate::tokens::crate_package;

/// The secret the fake crates.io hands out; it must never be printed.
const TOKEN: &str = "cio_secret_first_publish_token_0123456789";
/// Set in the child process that runs the flow for the output check.
const CHILD: &str = "PRM_CRATES_API_OUTPUT_CHILD";

fn publisher() -> TrustedPublisherPrefill {
    TrustedPublisherPrefill {
        provider: "github".to_owned(),
        organization: "acme".to_owned(),
        repository: "demo".to_owned(),
        workflow: "release.yml".to_owned(),
        environment: None,
        project: None,
    }
}

fn now() -> DateTime<Utc> {
    "2026-10-04T12:00:00.000Z".parse().expect("timestamp")
}

fn me(verified: bool) -> Value {
    json!({ "status": 200, "body": { "user": { "login": "maintainer", "email_verified": verified } } })
}

fn signed_out() -> Value {
    json!({ "status": 403, "body": { "errors": [{ "detail": "must be logged in" }] } })
}

/// A fake crates.io page and API: the token works until
/// `DELETE /tokens/current`, unless `stuck` keeps it active.
struct FakeHost {
    created: Value,
    me: Vec<Value>,
    publish_code: i32,
    stuck: bool,
    revoked: bool,
    clicked: bool,
    waited: bool,
    scripts: Vec<String>,
    envs: Vec<BTreeMap<String, String>>,
    process_token: Vec<Option<String>>,
    calls: Vec<String>,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            created: json!({ "status": 200, "body": { "api_token": { "id": 42, "token": TOKEN } } }),
            me: vec![me(true)],
            publish_code: 0,
            stuck: false,
            revoked: false,
            clicked: false,
            waited: false,
            scripts: Vec::new(),
            envs: Vec::new(),
            process_token: Vec::new(),
            calls: Vec::new(),
        }
    }
}

impl CratesHost for FakeHost {
    fn verbose(&self) -> bool {
        true
    }

    async fn evaluate(&mut self, script: &str) -> Result<Value> {
        self.scripts.push(script.to_owned());
        if script == LOGIN_CLICK_SCRIPT {
            self.clicked = true;
            return Ok(json!(true));
        }
        if script == page_fetch_script("GET", "/api/v1/me", None) {
            let answer = if self.me.len() > 1 {
                self.me.remove(0)
            } else {
                self.me[0].clone()
            };
            return Ok(answer);
        }
        if script.contains(r#"fetch("/api/v1/me/tokens", "#) {
            return Ok(self.created.clone());
        }
        if script == page_fetch_script("DELETE", "/api/v1/me/tokens/42", None) {
            return Ok(json!({ "status": 204, "body": null }));
        }
        bail!("unexpected page script {script}")
    }

    async fn goto(&mut self, _url: &str) -> Result<()> {
        Ok(())
    }

    async fn publish(&mut self, env: &BTreeMap<String, String>) -> Result<i32> {
        self.envs.push(env.clone());
        self.process_token.push(std::env::var(TOKEN_VARIABLE).ok());
        Ok(self.publish_code)
    }

    async fn wait_for_registry(&mut self) -> Result<()> {
        self.waited = true;
        Ok(())
    }

    async fn token_call(
        &mut self,
        method: &str,
        path: &str,
        token: &str,
        _body: Option<&Value>,
    ) -> (u16, Value) {
        self.calls.push(format!("{method} {path} {token}"));
        if method == "DELETE" && path == "/tokens/current" {
            self.revoked = !self.stuck;
            return (204, Value::Null);
        }
        (201, json!({}))
    }

    async fn token_state(&mut self, token: &str) -> Option<TokenState> {
        self.calls.push(format!("GET /me/tokens {token}"));
        Some(if self.revoked {
            TokenState::Revoked
        } else {
            TokenState::Active
        })
    }
}

#[test]
fn requests_a_one_hour_token_limited_to_the_crate_and_two_endpoints() {
    assert_eq!(
        token_request("demo", now()),
        json!({
            "api_token": {
                "name": "prm-first-publish-demo",
                "crate_scopes": ["demo"],
                "endpoint_scopes": ["publish-new", "trusted-publishing"],
                "expired_at": "2026-10-04T13:00:00.000Z",
            }
        })
    );
    assert_eq!(
        github_config_request("demo", &publisher()),
        json!({
            "github_config": {
                "crate": "demo",
                "repository_owner": "acme",
                "repository_name": "demo",
                "workflow_filename": "release.yml",
                "environment": null,
            }
        })
    );
    assert_eq!(
        first_publish_question("demo", Some("0.1.0"), Some(&publisher())),
        "Create a 1-hour token limited to crate demo with publish-new + trusted-publishing, publish demo v0.1.0, attach release.yml as trusted publisher, revoke the token? [y/N] "
    );
    let script = page_fetch_script(
        "PUT",
        "/api/v1/me/tokens",
        Some(&token_request("demo", now())),
    );
    assert!(
        script.contains(r#""credentials":"same-origin""#),
        "{script}"
    );
}

#[test]
fn reads_the_crate_version_from_the_package_table_only() {
    let directory = std::env::temp_dir().join(format!("prm-crate-version-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("directory");
    std::fs::write(
        directory.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n\n[dependencies]\nserde = { version = \"1\" }\n",
    )
    .expect("manifest");
    assert_eq!(crate_version(&directory).as_deref(), Some("1.2.3"));
    std::fs::write(
        directory.join("Cargo.toml"),
        "[workspace.package]\nversion = \"9.9.9\"\n\n[package]\nname = \"demo\"\nversion.workspace = true\n",
    )
    .expect("manifest");
    assert_eq!(crate_version(&directory), None);
    std::fs::remove_dir_all(&directory).expect("cleanup");
    assert_eq!(crate_version(&directory), None);
}

#[tokio::test]
async fn creates_the_token_in_the_page_hands_it_only_to_publish_attaches_revokes_and_verifies() {
    let mut host = FakeHost::new();
    let attached = first_publish(&mut host, "demo", Some(&publisher()), now())
        .await
        .expect("first publish");
    assert!(attached);
    assert_eq!(
        host.scripts,
        [page_fetch_script(
            "PUT",
            "/api/v1/me/tokens",
            Some(&token_request("demo", now()))
        )]
    );
    assert_eq!(
        host.envs,
        [BTreeMap::from([(
            TOKEN_VARIABLE.to_owned(),
            TOKEN.to_owned()
        )])]
    );
    assert_eq!(host.process_token, [None], "the token reached this process");
    assert!(host.waited);
    assert_eq!(
        host.calls,
        [
            format!("POST /trusted_publishing/github_configs {TOKEN}"),
            format!("DELETE /tokens/current {TOKEN}"),
            format!("GET /me/tokens {TOKEN}"),
        ]
    );
}

#[tokio::test]
async fn revokes_the_token_when_cargo_publish_fails() {
    let mut host = FakeHost::new();
    host.publish_code = 101;
    let error = first_publish(&mut host, "demo", Some(&publisher()), now())
        .await
        .expect_err("publish fails");
    assert!(
        format!("{error:#}").contains("cargo exited with status 101"),
        "{error:#}"
    );
    assert!(!host.waited);
    assert_eq!(
        host.calls,
        [
            format!("DELETE /tokens/current {TOKEN}"),
            format!("GET /me/tokens {TOKEN}"),
        ]
    );
    assert!(!format!("{error:#}").contains(TOKEN));
}

#[tokio::test]
async fn fails_the_run_while_crates_io_still_accepts_the_token() {
    let mut host = FakeHost::new();
    host.stuck = true;
    let error = first_publish(&mut host, "demo", Some(&publisher()), now())
        .await
        .expect_err("token still active");
    let message = format!("{error:#}");
    assert!(
        message.contains(
            "first-publish token still authenticates on crates.io; revoke it at https://crates.io/settings/tokens"
        ),
        "{message}"
    );
    assert!(!message.contains(TOKEN));
    // The browser session revokes it by id as a fallback.
    assert_eq!(
        host.scripts.last(),
        Some(&page_fetch_script("DELETE", "/api/v1/me/tokens/42", None))
    );
}

#[tokio::test]
async fn reports_both_failures_when_publish_fails_and_the_token_stays_active() {
    let mut host = FakeHost::new();
    host.stuck = true;
    host.publish_code = 101;
    let message = format!(
        "{:#}",
        first_publish(&mut host, "demo", None, now())
            .await
            .expect_err("both fail")
    );
    assert!(message.contains("still authenticates"), "{message}");
    assert!(
        message.contains("cargo exited with status 101"),
        "{message}"
    );
}

#[tokio::test]
async fn stops_before_publishing_when_crates_io_creates_no_token() {
    let mut host = FakeHost::new();
    host.created =
        json!({ "status": 400, "body": { "errors": [{ "detail": "crate scope is invalid" }] } });
    let error = first_publish(&mut host, "demo", None, now())
        .await
        .expect_err("no token");
    assert!(
        error
            .to_string()
            .contains("crate scope is invalid); nothing was published"),
        "{error}"
    );
    assert_eq!(host.envs, Vec::<BTreeMap<String, String>>::new());
    assert_eq!(host.calls, Vec::<String>::new());
}

#[tokio::test]
async fn signs_in_with_github_then_requires_a_verified_email() {
    let mut host = FakeHost::new();
    host.me = vec![signed_out(), signed_out(), me(true)];
    let session = sign_in(&mut host, Duration::from_millis(1), Duration::from_secs(5))
        .await
        .expect("signed in");
    assert!(host.clicked, "clicked Log in with GitHub");
    assert_eq!(session.login.as_deref(), Some("maintainer"));

    let mut unverified = FakeHost::new();
    unverified.me = vec![me(false)];
    let message = sign_in(
        &mut unverified,
        Duration::from_millis(1),
        Duration::from_secs(5),
    )
    .await
    .expect_err("unverified")
    .to_string();
    assert!(message.contains("verified email address"), "{message}");
    assert!(
        message.contains("https://crates.io/settings/profile"),
        "{message}"
    );
    assert!(!unverified.clicked);
}

#[test]
fn plans_the_api_flow_by_default_and_the_checklist_with_manual() {
    let endpoints = Endpoints::default();
    let context = FlowContext {
        directory: ".",
        slug: Some("acme/demo".to_owned()),
        workflow: None,
        environment: None,
        verify_release: false,
        endpoints: &endpoints,
        trust_npm: None,
        manual: false,
    };
    let package = crate_package(Registry::CratesIo);
    let ids = |steps: Vec<package_registry_manager::SetupStep>| -> Vec<String> {
        steps.into_iter().map(|step| step.id).collect()
    };
    assert_eq!(
        ids(crates_flow(&package, &context)),
        [
            "validate-package",
            "check-registry",
            "fetch-default-branch",
            "prepare-worktree",
            "crates-sign-in",
            "first-publish",
            "attach-trusted-publisher",
            "configure-trusted-publisher",
            "audit-token-secrets",
            "delete-token-secret",
            "crates-sign-out",
            "remove-worktree",
        ]
    );
    let manual = ids(crates_flow(
        &package,
        &FlowContext {
            manual: true,
            ..context
        },
    ));
    assert!(manual.iter().any(|id| id == "create-publish-token"));
    assert!(!manual.iter().any(|id| id == "crates-sign-in"));
}

/// Installed browsers for the test: Chrome and Firefox have a default
/// profile; Chrome is signed in to crates.io only.
struct FakeStores;

impl CookieStores for FakeStores {
    fn profiles(&self, browser: &str) -> Result<Vec<BrowserProfile>> {
        Ok(if matches!(browser, "chrome" | "firefox") {
            vec![BrowserProfile {
                browser: browser.to_owned(),
                name: "Default".to_owned(),
                display_name: String::new(),
                path: format!("/{browser}/Default").into(),
                is_default: true,
            }]
        } else {
            Vec::new()
        })
    }

    fn count(&self, browser: &str, _profile: &str, domain: &str) -> Result<usize> {
        if browser == "edge" {
            bail!("no cookie store");
        }
        Ok(usize::from(
            !(browser == "chrome" && domain == "github.com"),
        ))
    }
}

#[test]
fn offers_only_the_registry_sign_in_cookies_the_default_browser_first() {
    assert_eq!(
        sign_in_domains(Registry::CratesIo),
        ["crates.io", "github.com"]
    );
    assert_eq!(sign_in_domains(Registry::Npm), ["npmjs.com"]);
    assert_eq!(sign_in_domains(Registry::PyPi), ["pypi.org", "github.com"]);
    let sources = find_sign_in_sources(
        &FakeStores,
        &["crates.io", "github.com"],
        Some("firefox"),
        false,
    );
    let firefox = SignInSource {
        browser: "firefox".to_owned(),
        profile: Some("Default".to_owned()),
        label: "Firefox".to_owned(),
        domains: vec!["crates.io".to_owned(), "github.com".to_owned()],
    };
    let chrome = SignInSource {
        browser: "chrome".to_owned(),
        profile: Some("Default".to_owned()),
        label: "Google Chrome".to_owned(),
        domains: vec!["crates.io".to_owned()],
    };
    assert_eq!(sources, [firefox.clone(), chrome.clone()]);

    let mut asked = Vec::new();
    let mut choose = |answer: &str, auto: bool| {
        choose_sign_in_source(
            sources.clone(),
            &["crates.io"],
            |question| {
                asked.push(question.to_owned());
                Ok(answer.to_owned())
            },
            auto,
        )
        .expect("choice")
    };
    assert_eq!(choose("", false), Some(firefox.clone()));
    assert_eq!(choose("2", false), Some(chrome));
    assert_eq!(choose("n", false), None);
    assert_eq!(choose("y", true), Some(firefox));
    assert_eq!(asked.len(), 3, "auto does not ask");
    assert_eq!(
        asked[0],
        "Import your crates.io sign-in from Firefox into the automated profile? [Y/n/1-2] "
    );
    assert_eq!(
        choose_sign_in_source(Vec::new(), &["crates.io"], |_| bail!("asked"), false)
            .expect("no sources"),
        None
    );
}

#[test]
fn imports_the_sign_in_domains_unless_a_named_browser_is_imported_fully() {
    assert_eq!(
        parse_import_scope(None, None).ok(),
        Some(ImportScope::Domains)
    );
    assert_eq!(
        parse_import_scope(None, Some("auto")).ok(),
        Some(ImportScope::Domains)
    );
    assert_eq!(
        parse_import_scope(None, Some("chrome:Work")).ok(),
        Some(ImportScope::Full)
    );
    assert_eq!(
        parse_import_scope(Some("domains"), Some("chrome")).ok(),
        Some(ImportScope::Domains)
    );
    assert_eq!(
        parse_import_scope(Some("everything"), None)
            .expect_err("invalid")
            .to_string(),
        "--browser-import-scope must be full or domains"
    );
}

#[test]
fn signs_out_by_expiring_only_the_sign_in_domain_cookies() {
    let state = json!({
        "cookies": [
            { "name": "session", "domain": ".crates.io", "value": "secret", "expires": 1.9e9 },
            { "name": "cdn", "domain": "static.crates.io", "value": "secret", "expires": -1 },
            { "name": "other", "domain": "evilcrates.io", "value": "keep", "expires": -1 },
            { "name": "gh", "domain": "github.com", "value": "keep", "expires": -1 },
        ],
        "origins": [{ "origin": "https://crates.io", "localStorage": [] }],
    });
    assert_eq!(
        expired_cookies(&state, &["crates.io"]),
        json!({
            "cookies": [
                { "name": "session", "domain": ".crates.io", "value": "", "expires": 1.0 },
                { "name": "cdn", "domain": "static.crates.io", "value": "", "expires": 1.0 },
            ],
            "origins": [],
        })
    );
}

/// Runs the whole first publish, with verbose tracing, in a child process
/// that only the output check below starts.
#[tokio::test]
async fn first_publish_output_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let mut host = FakeHost::new();
    host.me = vec![signed_out(), me(true)];
    sign_in(&mut host, Duration::from_millis(1), Duration::from_secs(5))
        .await
        .expect("signed in");
    first_publish(&mut host, "demo", Some(&publisher()), now())
        .await
        .expect("first publish");
    let mut stuck = FakeHost::new();
    stuck.stuck = true;
    stuck.publish_code = 101;
    let error = first_publish(&mut stuck, "demo", None, now())
        .await
        .expect_err("both fail");
    eprintln!("error: {error:#}");
    println!("child finished");
}

#[test]
fn never_prints_the_token() {
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "crates_api::first_publish_output_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .env_remove(TOKEN_VARIABLE)
        .output()
        .expect("child test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("child finished"), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("Revoked the first-publish token"),
        "{stdout}"
    );
    assert!(
        stderr.contains("revoking through the browser session"),
        "{stderr}"
    );
    assert!(!stdout.contains(TOKEN), "the token was printed: {stdout}");
    assert!(!stderr.contains(TOKEN), "the token was printed: {stderr}");
}
