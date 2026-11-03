//! Session side of the generated Central, Packagist, `SwiftPM` and Gradle
//! steps: dispatch, the Central user-token browser flow, and recording.
//! None of these steps ever reaches the crates.io session.
use super::{is_yes, prompt, Result, Session, SetupStep, Value};
use crate::browser_options::ImportScope;
use crate::central_packagist::{
    plan_slug, retry_causal_jobs, run_generated_step, secret_presence, step_secrets, StepContext,
    CENTRAL_PACKAGIST_STEPS, COMPLETE_STATUSES, READ_CENTRAL_PAIR,
};
use crate::github::{GhManager, GithubGateway};
use serde_json::json;

/// Whether a generated step has its own handler here.
pub(super) fn handles(step: &SetupStep) -> bool {
    CENTRAL_PACKAGIST_STEPS.contains(&step.id.as_str())
}

#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
impl Session<'_> {
    const fn step_context(&self) -> StepContext<'_, GhManager> {
        StepContext {
            github: &GhManager,
            client: &self.client,
            plan: self.plan,
            cwd: self.options.repository,
            wait_timeout: self.options.wait_timeout,
        }
    }

    /// Runs one generated step and records its structured result. Results
    /// without evidence keep setup incomplete; `stop` ends the plan.
    pub(super) async fn run_central_packagist_step(&mut self, step: &SetupStep) -> Result<()> {
        let result = if step.id == "ensure-central-credential-pair" {
            self.ensure_central_pair(step).await?
        } else {
            let release_verified = self.holds("swift-release-verified");
            run_generated_step(
                &self.step_context(),
                step,
                &self.step_results,
                release_verified,
            )
            .await?
        };
        if step.id == "verify-swift-release" && result["status"] == "verified" {
            self.conditions.insert("swift-release-verified");
        }
        self.record_generated(step, result);
        Ok(())
    }

    fn record_generated(&mut self, step: &SetupStep, mut result: Value) {
        let status = result["status"].as_str().unwrap_or("unknown").to_owned();
        let reason = result["reason"].as_str().map(str::to_owned);
        let stops = result["stop"] == true;
        if let Some(object) = result.as_object_mut() {
            object.remove("stop");
            object.insert("step".into(), json!(step.id));
        }
        println!(
            "{}: {status}{}",
            step.id,
            reason.map_or(String::new(), |reason| format!(" ({reason})"))
        );
        if !COMPLETE_STATUSES.contains(&status.as_str()) {
            self.incomplete.push(result.clone());
        }
        self.step_results.push(result);
        if stops {
            self.stopped = true;
        }
    }

    /// Missing Central secrets are generated once in a domain-scoped
    /// automated browser after explicit confirmation, stored as a matched
    /// pair, read back, and then only the causal failed jobs are retried.
    async fn ensure_central_pair(&mut self, step: &SetupStep) -> Result<Value> {
        let names = step_secrets(step);
        let slug = plan_slug(self.plan).filter(|_| names.len() == 2);
        let Some(slug) = slug else {
            return Ok(json!({
                "status": "unsupported",
                "reason": "the Central pair needs GitHub repository coordinates and exactly two selected secret names",
                "stop": true,
            }));
        };
        let cwd = self.options.repository;
        let (present, missing) = secret_presence(&GhManager, &slug, &names, cwd).await?;
        if missing.is_empty() {
            return Ok(json!({
                "status": "present",
                "secrets": present,
                "reason": "existing Central secrets kept; nothing rotated",
            }));
        }
        let browser = self.options.browser_options;
        let url = step
            .url
            .as_deref()
            .unwrap_or("https://central.sonatype.com/usertoken");
        if self.options.no_browser
            || browser.attach.is_some()
            || browser.import_scope == ImportScope::Full
        {
            return Ok(json!({
                "status": "manual-prerequisite",
                "secrets": missing,
                "reason": format!(
                    "generate a Central Portal user token at {url} and store {} as encrypted secrets; automation needs a dedicated profile with domain-scoped sign-in import",
                    missing.join(" + ")
                ),
                "stop": true,
            }));
        }
        let replaces = if present.is_empty() {
            String::new()
        } else {
            format!(
                " A Central token is a matched pair, so {} is replaced too.",
                present.join(" + ")
            )
        };
        if !self.options.yes
            && !is_yes(&prompt(&format!(
                "Generate one Central Portal user token and store {} for {slug}?{replaces} [y/N] ",
                names.join(" + ")
            ))?)
        {
            return Ok(json!({
                "status": "interrupted",
                "secrets": missing,
                "reason": "Central user-token creation declined",
                "stop": true,
            }));
        }
        let page = self.visible_automated().await?;
        page.goto(url).await?;
        prompt("Generate the user token in this browser and leave its username and password dialog open, then press Enter (never paste the values)...")?;
        let pair = self
            .visible_automated()
            .await?
            .evaluate(READ_CENTRAL_PAIR)
            .await?;
        let (Some(username), Some(password)) = (
            pair["username"].as_str().filter(|value| !value.is_empty()),
            pair["password"].as_str().filter(|value| !value.is_empty()),
        ) else {
            return Ok(json!({
                "status": "manual-prerequisite",
                "secrets": missing,
                "reason": "the Central token dialog was not readable; nothing was stored, so the pair is still missing",
                "stop": true,
            }));
        };
        // The pair is matched: both values are written together after the
        // explicit confirmation above, never one silently. Values go on stdin.
        for (name, value) in names.iter().zip([username, password]) {
            GhManager
                .call(
                    &[
                        "secret".into(),
                        "set".into(),
                        name.clone(),
                        "--repo".into(),
                        slug.clone(),
                    ],
                    Some(value),
                    cwd,
                )
                .await?;
        }
        let (_, unread) = secret_presence(&GhManager, &slug, &names, cwd).await?;
        if !unread.is_empty() {
            return Ok(json!({
                "status": "unverified",
                "secrets": unread,
                "reason": "gh-manager did not read the stored secrets back",
                "stop": true,
            }));
        }
        self.conditions.insert("central-pair-stored");
        let retry = retry_causal_jobs(&self.step_context(), &slug, &names).await?;
        Ok(json!({"status":"configured","secrets":names,"retry":retry}))
    }
}
