//! Session side of the generated Central, Packagist, `SwiftPM` and Gradle
//! steps: dispatch, the Central user-token browser flow, and recording.
//! None of these steps ever reaches the crates.io session.
use super::{is_yes, prompt, Result, Session, SetupStep, Value};
use crate::browser_options::ImportScope;
use crate::central_packagist::{
    central_pair_preflight, run_generated_step, store_central_pair, CentralPairStart, StepContext,
    CENTRAL_PACKAGIST_STEPS, COMPLETE_STATUSES, READ_CENTRAL_PAIR,
};
use crate::github::GhManager;
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
        let browser = self.options.browser_options;
        let automation_allowed = !self.options.no_browser
            && browser.attach.is_none()
            && browser.import_scope != ImportScope::Full;
        let start = central_pair_preflight(
            &GhManager,
            self.plan,
            step,
            automation_allowed,
            self.options.repository,
        )
        .await?;
        let (slug, names, present, missing, url) = match start {
            CentralPairStart::Done(result) => return Ok(result),
            CentralPairStart::Generate {
                slug,
                names,
                present,
                missing,
                url,
            } => (slug, names, present, missing, url),
        };
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
        page.goto(&url).await?;
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
        // explicit confirmation above, never one silently.
        let result =
            store_central_pair(&self.step_context(), &slug, &names, [username, password]).await?;
        if result["status"] == "configured" {
            self.conditions.insert("central-pair-stored");
        }
        Ok(result)
    }
}
