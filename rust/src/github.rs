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

    /// Latest default-branch workflow runs; transports can bound each eligible event.
    async fn release_runs(
        &self,
        slug: &str,
        workflow: &str,
        branch: &str,
        cwd: &Path,
    ) -> Result<Value> {
        self.call(
            &[
                "runs".into(),
                "list".into(),
                slug.into(),
                "--workflow".into(),
                workflow.into(),
                "--branch".into(),
                branch.into(),
            ],
            None,
            cwd,
        )
        .await
    }
}

/// Resolves the pinned package through npm; no ambient gh-manager is required.
pub struct GhManager;
impl GithubGateway for GhManager {
    async fn release_runs(
        &self,
        slug: &str,
        workflow: &str,
        branch: &str,
        cwd: &Path,
    ) -> Result<Value> {
        let mut runs = Vec::new();
        for event in ["push", "workflow_dispatch"] {
            let workflow = crate::registry_state::encode_uri_component(workflow);
            let mut url = reqwest::Url::parse(&format!(
                "https://api.github.com/repos/{slug}/actions/workflows/{workflow}/runs"
            ))?;
            url.query_pairs_mut().extend_pairs([
                ("branch", branch),
                ("event", event),
                ("per_page", "1"),
            ]);
            let endpoint = format!(
                "{}?{}",
                url.path().trim_start_matches('/'),
                url.query().unwrap_or_default()
            );
            let output = Command::new("gh")
                .args(["api", &endpoint])
                .current_dir(cwd)
                .output()
                .await?;
            if !output.status.success() {
                bail!("could not list the release workflow runs");
            }
            let document: Value = serde_json::from_slice(&output.stdout)?;
            runs.extend(
                document["workflow_runs"]
                    .as_array()
                    .context("invalid release runs list")?
                    .iter()
                    .cloned(),
            );
        }
        Ok(Value::Array(runs))
    }

    async fn call(&self, args: &[String], input: Option<&str>, cwd: &Path) -> Result<Value> {
        let node = Command::new("node")
            .arg("--version")
            .output()
            .await
            .context("Node.js 22.12 or newer and npm are required for gh-manager")?;
        let version = String::from_utf8_lossy(&node.stdout).trim().to_owned();
        if !node.status.success() || !crate::prerequisites::supports_node(&version) {
            bail!("package-registry-manager needs Node.js 22.12 or newer (found {version})");
        }
        let mut command = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" });
        command.args(["--yes", "--package", GH_MANAGER_PACKAGE, "gh-manager"]);
        let mut child = command
            .args(args)
            .arg("--json")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
            if input.is_none() {
                if let Some(message) = regex::Regex::new(r"GitHub log download \d{3}")?
                    .find(&String::from_utf8_lossy(&result.stderr))
                {
                    bail!("{}", message.as_str());
                }
            }
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
