//! Pinned gh-manager CLI transport for the Rust port. Secrets use stdin only.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{path::Path, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command};

/// Source release used until the scoped npm package is published.
pub const GH_MANAGER_PACKAGE: &str = "https://codeload.github.com/link-foundation/gh-manager/tar.gz/29808e088ac8fd8374361920bbebab07a4d63ac6";

/// GitHub command transport, injectable for deterministic discovery tests.
#[allow(async_fn_in_trait)]
pub trait GithubGateway {
    /// Execute exact arguments and return structured, nonsensitive output.
    async fn call(&self, args: &[String], input: Option<&str>, cwd: &Path) -> Result<Value>;
}

/// Resolves the pinned package through npm; no ambient gh-manager is required.
pub struct GhManager;
impl GithubGateway for GhManager {
    async fn call(&self, args: &[String], input: Option<&str>, cwd: &Path) -> Result<Value> {
        let mut command = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" });
        command.args(["--yes", "--package", GH_MANAGER_PACKAGE, "gh-manager"]);
        let mut child = command
            .args(args)
            .arg("--json")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("Node.js and npm are required to run the pinned gh-manager CLI")?;
        if let Some(mut stdin) = child.stdin.take() {
            if let Some(value) = input {
                stdin
                    .write_all(value.as_bytes())
                    .await
                    .context("cannot send secret to gh-manager")?;
            }
        }
        let result = child.wait_with_output().await?;
        if !result.status.success() {
            bail!("gh-manager operation failed; check GitHub authentication and permissions (output withheld)");
        }
        serde_json::from_slice(&result.stdout)
            .context("gh-manager returned invalid JSON (output withheld)")
    }
}

/// Registry-supplied patterns, shared with the JavaScript port.
#[must_use]
pub fn failure_patterns(registry: Option<crate::Registry>) -> Vec<String> {
    let document: Value = serde_json::from_str(include_str!("publishing-policy.json"))
        .expect("checked registry policies");
    if let Some(registry) = registry {
        return document[registry.to_string()]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
    }
    document
        .as_object()
        .expect("policy map")
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect()
}
