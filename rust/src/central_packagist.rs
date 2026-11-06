//! Handlers for the generated Central, Packagist and Gradle-toolchain steps.
//!
//! None of them is ever routed to the crates.io session; each returns a
//! structured result whose status is only `verified`, `present`,
//! `configured` or `not-used` when fresh read-back evidence exists.
use crate::github::GithubGateway;
use crate::model::{Registry, SetupPlan, SetupStep};
use crate::registry_state::RegistryClient;
use anyhow::Result;
use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// Generated Central, Packagist, `SwiftPM` and Gradle step ids with their own handlers.
pub const CENTRAL_PACKAGIST_STEPS: [&str; 10] = [
    "ensure-central-credential-pair",
    "verify-namespace",
    "ensure-signing-key",
    "verify-central-publication",
    "verify-packagist-package",
    "packagist-github-hook",
    "packagist-update-api",
    "verify-swift-release",
    "verify-swift-consumer",
    "verify-gradle-toolchain",
];

/// Statuses backed by evidence (or an explicit "not used"); anything else keeps setup incomplete.
pub const COMPLETE_STATUSES: [&str; 4] = ["verified", "present", "configured", "not-used"];

/// Reads the open Central Portal user-token dialog in the page; values stay in memory.
pub fn read_central_pair_script() -> String {
    let source = include_str!("central-token-page.mjs")
        .split_once("export default ")
        .expect("page function")
        .1;
    format!("({source})()")
}

const MISSING_SECRET: &str = r"(?i)secrets? (?:are |is )?missing|missing (?:secrets?|credentials?)|(?:CENTRAL_USERNAME|CENTRAL_TOKEN|GPG_PRIVATE_KEY|GPG_PASSPHRASE)[^\n]*(?:not set|missing|empty)";

/// Everything a generated step reads: the plan, a GitHub gateway, and a
/// registry HTTP client whose endpoints tests can override.
pub struct StepContext<'a, G: GithubGateway> {
    /// GitHub reads and gh-manager commands.
    pub github: &'a G,
    /// Public registry reads.
    pub client: &'a RegistryClient,
    /// The plan being executed.
    pub plan: &'a SetupPlan,
    /// Working directory for gateway calls.
    pub cwd: &'a Path,
    /// Bound for following a restarted run.
    pub wait_timeout: Duration,
}

/// One public HTTP answer; `status` is `None` for a transport failure.
#[derive(Debug, Clone, Default)]
pub struct HttpAnswer {
    /// HTTP status, when a response arrived.
    pub status: Option<u16>,
    /// JSON body of a successful GET.
    pub json: Option<Value>,
    /// Transport failure reason.
    pub reason: Option<String>,
}

impl HttpAnswer {
    pub(crate) fn shown(&self) -> String {
        self.status
            .map_or_else(|| "transport".to_owned(), |status| status.to_string())
    }
}

/// GET (JSON) or HEAD one public URL; never sends credentials.
pub async fn http(client: &RegistryClient, url: &str, head: bool) -> HttpAnswer {
    let request = if head {
        client.client.head(url)
    } else {
        client.client.get(url)
    };
    match request.header("accept", "application/json").send().await {
        Ok(response) => {
            let status = response.status();
            if client.verbose() {
                eprintln!(
                    "{} {url} -> {}",
                    if head { "HEAD" } else { "GET" },
                    status.as_u16()
                );
            }
            let json = if !head && status.is_success() {
                response
                    .bytes()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_slice(&body).ok())
            } else {
                None
            };
            HttpAnswer {
                status: Some(status.as_u16()),
                json,
                reason: None,
            }
        }
        Err(error) => HttpAnswer {
            status: None,
            json: None,
            reason: Some(error.without_url().to_string()),
        },
    }
}

/// Repository setting (secret) names listed in a step's `secrets` detail; never values.
#[must_use]
pub fn step_setting_names(step: &SetupStep) -> Vec<String> {
    step.details
        .get("secrets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["secret"].as_str().map(str::to_owned))
        .collect()
}

/// The plan's GitHub `owner/name`.
#[must_use]
pub fn plan_slug(plan: &SetupPlan) -> Option<String> {
    crate::repository_identity::repository_slug(&plan.repository)
}

/// Which named repository settings exist (repository, then organization scope), from metadata only; values are never read.
#[allow(clippy::future_not_send)]
pub async fn setting_presence(
    github: &impl GithubGateway,
    slug: &str,
    names: &[String],
    cwd: &Path,
) -> Result<(Vec<String>, Vec<String>)> {
    let owner = slug.split('/').next().unwrap_or_default();
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for name in names {
        let mut metadata = github
            .call(
                &[
                    "secret".into(),
                    "get-metadata".into(),
                    name.clone(),
                    "--repo".into(),
                    slug.into(),
                ],
                None,
                cwd,
            )
            .await?;
        if metadata.is_null() {
            // Organization metadata may be inaccessible (403/404/422): absent.
            metadata = github
                .call(
                    &[
                        "secret".into(),
                        "get-metadata".into(),
                        name.clone(),
                        "--org".into(),
                        owner.into(),
                    ],
                    None,
                    cwd,
                )
                .await
                .unwrap_or(Value::Null);
        }
        if metadata.is_null() {
            missing.push(name.clone());
        } else {
            present.push(name.clone());
        }
    }
    Ok((present, missing))
}

pub(crate) fn maven_path(
    client: &RegistryClient,
    group: &str,
    artifact: &str,
    version: &str,
    extension: &str,
) -> String {
    format!(
        "{}/{}/{artifact}/{version}/{artifact}-{version}.{extension}",
        client
            .endpoints()
            .base(Registry::MavenCentral)
            .unwrap_or_default(),
        group.replace('.', "/")
    )
}

/// Runs one generated step other than the Central pair, which needs the
/// session's browser. `prior` holds earlier step results of this plan;
/// `release_verified` records a verified `SwiftPM` release.
#[allow(clippy::future_not_send)]
pub async fn run_generated_step(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
    prior: &[Value],
    release_verified: bool,
) -> Result<Value> {
    match step.id.as_str() {
        "verify-namespace" => Ok(
            json!({"status":"manual-prerequisite","reason":"authenticated Central ownership read-back requires the dedicated browser"}),
        ),
        "ensure-signing-key" => crate::central_signing::ensure(context, step, false).await,
        "verify-central-publication" => Ok(crate::central_publication::verify(context, step).await),
        "verify-packagist-package" => Ok(verify_packagist_package(context).await),
        "packagist-github-hook" => Ok(packagist_hook(step, prior)),
        "packagist-update-api" => packagist_update_api(context, step).await,
        "verify-swift-release" => crate::swift_release::verify_swift_release(context, step).await,
        "verify-swift-consumer" => {
            crate::swift_release::verify_swift_consumer(context, step, release_verified).await
        }
        "verify-gradle-toolchain" => Ok(manual_prerequisite(step)),
        other => Ok(json!({
            "status": "unsupported",
            "reason": format!("no handler for {other}"),
            "stop": true,
        })),
    }
}

fn stable(version: &str) -> bool {
    !Regex::new(r"(?i)^dev-|-dev$|[-.](?:alpha|beta|rc|dev)")
        .expect("static pattern")
        .is_match(version)
}

/// Exact stable tag read-back from Packagist's p2 metadata. Package
/// existence or a dev branch never passes.
#[allow(clippy::future_not_send)]
async fn verify_packagist_package(context: &StepContext<'_, impl GithubGateway>) -> Value {
    let item = &context.plan.package;
    if item.manifest != "composer.json" {
        return json!({
            "status": "unsupported",
            "reason": format!("{} is not the canonical root Composer descriptor Packagist reads", item.manifest),
        });
    }
    let base = context
        .client
        .endpoints()
        .base(Registry::Packagist)
        .unwrap_or_default();
    let tagged = http(
        context.client,
        &format!("{base}/p2/{}.json", item.name),
        false,
    )
    .await;
    match tagged.status {
        Some(404) => {
            return json!({"status":"unregistered","reason":format!("{} is not on Packagist", item.name)});
        }
        Some(status @ (401 | 403)) => {
            return json!({"status":"auth-rejected","reason":format!("HTTP {status}")});
        }
        Some(200) if tagged.json.is_some() => {}
        _ => {
            return json!({
                "status": "transport-unknown",
                "reason": tagged.reason.clone().unwrap_or_else(|| format!("HTTP {}", tagged.shown())),
            });
        }
    }
    let versions: Vec<String> = tagged
        .json
        .as_ref()
        .map_or(&Value::Null, |json| &json["packages"][&item.name])
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["version"].as_str())
        .filter(|version| !version.is_empty() && stable(version))
        .map(str::to_owned)
        .collect();
    if versions.is_empty() {
        let dev = http(
            context.client,
            &format!("{base}/p2/{}~dev.json", item.name),
            false,
        )
        .await;
        let has_dev = dev
            .json
            .as_ref()
            .and_then(|json| json["packages"][&item.name].as_array())
            .is_some_and(|entries| !entries.is_empty());
        return json!({
            "status": if has_dev { "dev-only" } else { "stable-version-missing" },
            "reason": "no stable tagged version",
        });
    }
    let Some(wanted) = item
        .version
        .as_deref()
        .map(|version| version.trim_start_matches('v'))
    else {
        return json!({"status":"unknown","reason":"the expected version is unresolved","versions":versions});
    };
    if versions
        .iter()
        .any(|version| version.trim_start_matches('v') == wanted)
    {
        json!({"status":"verified","version":wanted})
    } else {
        json!({
            "status": "stable-version-missing",
            "reason": format!("{wanted} is not published; latest {}", versions[0]),
        })
    }
}

/// The GitHub hook is invisible to this tool: it stays a prerequisite.
fn packagist_hook(step: &SetupStep, prior: &[Value]) -> Value {
    let exact = prior
        .iter()
        .any(|entry| entry["step"] == "verify-packagist-package" && entry["status"] == "verified");
    json!({
        "status": "manual-prerequisite",
        "reason": format!(
            "confirm the GitHub integration at {}{}",
            step.url.as_deref().unwrap_or("https://packagist.org"),
            if exact { "; the exact tag reached Packagist, but that does not prove the hook" } else { "" }
        ),
    })
}

#[allow(clippy::future_not_send)]
async fn packagist_update_api(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
) -> Result<Value> {
    if step.details.get("state").and_then(Value::as_str) != Some("configured") {
        return Ok(json!({
            "status": "not-used",
            "reason": "the workflow does not call the update-package API; the GitHub hook is the only update path",
        }));
    }
    let names = step_setting_names(step);
    let (present, missing) = match plan_slug(context.plan) {
        Some(slug) => setting_presence(context.github, &slug, &names, context.cwd).await?,
        None => (Vec::new(), names),
    };
    Ok(if missing.is_empty() {
        json!({"status":"configured","secrets":present})
    } else {
        json!({
            "status": "manual-prerequisite",
            "missing": missing,
            "reason": format!("{} missing; add the Packagist API token as encrypted secrets", missing.join(" + ")),
        })
    })
}

/// Where the Central pair stands before any browser opens.
#[derive(Debug)]
pub enum CentralPairStart {
    /// Finished without a browser: unsupported, kept, or a prerequisite.
    Done(Value),
    /// Fields are missing and a domain-scoped automated browser may generate them.
    Generate {
        /// The plan's `owner/name`.
        slug: String,
        /// The two selected secret names, username first.
        names: Vec<String>,
        /// Selected names that already exist (replaced together with the pair).
        present: Vec<String>,
        /// Selected names that are missing.
        missing: Vec<String>,
        /// The Central Portal user-token page.
        url: String,
    },
}

/// Decides the Central pair step from secret metadata alone: kept when both
/// fields exist, a structured prerequisite when automation is not allowed,
/// and never a crates.io call or a browser launch.
#[allow(clippy::future_not_send)]
pub async fn central_pair_preflight(
    github: &impl GithubGateway,
    plan: &SetupPlan,
    step: &SetupStep,
    automation_allowed: bool,
    cwd: &Path,
) -> Result<CentralPairStart> {
    let names = step_setting_names(step);
    let Some(slug) = plan_slug(plan).filter(|_| names.len() == 2) else {
        return Ok(CentralPairStart::Done(json!({
            "status": "unsupported",
            "reason": "the Central pair needs GitHub repository coordinates and exactly two selected secret names",
            "stop": true,
        })));
    };
    let (present, missing) = setting_presence(github, &slug, &names, cwd).await?;
    if missing.is_empty() {
        return Ok(CentralPairStart::Done(json!({
            "status": "present",
            "secrets": present,
            "reason": "existing Central secrets kept; presence does not prove token validity",
            "expiry": central_expiry(github, &slug, &names, cwd).await,
        })));
    }
    let url = step
        .url
        .clone()
        .unwrap_or_else(|| "https://central.sonatype.com/usertoken".into());
    if !automation_allowed {
        return Ok(CentralPairStart::Done(json!({
            "status": "manual-prerequisite",
            "secrets": missing,
            "reason": format!(
                "generate a Central Portal user token at {url} and store {} as encrypted secrets; automation needs a dedicated profile with domain-scoped sign-in import",
                missing.join(" + ")
            ),
            "stop": true,
        })));
    }
    Ok(CentralPairStart::Generate {
        slug,
        names,
        present,
        missing,
        url,
    })
}

/// Stores a matched Central pair (values on stdin, never in argv or logs),
/// reads both names back, and only then retries the causal failed jobs.
#[allow(clippy::future_not_send)]
pub async fn store_central_pair_with_expiry(
    context: &StepContext<'_, impl GithubGateway>,
    slug: &str,
    names: &[String],
    values: [&str; 2],
    expires_at: Option<&str>,
) -> Result<Value> {
    for (name, value) in names.iter().zip(values) {
        let mut args = vec![
            "secret".into(),
            "set".into(),
            name.clone(),
            "--repo".into(),
            slug.into(),
        ];
        if let Some(expiry) = expires_at {
            args.extend(["--expires-at".into(), expiry.into()]);
        }
        context.github.call(&args, Some(value), context.cwd).await?;
    }
    let (_, unread) = setting_presence(context.github, slug, names, context.cwd).await?;
    if !unread.is_empty() {
        return Ok(json!({
            "status": "unverified",
            "secrets": unread,
            "reason": "gh-manager did not read the stored secrets back",
            "stop": true,
        }));
    }
    let retry = retry_causal_jobs(context, slug, names).await?;
    Ok(
        json!({"status":"configured","secrets":names,"retry":retry,"expiry":{"status":if expires_at.is_some() {"server-reported"} else {"unknown"},"expires_at":expires_at}}),
    )
}

/// Pressing Enter is never verification.
#[must_use]
pub fn manual_prerequisite(step: &SetupStep) -> Value {
    println!("{}", step.description);
    if let Some(url) = &step.url {
        println!("  {url}");
    }
    json!({"status":"manual-prerequisite","reason":"no automated read-back evidence"})
}

/// Settings-only repair: restart only the failed main jobs whose logs show
/// the missing Central secrets, then follow them through gh-manager.
#[allow(clippy::future_not_send)]
pub async fn retry_causal_jobs(
    context: &StepContext<'_, impl GithubGateway>,
    slug: &str,
    names: &[String],
) -> Result<Value> {
    let Some(workflow) = context.plan.package.workflow.as_deref() else {
        return Ok(json!({"status":"not-run","reason":"no publishing workflow to retry"}));
    };
    let github = context.github;
    let repository = github
        .rest(&format!("repos/{slug}"), context.cwd)
        .await?
        .unwrap_or_default();
    let branch = repository["default_branch"].as_str().unwrap_or("main");
    let runs = github
        .release_runs(slug, workflow, branch, context.cwd)
        .await?;
    let Some(failed) = runs
        .as_array()
        .into_iter()
        .flatten()
        .filter(|run| run["conclusion"] == "failure")
        .max_by_key(|run| run["id"].as_u64().unwrap_or_default())
    else {
        return Ok(json!({"status":"not-needed","reason":"no failed main run"}));
    };
    let run_id = failed["id"].as_u64().unwrap_or_default();
    let auth = Regex::new(
        &std::iter::once(MISSING_SECRET.to_owned())
            .chain(crate::github::failure_patterns(Some(
                Registry::MavenCentral,
            )))
            .collect::<Vec<_>>()
            .join("|"),
    )?;
    let jobs = github
        .rest(
            &format!("repos/{slug}/actions/runs/{run_id}/jobs?per_page=100"),
            context.cwd,
        )
        .await?
        .unwrap_or_default();
    let mut causal = Vec::new();
    for job in jobs["jobs"].as_array().into_iter().flatten() {
        if job["conclusion"] != "failure" {
            continue;
        }
        let Some(id) = job["id"].as_u64() else {
            continue;
        };
        if let Ok(log) = github.job_log(slug, id, context.cwd).await {
            if auth.is_match(&log) && names.iter().any(|name| log.contains(name.as_str())) {
                causal.push(id);
            }
        }
    }
    if causal.is_empty() {
        return Ok(json!({
            "status": "unknown",
            "reason": "no failed job log names the stored Central secrets; unrelated CI was not restarted",
            "run": failed["html_url"],
        }));
    }
    let request = json!({
        "repo": slug,
        "workflow": workflow,
        "runId": run_id,
        "jobIds": causal,
        "changes": [{"kind":"setting","changed":true,"verified":true}],
        "timeout": u64::try_from(context.wait_timeout.as_millis()).unwrap_or(u64::MAX),
    });
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("repair-evidence.json");
    std::fs::write(&file, serde_json::to_vec(&request)?)?;
    let result = github
        .call(
            &[
                "repair".into(),
                "verify".into(),
                file.to_string_lossy().into_owned(),
                "--timeout".into(),
                context.wait_timeout.as_secs().to_string(),
            ],
            None,
            context.cwd,
        )
        .await?;
    Ok(json!({
        "status": result["status"],
        "run": if result["runUrl"].is_null() { failed["html_url"].clone() } else { result["runUrl"].clone() },
        "jobs": causal,
    }))
}

/// Store a pair with unknown expiry; never calculate an artificial lifetime.
#[allow(clippy::future_not_send)]
pub async fn store_central_pair(
    context: &StepContext<'_, impl GithubGateway>,
    slug: &str,
    names: &[String],
    values: [&str; 2],
) -> Result<Value> {
    store_central_pair_with_expiry(context, slug, names, values, None).await
}

/// Existing credential presence is independent of recorded expiry and validity.
#[allow(clippy::future_not_send)]
async fn central_expiry(
    github: &impl GithubGateway,
    slug: &str,
    names: &[String],
    cwd: &Path,
) -> Value {
    let mut values = Vec::new();
    for name in names {
        let expiry = github
            .call(
                &[
                    "variable".into(),
                    "get".into(),
                    format!("{name}_EXPIRES_AT"),
                    "--repo".into(),
                    slug.into(),
                ],
                None,
                cwd,
            )
            .await
            .unwrap_or(Value::Null);
        values.push(expiry["value"].clone());
    }
    let date = values
        .first()
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    if values.len() == 2 && values[0] == values[1] && date.is_some() {
        json!({"status":"recorded","expires_at":values[0],"expired":date.is_some_and(|d| d <= chrono::Utc::now())})
    } else {
        json!({"status":"unknown","reason":"existing encrypted token lifetime is unreadable; no expiry was invented"})
    }
}
