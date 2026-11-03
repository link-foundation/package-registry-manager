//! Handlers for the generated Central, Packagist and Gradle-toolchain steps.
//!
//! None of them is ever routed to the crates.io session; each returns a
//! structured result whose status is only `verified`, `present`,
//! `configured` or `not-used` when fresh read-back evidence exists.
use crate::github::GithubGateway;
use crate::model::{Registry, SetupPlan, SetupStep};
use crate::registry_state::{encode_uri_component, RegistryClient};
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
pub const READ_CENTRAL_PAIR: &str = r##"(() => {
  const root = document.querySelector('[role="dialog"]') ?? document.body;
  const text = (element) => (element?.value ?? element?.textContent ?? "").trim();
  const field = (label) => {
    for (const node of root.querySelectorAll("label, dt, th, span, p, div")) {
      if (node.children.length === 0 && new RegExp("^" + label + ":?$", "i").test(text(node))) {
        const target = node.htmlFor ? root.querySelector("#" + CSS.escape(node.htmlFor)) : null;
        const next = target ?? node.nextElementSibling ?? node.parentElement?.nextElementSibling;
        const value = text(next?.querySelector?.("input, code, pre") ?? next);
        if (value && !/\s/.test(value)) return value;
      }
    }
    return null;
  };
  return { username: field("username"), password: field("password") };
})()"##;

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
    fn shown(&self) -> String {
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

/// Secret names of a step's `secrets` detail.
#[must_use]
pub fn step_secrets(step: &SetupStep) -> Vec<String> {
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

/// Selected secret presence at repository, then organization scope.
#[allow(clippy::future_not_send)]
pub async fn secret_presence(
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

fn maven_path(
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
        "verify-namespace" => Ok(verify_namespace(context, step).await),
        "ensure-signing-key" => ensure_signing_key(context, step).await,
        "verify-central-publication" => Ok(verify_central_publication(context, step).await),
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

#[allow(clippy::future_not_send)]
/// Published artifacts under the namespace prove it was verified; nothing else does.
async fn verify_namespace(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
) -> Value {
    let Some(namespace) = step.details.get("namespace").and_then(Value::as_str) else {
        return json!({"status":"manual-prerequisite","reason":"the package coordinates name no namespace"});
    };
    let search = context.client.endpoints().named(
        "PACKAGE_REGISTRY_MANAGER_MAVEN_SEARCH",
        "https://search.maven.org",
    );
    let query = encode_uri_component(&format!("g:\"{namespace}\""));
    let response = http(
        context.client,
        &format!("{search}/solrsearch/select?q={query}&rows=1&wt=json"),
        false,
    )
    .await;
    if response
        .json
        .as_ref()
        .and_then(|json| json["response"]["numFound"].as_u64())
        .is_some_and(|found| found > 0)
    {
        return json!({"status":"verified","namespace":namespace,"evidence":"artifacts are published under the namespace"});
    }
    json!({
        "status": "manual-prerequisite",
        "namespace": namespace,
        "reason": format!(
            "no fresh read-back proves {namespace} is verified; confirm it at {}",
            step.url.as_deref().unwrap_or("https://central.sonatype.com/publishing/namespaces")
        ),
    })
}

/// Signing works only when a published signature exists; presence is not proof.
#[allow(clippy::future_not_send)]
async fn ensure_signing_key(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
) -> Result<Value> {
    let names = step_secrets(step);
    let (present, missing) = match plan_slug(context.plan) {
        Some(slug) => secret_presence(context.github, &slug, &names, context.cwd).await?,
        None => (Vec::new(), names),
    };
    let package = &context.plan.package;
    if let (Some((group, artifact)), Some(version)) =
        (package.name.split_once(':'), package.version.as_deref())
    {
        let signature = http(
            context.client,
            &maven_path(context.client, group, artifact, version, "pom.asc"),
            true,
        )
        .await;
        if signature.status == Some(200) {
            return Ok(
                json!({"status":"verified","secrets":present,"evidence":"the exact release has a published signature"}),
            );
        }
    }
    Ok(json!({
        "status": "manual-prerequisite",
        "secrets": present,
        "missing": missing,
        "reason": if missing.is_empty() {
            "signing secrets are present, but no signed release proves the key is valid and published".to_owned()
        } else {
            format!("{} missing; provide an armored key whose public key is on a keyserver", missing.join(" + "))
        },
    }))
}

#[allow(clippy::future_not_send)]
/// Root POM, Gradle module metadata and every referenced platform coordinate.
async fn verify_central_publication(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
) -> Value {
    let coordinates = step.details.get("coordinates").cloned().unwrap_or_default();
    let (Some(group), Some(artifact), Some(version)) = (
        coordinates["group"].as_str(),
        coordinates["artifact"].as_str(),
        coordinates["version"].as_str(),
    ) else {
        return json!({"status":"unknown","reason":"the exact coordinates are unresolved"});
    };
    let client = context.client;
    let root = http(
        client,
        &maven_path(client, group, artifact, version, "pom"),
        true,
    )
    .await;
    if root.status != Some(200) {
        return json!({
            "status": if root.status == Some(404) { "unpublished" } else { "transport-unknown" },
            "reason": format!("root POM HTTP {}", root.shown()),
        });
    }
    let multiplatform = context
        .plan
        .package
        .audit
        .get("gradle")
        .is_some_and(|gradle| gradle["multiplatform"] == true);
    let referenced: Vec<String> = if multiplatform {
        let module = http(
            client,
            &maven_path(client, group, artifact, version, "module"),
            false,
        )
        .await;
        let Some(document) = module.json.as_ref() else {
            return json!({
                "status": "partial",
                "reason": format!(
                    "Gradle module metadata missing (HTTP {}); the root POM alone certifies no variant",
                    module.shown()
                ),
            });
        };
        document["variants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|variant| {
                let module = &variant["available-at"]["module"];
                Some(format!(
                    "{}:{}",
                    module["group"].as_str()?,
                    module["module"].as_str()?
                ))
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut wanted: Vec<String> = step
        .details
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.as_str().map(str::to_owned))
        .chain(referenced.iter().cloned())
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    wanted.retain(|entry| seen.insert(entry.clone()));
    let mut missing = Vec::new();
    for coordinates in wanted {
        if multiplatform && !referenced.contains(&coordinates) {
            missing.push(format!("{coordinates} (not in module metadata)"));
            continue;
        }
        let (variant_group, variant_artifact) =
            coordinates.split_once(':').unwrap_or((&coordinates, ""));
        let pom = http(
            client,
            &maven_path(client, variant_group, variant_artifact, version, "pom"),
            true,
        )
        .await;
        if pom.status != Some(200) {
            missing.push(format!("{coordinates} (HTTP {})", pom.shown()));
        }
    }
    if missing.is_empty() {
        json!({"status":"verified","variants":referenced})
    } else {
        json!({"status":"partial","missing":missing,"reason":"platform publications are missing"})
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
    let names = step_secrets(step);
    let (present, missing) = match plan_slug(context.plan) {
        Some(slug) => secret_presence(context.github, &slug, &names, context.cwd).await?,
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
    let names = step_secrets(step);
    let Some(slug) = plan_slug(plan).filter(|_| names.len() == 2) else {
        return Ok(CentralPairStart::Done(json!({
            "status": "unsupported",
            "reason": "the Central pair needs GitHub repository coordinates and exactly two selected secret names",
            "stop": true,
        })));
    };
    let (present, missing) = secret_presence(github, &slug, &names, cwd).await?;
    if missing.is_empty() {
        return Ok(CentralPairStart::Done(json!({
            "status": "present",
            "secrets": present,
            "reason": "existing Central secrets kept; nothing rotated",
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
pub async fn store_central_pair(
    context: &StepContext<'_, impl GithubGateway>,
    slug: &str,
    names: &[String],
    values: [&str; 2],
) -> Result<Value> {
    for (name, value) in names.iter().zip(values) {
        context
            .github
            .call(
                &[
                    "secret".into(),
                    "set".into(),
                    name.clone(),
                    "--repo".into(),
                    slug.into(),
                ],
                Some(value),
                context.cwd,
            )
            .await?;
    }
    let (_, unread) = secret_presence(context.github, slug, names, context.cwd).await?;
    if !unread.is_empty() {
        return Ok(json!({
            "status": "unverified",
            "secrets": unread,
            "reason": "gh-manager did not read the stored secrets back",
            "stop": true,
        }));
    }
    let retry = retry_causal_jobs(context, slug, names).await?;
    Ok(json!({"status":"configured","secrets":names,"retry":retry}))
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
