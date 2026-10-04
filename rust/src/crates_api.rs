//! The crates.io first publish through the crates.io API (#26).
//!
//! The automated browser holds the crates.io session, a short-lived token
//! limited to one crate is created from inside the page, handed only to the
//! `cargo publish` child process, used to attach the trusted publisher, then
//! revoked and the revocation verified. The token never reaches the DOM, the terminal, the
//! clipboard, or cargo's credentials file.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde_json::{json, Value};

use crate::model::TrustedPublisherPrefill;
use crate::tokens::{TokenState, CRATES_TOKENS_URL};

/// Where the automated browser signs in to crates.io.
pub const CRATES_IO_URL: &str = "https://crates.io/";
/// Where a crates.io account verifies its email address.
pub const CRATES_PROFILE_URL: &str = "https://crates.io/settings/profile";
/// How long the first-publish token lives.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// The only endpoints the first-publish token may call.
pub const TOKEN_SCOPES: [&str; 2] = ["publish-new", "trusted-publishing"];
/// How long the sign-in waits for the maintainer.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The variable that hands the token to `cargo publish`.
pub const TOKEN_VARIABLE: &str = "CARGO_REGISTRY_TOKEN";

/// The `PUT /api/v1/me/tokens` body: one crate, two endpoints, one hour.
#[must_use]
pub fn token_request(crate_name: &str, now: DateTime<Utc>) -> Value {
    let expires = now + chrono::Duration::from_std(TOKEN_LIFETIME).unwrap_or_default();
    json!({
        "api_token": {
            "name": format!("prm-first-publish-{crate_name}"),
            "crate_scopes": [crate_name],
            "endpoint_scopes": TOKEN_SCOPES,
            "expired_at": expires.to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    })
}

/// The `POST /api/v1/trusted_publishing/github_configs` body.
#[must_use]
pub fn github_config_request(crate_name: &str, publisher: &TrustedPublisherPrefill) -> Value {
    json!({
        "github_config": {
            "crate": crate_name,
            "repository_owner": publisher.organization,
            "repository_name": publisher.repository,
            "workflow_filename": publisher.workflow,
            "environment": publisher.environment.as_deref().filter(|name| !name.is_empty()),
        }
    })
}

/// A script that calls the crates.io API from inside the page with its
/// session cookie and returns `{ status, body }` to the tool, never to the DOM.
#[must_use]
pub fn page_fetch_script(method: &str, path: &str, body: Option<&Value>) -> String {
    let mut init = json!({
        "method": method,
        "credentials": "same-origin",
        "headers": { "accept": "application/json" },
    });
    if let Some(body) = body {
        init["headers"]["content-type"] = json!("application/json");
        init["body"] = json!(body.to_string());
    }
    format!(
        "(async () => {{\n  const response = await fetch({}, {init});\n  const body = await response.json().catch(() => null);\n  return {{ status: response.status, body }};\n}})()",
        json!(path)
    )
}

/// Clicks crates.io's "Log in with GitHub" button; returns whether it found one.
pub const LOGIN_CLICK_SCRIPT: &str = r#"(() => {
  const button = [...document.querySelectorAll("button, a")].find((element) =>
    /log\s*in with github/i.test(element.textContent || ""),
  );
  if (button) {
    button.click();
  }
  return Boolean(button);
})()"#;

/// The first error detail of a crates.io answer.
#[must_use]
pub fn error_detail(body: &Value) -> String {
    match &body["errors"][0]["detail"] {
        Value::Null => "no error detail".to_owned(),
        Value::String(detail) => detail.clone(),
        other => other.to_string(),
    }
}

/// The question asked once before the token is created.
#[must_use]
pub fn first_publish_question(
    crate_name: &str,
    version: Option<&str>,
    publisher: Option<&TrustedPublisherPrefill>,
) -> String {
    let release = version.map_or_else(
        || crate_name.to_owned(),
        |version| format!("{crate_name} v{version}"),
    );
    let attach = publisher
        .map(|publisher| format!(", attach {} as trusted publisher", publisher.workflow))
        .unwrap_or_default();
    format!(
        "Create a 1-hour token limited to crate {crate_name} with publish-new + trusted-publishing, publish {release}{attach}, revoke the token? [y/N] "
    )
}

/// Reads `version` from the `[package]` table of a Cargo.toml, or `None`
/// when it is inherited from the workspace.
#[must_use]
pub fn crate_version(directory: &Path) -> Option<String> {
    let text = std::fs::read_to_string(directory.join("Cargo.toml")).unwrap_or_default();
    let header = Regex::new(r"^\s*\[([^\]]+)\]").expect("static pattern must compile");
    let version =
        Regex::new(r#"^\s*version\s*=\s*["']([^"']+)["']"#).expect("static pattern must compile");
    let mut table = String::new();
    for line in text.lines() {
        if let Some(captures) = header.captures(line) {
            captures[1].trim().clone_into(&mut table);
        } else if let Some(captures) = version.captures(line).filter(|_| table == "package") {
            return Some(captures[1].to_owned());
        }
    }
    None
}

/// The token while it lives: never printed, and its bytes zeroed on drop.
pub struct Secret(String);

impl Secret {
    /// The token, for the requests that need it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(<redacted>)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&bytes);
    }
}

/// What the crates.io page and the tool do for the API steps; the setup
/// session drives the automated browser, and tests replace it.
#[allow(async_fn_in_trait)] // Used only within this crate and its tests.
pub trait CratesHost {
    /// Whether to trace the API calls (`--verbose`); never the token.
    fn verbose(&self) -> bool {
        false
    }
    /// Evaluates `script` in the crates.io page and returns its value.
    async fn evaluate(&mut self, script: &str) -> Result<Value>;
    /// Opens `url` in the page.
    async fn goto(&mut self, url: &str) -> Result<()>;
    /// Runs `cargo publish` with `env` added to the child's environment only;
    /// returns its exit code.
    async fn publish(&mut self, env: &BTreeMap<String, String>) -> Result<i32>;
    /// Waits until the registry shows the crate.
    async fn wait_for_registry(&mut self) -> Result<()>;
    /// Calls the crates.io API with the token from the tool, never from the
    /// page; status 0 when the request failed.
    async fn token_call(
        &mut self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> (u16, Value);
    /// Asks crates.io whether the token still authenticates.
    async fn token_state(&mut self, token: &str) -> Option<TokenState>;
}

/// The crates.io session of the page, from `GET /api/v1/me`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CratesSession {
    pub signed_in: bool,
    pub login: Option<String>,
    pub email_verified: bool,
    pub email_sent: bool,
}

/// Reads the crates.io session of the page from `GET /api/v1/me`.
#[allow(clippy::future_not_send)] // The browser adapter is intentionally !Sync.
pub async fn read_session(host: &mut impl CratesHost) -> Result<CratesSession> {
    let answer = host
        .evaluate(&page_fetch_script("GET", "/api/v1/me", None))
        .await?;
    let user = &answer["body"]["user"];
    Ok(CratesSession {
        signed_in: answer["status"] == 200,
        login: user["login"].as_str().map(str::to_owned),
        email_verified: user["email_verified"] == true,
        email_sent: user["email_verification_sent"] == true,
    })
}

/// Signs the automated browser in to crates.io.
///
/// An existing session is kept; otherwise "Log in with GitHub" is clicked and
/// `/me` polled until it answers 200. crates.io lets only accounts with a verified email publish, so an
/// unverified one stops here with the settings link.
#[allow(clippy::future_not_send)] // The browser adapter is intentionally !Sync.
pub async fn sign_in(
    host: &mut impl CratesHost,
    interval: Duration,
    timeout: Duration,
) -> Result<CratesSession> {
    host.goto(CRATES_IO_URL).await?;
    let mut session = read_session(host).await?;
    if !session.signed_in {
        println!(
            "  Not signed in to crates.io; finish the GitHub sign-in in the automated browser."
        );
        if host.evaluate(LOGIN_CLICK_SCRIPT).await? != Value::Bool(true) {
            println!("  Click \"Log in with GitHub\" on crates.io.");
        }
        let deadline = Instant::now() + timeout;
        while !session.signed_in {
            if Instant::now() > deadline {
                bail!("timed out waiting for the crates.io sign-in");
            }
            tokio::time::sleep(interval).await;
            session = read_session(host).await?;
        }
    }
    println!(
        "  Signed in to crates.io as {}.",
        session.login.as_deref().unwrap_or("your account")
    );
    if !session.email_verified {
        let sent = if session.email_sent {
            " A verification email was sent; follow its link,"
        } else {
            " Add and verify an email address"
        };
        bail!(
            "crates.io only lets accounts with a verified email address publish or attach trusted publishers.{sent} at {CRATES_PROFILE_URL}, then re-run setup"
        );
    }
    Ok(session)
}

/// Attaches the trusted publisher with the page's session cookie, for a crate
/// that already exists or when the token could not. Returns whether it worked.
#[allow(clippy::future_not_send)] // The browser adapter is intentionally !Sync.
pub async fn attach_with_session(
    host: &mut impl CratesHost,
    crate_name: &str,
    publisher: &TrustedPublisherPrefill,
) -> Result<bool> {
    let answer = host
        .evaluate(&page_fetch_script(
            "POST",
            "/api/v1/trusted_publishing/github_configs",
            Some(&github_config_request(crate_name, publisher)),
        ))
        .await?;
    Ok(report_attach(
        u16::try_from(answer["status"].as_u64().unwrap_or_default()).unwrap_or_default(),
        &answer["body"],
    ))
}

fn report_attach(status: u16, body: &Value) -> bool {
    if (200..300).contains(&status) {
        println!("  crates.io stored the trusted publisher.");
        return true;
    }
    eprintln!(
        "warning: crates.io did not attach the trusted publisher ({status}: {})",
        error_detail(body)
    );
    false
}

/// Publishes a crate for the first time through the crates.io API.
///
/// The token is revoked whatever happens, and the run fails while crates.io
/// still accepts it. Returns whether the trusted publisher was attached.
#[allow(clippy::future_not_send)] // The browser adapter is intentionally !Sync.
pub async fn first_publish(
    host: &mut impl CratesHost,
    crate_name: &str,
    publisher: Option<&TrustedPublisherPrefill>,
    now: DateTime<Utc>,
) -> Result<bool> {
    let mut created = host
        .evaluate(&page_fetch_script(
            "PUT",
            "/api/v1/me/tokens",
            Some(&token_request(crate_name, now)),
        ))
        .await?;
    let status = created["status"].as_u64().unwrap_or_default();
    // The token lives only in `secret`, whose bytes are zeroed once revoked.
    let secret = match created["body"]["api_token"]["token"].take() {
        Value::String(token) if status == 200 && !token.is_empty() => Secret(token),
        _ => bail!(
            "crates.io did not create the first-publish token ({status}: {}); nothing was published",
            error_detail(&created["body"])
        ),
    };
    let token_id = created["body"]["api_token"]["id"].clone();
    println!("  Created a 1-hour first-publish token; it stays in this process only.");
    let outcome = publish_and_attach(host, crate_name, publisher, &secret).await;
    let revoked = revoke(host, &secret, &token_id).await;
    drop(secret);
    match (outcome, revoked) {
        (Ok(attached), Ok(())) => Ok(attached),
        (Err(failure), Ok(())) => Err(failure),
        (Ok(_), Err(error)) => Err(error),
        (Err(failure), Err(error)) => Err(anyhow!("{error:#}; the run failed before: {failure:#}")),
    }
}

#[allow(clippy::future_not_send)] // The browser adapter is intentionally !Sync.
async fn publish_and_attach(
    host: &mut impl CratesHost,
    crate_name: &str,
    publisher: Option<&TrustedPublisherPrefill>,
    secret: &Secret,
) -> Result<bool> {
    let mut env = BTreeMap::from([(TOKEN_VARIABLE.to_owned(), secret.expose().to_owned())]);
    let code = host.publish(&env).await;
    if let Some(copy) = env.remove(TOKEN_VARIABLE) {
        drop(Secret(copy));
    }
    let code = code?;
    if code != 0 {
        bail!("cargo exited with status {code}");
    }
    host.wait_for_registry().await?;
    let Some(publisher) = publisher else {
        return Ok(false);
    };
    let (status, body) = host
        .token_call(
            "POST",
            "/trusted_publishing/github_configs",
            secret.expose(),
            Some(&github_config_request(crate_name, publisher)),
        )
        .await;
    Ok(report_attach(status, &body))
}

/// Revokes the token with `DELETE /api/v1/tokens/current`, falls back to the
/// page session, and verifies that crates.io rejects it.
#[allow(clippy::future_not_send)] // The browser adapter is intentionally !Sync.
async fn revoke(host: &mut impl CratesHost, secret: &Secret, token_id: &Value) -> Result<()> {
    let (status, _) = host
        .token_call("DELETE", "/tokens/current", secret.expose(), None)
        .await;
    let mut state = host.token_state(secret.expose()).await;
    if state != Some(TokenState::Revoked) && !token_id.is_null() {
        if host.verbose() {
            eprintln!("token revocation answered {status}; revoking through the browser session");
        }
        let id = match token_id {
            Value::String(id) => id.clone(),
            other => other.to_string(),
        };
        let _ = host
            .evaluate(&page_fetch_script(
                "DELETE",
                &format!("/api/v1/me/tokens/{id}"),
                None,
            ))
            .await;
        state = host.token_state(secret.expose()).await;
    }
    match state {
        Some(TokenState::Revoked) => {
            println!("  Revoked the first-publish token; crates.io rejects it now.");
            Ok(())
        }
        None => {
            eprintln!(
                "warning: crates.io gave no clear answer about the first-publish token; check that it is revoked at {CRATES_TOKENS_URL}"
            );
            Ok(())
        }
        Some(TokenState::Active) => bail!(
            "the first-publish token still authenticates on crates.io; revoke it at {CRATES_TOKENS_URL}"
        ),
    }
}
