//! Read-only `SwiftPM` release verification.
//!
//! Tag, tagged commit, tagged
//! manifest, public GitHub release, and the release workflow's consumer gate.
//! Nothing is created or moved; each failure is a distinct state.
use crate::central_packagist::{plan_slug, StepContext};
use crate::github::GithubGateway;
use crate::model::SetupStep;
use crate::registry_state::encode_uri_component;
use crate::swift_distribution::parse_swift_manifest;
use anyhow::Result;
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};

const CONSUMER_STEP: &str = r"(?i)consumer|resolves?\b|verify\b.*\b(?:swift|release)";

#[allow(clippy::future_not_send)]
async fn file_at(
    github: &impl GithubGateway,
    slug: &str,
    file: &str,
    reference: &str,
    cwd: &std::path::Path,
) -> Result<Option<String>> {
    let path = file
        .split('/')
        .map(encode_uri_component)
        .collect::<Vec<_>>()
        .join("/");
    let body = github
        .rest(
            &format!(
                "repos/{slug}/contents/{path}?ref={}",
                encode_uri_component(reference)
            ),
            cwd,
        )
        .await?;
    let Some(content) = body.as_ref().and_then(|body| body["content"].as_str()) else {
        return Ok(None);
    };
    let packed: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    Ok(base64::engine::general_purpose::STANDARD
        .decode(packed)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

/// Reads the exact `SwiftPM` release back from GitHub.
///
/// The exact tag, the commit it names (whose version source must hold
/// the version), the tagged `Package.swift` with its library products, and
/// a public GitHub release. Returns `missing-tag`, `wrong-commit`,
/// `manifest-missing`, `manifest-incorrect`, `release-missing` or `verified`.
#[allow(clippy::future_not_send, clippy::too_many_lines)]
pub async fn verify_swift_release(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
) -> Result<Value> {
    let item = &context.plan.package;
    let facts = item.audit.get("swift_package").cloned().unwrap_or_default();
    let tag = step
        .details
        .get("tag")
        .and_then(Value::as_str)
        .or_else(|| facts["tag"].as_str())
        .map(str::to_owned);
    let slug = plan_slug(context.plan);
    let (Some(tag), Some(slug)) = (tag.clone(), slug.clone()) else {
        return Ok(json!({
            "status": "unknown",
            "reason": if tag.is_none() {
                "the Swift version source is unresolved, so the expected tag is unknown"
            } else {
                "no GitHub repository identity"
            },
        }));
    };
    let (github, cwd) = (context.github, context.cwd);
    let Some(reference) = github
        .rest(
            &format!("repos/{slug}/git/ref/tags/{}", encode_uri_component(&tag)),
            cwd,
        )
        .await?
    else {
        return Ok(json!({
            "status": "missing-tag",
            "tag": tag,
            "reason": format!("the bare tag {tag} does not exist; the release workflow creates it at the gated commit"),
        }));
    };
    let mut commit = reference["object"]["sha"].as_str().map(str::to_owned);
    if reference["object"]["type"] == "tag" {
        let annotated = github
            .rest(
                &format!("repos/{slug}/git/tags/{}", commit.unwrap_or_default()),
                cwd,
            )
            .await?
            .unwrap_or_default();
        commit = annotated["object"]["sha"].as_str().map(str::to_owned);
    }
    let commit = commit.unwrap_or_default();
    if let Some(expected) = step.details.get("commit").and_then(Value::as_str) {
        if expected != commit {
            return Ok(json!({
                "status": "wrong-commit",
                "tag": tag,
                "commit": commit,
                "reason": format!("the tag names {commit}, not {expected}; existing tags are never moved"),
            }));
        }
    }
    if let Some(source) = item.audit.get("version_source").and_then(Value::as_str) {
        let declared = file_at(github, &slug, source, &commit, cwd)
            .await?
            .map(|text| text.trim().trim_start_matches('v').to_owned());
        if declared.as_deref() != Some(tag.trim_start_matches('v')) {
            return Ok(json!({
                "status": "wrong-commit",
                "tag": tag,
                "commit": commit,
                "reason": format!(
                    "{source} at the tagged commit declares {}, not {tag}; existing tags are never moved",
                    declared.as_deref().unwrap_or("nothing")
                ),
            }));
        }
    }
    let Some(manifest) = file_at(github, &slug, &item.manifest, &commit, cwd).await? else {
        return Ok(json!({
            "status": "manifest-missing",
            "tag": tag,
            "commit": commit,
            "reason": format!("{} is absent at the tagged commit", item.manifest),
        }));
    };
    let parsed = parse_swift_manifest(&manifest);
    let products: Vec<String> = facts["products"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    let absent = products.iter().any(|name| !parsed.libraries.contains(name));
    if parsed.name.as_deref() != Some(item.name.as_str()) || absent {
        return Ok(json!({
            "status": "manifest-incorrect",
            "tag": tag,
            "commit": commit,
            "reason": format!(
                "the tagged Package.swift declares {} with products {}; expected {} with {}",
                parsed.name.as_deref().unwrap_or("no package"),
                if parsed.libraries.is_empty() { "none".to_owned() } else { parsed.libraries.join(", ") },
                item.name,
                products.join(", ")
            ),
        }));
    }
    let release = github
        .rest(
            &format!("repos/{slug}/releases/tags/{}", encode_uri_component(&tag)),
            cwd,
        )
        .await?;
    match release {
        Some(release) if release["draft"] != true => Ok(json!({
            "status": "verified",
            "tag": tag,
            "commit": commit,
            "url": release["html_url"],
        })),
        release => Ok(json!({
            "status": "release-missing",
            "tag": tag,
            "commit": commit,
            "reason": if release.is_some() {
                "the GitHub release is still a draft".to_owned()
            } else {
                format!("no public GitHub release for {tag}")
            },
        })),
    }
}

/// Reads the exact-version consumer gate of the release workflow.
///
/// The latest completed main run's consumer step is the only evidence. Skipped,
/// failed, missing or unreadable steps keep the release unverified.
#[allow(clippy::future_not_send)]
pub async fn verify_swift_consumer(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
    release_verified: bool,
) -> Result<Value> {
    if !release_verified {
        return Ok(json!({
            "status": "manual-prerequisite",
            "reason": "the exact tag and release are unverified, so no consumer can resolve them",
        }));
    }
    let item = &context.plan.package;
    let (Some(workflow), Some(slug)) = (item.workflow.as_deref(), plan_slug(context.plan)) else {
        return Ok(json!({"status":"manual-prerequisite","reason":step.description}));
    };
    let (github, cwd) = (context.github, context.cwd);
    let repository = github
        .rest(&format!("repos/{slug}"), cwd)
        .await?
        .unwrap_or_default();
    let branch = repository["default_branch"].as_str().unwrap_or("main");
    let runs = github.release_runs(&slug, workflow, branch, cwd).await?;
    let Some(run) = runs
        .as_array()
        .into_iter()
        .flatten()
        .filter(|run| run["status"] == "completed")
        .max_by_key(|run| run["id"].as_u64().unwrap_or_default())
    else {
        return Ok(
            json!({"status":"consumer-unknown","reason":format!("no completed {workflow} run")}),
        );
    };
    let callee = item
        .workflow_callees
        .last()
        .and_then(|chain| chain.rsplit(" -> ").next())
        .map(str::to_owned);
    let jobs = github
        .rest(
            &format!(
                "repos/{slug}/actions/runs/{}/jobs?per_page=100",
                run["id"].as_u64().unwrap_or_default()
            ),
            cwd,
        )
        .await?
        .unwrap_or_default();
    let pattern = Regex::new(CONSUMER_STEP)?;
    let check = jobs["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|job| {
            callee.as_deref().is_none_or(|callee| {
                let name = job["name"].as_str().unwrap_or_default();
                name == callee || name.ends_with(&format!("/ {callee}"))
            })
        })
        .flat_map(|job| job["steps"].as_array().into_iter().flatten())
        .find(|entry| pattern.is_match(entry["name"].as_str().unwrap_or_default()));
    let Some(check) = check else {
        return Ok(json!({
            "status": "consumer-unknown",
            "run": run["html_url"],
            "reason": "the run has no readable consumer step",
        }));
    };
    if check["conclusion"] == "success" {
        return Ok(json!({"status":"verified","run":run["html_url"],"step":check["name"]}));
    }
    let outcome = check["conclusion"]
        .as_str()
        .or_else(|| check["status"].as_str())
        .unwrap_or("unknown");
    Ok(json!({
        "status": if check["conclusion"] == "skipped" { "consumer-skipped" } else { "consumer-failed" },
        "run": run["html_url"],
        "step": check["name"],
        "reason": format!("the consumer step {outcome}"),
    }))
}
