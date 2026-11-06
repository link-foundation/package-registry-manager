//! Pinned gh-manager CLI transport for the Rust port. Secrets use stdin only.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{path::Path, process::Stdio};
use tokio::{io::AsyncWriteExt, process::Command};

/// Source release used until the scoped npm package is published.
pub const GH_MANAGER_PACKAGE: &str = "https://codeload.github.com/link-foundation/gh-manager/tar.gz/cab12f0404d1bc90c72eacea533ce3d5ae8f4894";

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

    /// The commit a branch points at, so every scan read uses one immutable
    /// tree. `None` keeps reads on the branch (unsupported or unreadable).
    async fn head_commit(&self, _slug: &str, _branch: &str, _cwd: &Path) -> Option<String> {
        None
    }

    /// One read-only GitHub REST document; `None` is a 404. Transports
    /// without REST access report the read as unsupported.
    async fn rest(&self, endpoint: &str, _cwd: &Path) -> Result<Option<Value>> {
        anyhow::bail!("GitHub REST read {endpoint} is unsupported by this gateway")
    }

    /// Explicit REST mutation; request bodies use stdin, never shell interpolation.
    async fn mutate(
        &self,
        _method: &str,
        _endpoint: &str,
        _body: Option<&Value>,
        _cwd: &Path,
    ) -> Result<Value> {
        anyhow::bail!("GitHub REST mutations are unsupported by this gateway")
    }

    /// The plain-text log of one Actions job, never printed by callers.
    async fn job_log(&self, slug: &str, job: u64, _cwd: &Path) -> Result<String> {
        anyhow::bail!("job log {slug}#{job} is unsupported by this gateway")
    }
}

/// Resolves the pinned package through npm; no ambient gh-manager is required.
pub struct GhManager;
impl GithubGateway for GhManager {
    async fn mutate(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<&Value>,
        cwd: &Path,
    ) -> Result<Value> {
        let mut command = Command::new("gh");
        command
            .args(["api", "--method", method, endpoint])
            .current_dir(cwd);
        if body.is_some() {
            command.args(["--input", "-"]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            if let Some(body) = body {
                stdin.write_all(body.to_string().as_bytes()).await?;
            }
        }
        let result = child.wait_with_output().await?;
        if !result.status.success() {
            bail!("GitHub {method} request failed; no credentials were logged");
        }
        if result.stdout.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&result.stdout)?)
    }
    async fn rest(&self, endpoint: &str, cwd: &Path) -> Result<Option<Value>> {
        let (status, body, _) = github_api(endpoint.trim_start_matches('/'), cwd).await?;
        match status {
            200..=299 => Ok(Some(body)),
            404 => Ok(None),
            _ => anyhow::bail!("GitHub HTTP {status} for {endpoint}"),
        }
    }

    async fn job_log(&self, slug: &str, job: u64, cwd: &Path) -> Result<String> {
        let output = Command::new("gh")
            .args(["api", &format!("repos/{slug}/actions/jobs/{job}/logs")])
            .current_dir(cwd)
            .output()
            .await?;
        if !output.status.success() {
            anyhow::bail!("job log {slug}#{job} is unavailable");
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn head_commit(&self, slug: &str, branch: &str, cwd: &Path) -> Option<String> {
        let branch = crate::registry_state::encode_uri_component(branch);
        let (status, body, _) = github_api(&format!("repos/{slug}/commits/{branch}"), cwd)
            .await
            .ok()?;
        let sha = body["sha"].as_str().filter(|_| status == 200)?;
        (sha.len() == 40
            && sha
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| sha.to_owned())
    }
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
            let endpoint = format!("repos/{slug}/actions/workflows/{workflow}/runs");
            let mut filtered = true;
            let mut page = 1;
            loop {
                let mut url = reqwest::Url::parse(&format!("https://api.github.com/{endpoint}"))?;
                url.query_pairs_mut().extend_pairs([
                    ("event", event),
                    ("per_page", if filtered { "1" } else { "100" }),
                    ("page", &page.to_string()),
                ]);
                if filtered {
                    url.query_pairs_mut().append_pair("branch", branch);
                }
                let endpoint = format!(
                    "{}?{}",
                    url.path().trim_start_matches('/'),
                    url.query().unwrap_or_default()
                );
                let (status, document, next) = github_api(&endpoint, cwd).await?;
                if status == 404 && filtered {
                    filtered = false;
                    continue;
                }
                if status != 200 {
                    bail!("could not list the release workflow runs (HTTP {status})");
                }
                let matching: Vec<_> = document["workflow_runs"]
                    .as_array()
                    .context("invalid release runs list")?
                    .iter()
                    .filter(|run| run["head_branch"] == branch && run["event"] == event)
                    .cloned()
                    .collect();
                let found = !matching.is_empty();
                runs.extend(matching);
                if filtered || found || !next {
                    break;
                }
                page += 1;
            }
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
        let read = input.is_none()
            && matches!(
                args.get(1).map(String::as_str),
                Some("list" | "files" | "logs" | "view")
            );
        let rate_limited = regex::Regex::new(r"(?i)(?:HTTP|GitHub[^\n]*?)\s*(?:403|429)\b")?;
        let after = regex::Regex::new(r"(?i)retry-after:\s*([^\r\n]+)")?;
        let expired = regex::Regex::new(r"GitHub log download \d{3}")?;
        for attempt in 0..4 {
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
                let diagnostic = String::from_utf8_lossy(&result.stderr);
                if read && attempt < 3 && rate_limited.is_match(&diagnostic) {
                    let capture = after.captures(&diagnostic);
                    tokio::time::sleep(crate::request_policy::retry_delay(
                        capture
                            .as_ref()
                            .and_then(|value| value.get(1))
                            .map(|value| value.as_str()),
                        attempt,
                    ))
                    .await;
                    continue;
                }
                if input.is_none() {
                    if let Some(message) = expired.find(&String::from_utf8_lossy(&result.stderr)) {
                        bail!("{}", message.as_str());
                    }
                }
                bail!("gh-manager operation failed; check GitHub authentication and permissions (output withheld)");
            }
            return serde_json::from_slice(&result.stdout)
                .context("gh-manager returned invalid JSON (output withheld)");
        }
        unreachable!("bounded read attempts return a result")
    }
}

// gh supplies authentication; headers remain available for rate-limit backoff.
async fn github_api(endpoint: &str, cwd: &Path) -> Result<(u16, Value, bool)> {
    for attempt in 0..4 {
        let output = Command::new("gh")
            .args(["api", "--include", endpoint])
            .current_dir(cwd)
            .output()
            .await?;
        let response = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
        let (headers, body) = response
            .split_once("\n\n")
            .context("GitHub returned no response headers")?;
        let status = headers
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .context("GitHub returned no HTTP status")?;
        let header = |name: &str| {
            headers.lines().find_map(|line| {
                line.split_once(':')
                    .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.trim())
            })
        };
        if matches!(status, 403 | 429) && attempt < 3 {
            let mut wait = crate::request_policy::retry_delay(header("retry-after"), attempt);
            if let Some(reset) =
                header("x-ratelimit-reset").and_then(|value| value.parse::<i64>().ok())
            {
                wait = wait.max(std::time::Duration::from_secs(
                    u64::try_from((reset - chrono::Utc::now().timestamp()).max(0))
                        .unwrap_or_default(),
                ));
            }
            tokio::time::sleep(wait).await;
            continue;
        }
        return Ok((
            status,
            serde_json::from_str(body).unwrap_or(Value::Null),
            header("link").is_some_and(|value| value.contains("rel=\"next\"")),
        ));
    }
    unreachable!("bounded API attempts return a result")
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
