//! Long-lived registry tokens.
//!
//! Covers the repository secrets that trusted publishing replaces, and the
//! one-time crates.io first-publish token, whose revocation is verified
//! against the crates.io API before setup ends.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

use crate::model::{Package, SetupStep, StepKind};
use crate::publishers::registry_token_secrets;

/// Where the crates.io first-publish token is revoked.
pub const CRATES_TOKENS_URL: &str = "https://crates.io/settings/tokens";

/// Steps that list the repository's secrets and delete the long-lived
/// registry tokens trusted publishing makes unnecessary, one confirmation each.
#[must_use]
pub fn token_secret_steps(package: &Package, slug: &str) -> [SetupStep; 2] {
    let names = registry_token_secrets(package.registry).join(", ");
    [
        SetupStep::new(
            "audit-token-secrets",
            "Look for leftover long-lived token secrets",
            StepKind::Check,
            format!("Trusted publishing makes long-lived tokens such as {names} unnecessary. Organization secrets are not listed here; an organization owner removes them with gh secret delete --org."),
        )
        .command("gh", &["secret", "list", "--repo", slug, "--json", "name"])
        .cwd("."),
        SetupStep::new(
            "delete-token-secret",
            "Delete the leftover token secrets",
            StepKind::Command,
            "Remove each long-lived token secret that no workflow reads any more.",
        )
        .command("gh", &["secret", "delete", "{token_secret}", "--repo", slug])
        .when("token-secret-present")
        .cwd(".")
        .confirmed(),
    ]
}

/// The registry token secrets a repository holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenSecretAudit {
    /// Secrets a workflow still reads; trusted publishing must replace them first.
    pub in_use: Vec<String>,
    /// Secrets no workflow reads, which can be deleted.
    pub leftover: Vec<String>,
}

/// Split the registry token secrets in `gh secret list --json name` output
/// into the ones a workflow still reads and leftovers that can be deleted.
///
/// # Errors
///
/// Fails when the output is not a JSON array.
pub fn audit_token_secrets(output: &str, package: &Package) -> Result<TokenSecretAudit> {
    let listed: Vec<Value> = if output.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(output)?
    };
    let mut audit = TokenSecretAudit::default();
    for name in registry_token_secrets(package.registry) {
        if listed.iter().any(|item| item["name"] == *name) {
            let list = if package.token_secrets.iter().any(|read| read == name) {
                &mut audit.in_use
            } else {
                &mut audit.leftover
            };
            list.push((*name).to_owned());
        }
    }
    Ok(audit)
}

/// Print which registry token secrets remain and return the leftovers, the
/// ones no workflow reads any more and that can be deleted.
///
/// # Errors
///
/// Fails when the output is not a JSON array.
pub fn report_token_secrets(output: &str, package: &Package) -> Result<Vec<String>> {
    let audit = audit_token_secrets(output, package)?;
    for name in &audit.in_use {
        println!(
            "  {name} is still read by a workflow; switch that workflow to trusted publishing before deleting it."
        );
    }
    for name in &audit.leftover {
        println!("  {name} is no longer needed with trusted publishing.");
    }
    Ok(audit.leftover)
}

/// Return cargo's home directory, as cargo resolves it: `CARGO_HOME`, else
/// `.cargo` in the home directory.
#[must_use]
pub fn cargo_home(cargo_home: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    cargo_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".cargo")))
}

/// Read the crates.io token `cargo login` stored, or `None` when cargo keeps
/// it elsewhere (a credential provider) or holds none.
#[must_use]
pub fn read_cargo_token(directory: &Path) -> Option<String> {
    ["credentials.toml", "credentials"]
        .iter()
        .find_map(|name| registry_token(&fs::read_to_string(directory.join(name)).ok()?))
}

/// Extract `token` from the `[registry]` table of cargo's credentials.
#[must_use]
pub fn registry_token(contents: &str) -> Option<String> {
    let header = Regex::new(r"^\s*\[([^\]]+)\]\s*(?:#.*)?$").expect("valid header pattern");
    let value =
        Regex::new(r#"^\s*token\s*=\s*(?:"([^"]*)"|'([^']*)')"#).expect("valid token pattern");
    let mut table = String::new();
    for line in contents.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(captures) = header.captures(line) {
            captures[1].trim().clone_into(&mut table);
            continue;
        }
        if table == "registry" {
            if let Some(captures) = value.captures(line) {
                let token = captures.get(1).or_else(|| captures.get(2))?;
                return Some(token.as_str().to_owned());
            }
        }
    }
    None
}

/// How crates.io answered a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenState {
    /// crates.io no longer knows the token.
    Revoked,
    /// The token still authenticates.
    Active,
}

/// Classify the crates.io answer to a token on an endpoint only the website
/// session may use; `None` when the answer is unclear.
#[must_use]
pub fn token_state(status: u16, body: &Value) -> Option<TokenState> {
    let detail = body["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .map(|error| match &error["detail"] {
                    Value::String(text) => text.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
        .to_lowercase();
    if matches!(status, 401 | 403)
        && (detail.contains("authentication failed")
            || detail.contains("does not match the format"))
    {
        Some(TokenState::Revoked)
    } else if (200..300).contains(&status)
        || detail.contains("only be performed on the crates.io website")
    {
        Some(TokenState::Active)
    } else {
        None
    }
}
