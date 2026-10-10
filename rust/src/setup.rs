use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use command_stream::StreamingRunner;
use regex::Regex;
use serde_json::Value;
use tokio::sync::mpsc::unbounded_channel;

use crate::approvals::{
    approval_deadline, next_link, LinkKind, APPROVAL_ATTEMPTS, TWO_FACTOR_HINT,
};
use crate::auth_urls::{
    node_options_with_shim, resolve_program, run_interactive, write_tty_shim, CommandOutput,
};
use crate::automation::Automation;
use crate::browser::{npm_prefill_script, open_in_user_browser};
use crate::browser_options::BrowserOptions;
use crate::default_browser::{detect_default_browser, open_with_command};
use crate::flows::CLEANUP_CONDITIONS;
use crate::model::Registry;
use crate::model::{CommandSpec, SetupPlan, SetupStep, StepKind};
use crate::npm_package::{
    lists_trusted_publisher, packed_entry, report_pack_warnings, trust_created, verify_bins,
    PUBLISH_REJECTED,
};
use crate::pages::{pages_change_warning, report_pages, PagesState, PAGES_CHANGES};
use crate::plan::package_directory;
use crate::prerequisites::two_factor_mode;
use crate::registry_state::{npm_trusted, truthy, Endpoints, Lookup, RegistryClient};
use crate::tokens::{
    cargo_home, read_cargo_token, report_token_secrets, TokenState, CRATES_TOKENS_URL,
};

mod batch;
mod crates_session;
mod credential_session;
mod repository_session;
pub use batch::{execute_plan, execute_plans, execute_plans_with};

const PREFILLED_FORMS: [&str; 2] = ["configure-trusted-publisher", "create-pending-publisher"];
const INTERACTIVE_CHECKS: [&str; 2] = ["check-trust", "verify-trusted-publisher"];
/// Checks whose failure is an answer, so their output is not echoed.
const QUIET_CHECKS: [&str; 2] = ["check-sign-in", "check-pages"];

/// Where `--browser` opens URLs that need no automation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum BrowserMode {
    /// The user's default browser, where they are usually already signed in.
    #[default]
    Default,
    /// The dedicated automation profile, as for form filling.
    Automated,
}

/// Options for [`execute_plan`].
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)] // These booleans mirror independent CLI switches.
pub struct ExecuteOptions<'a> {
    pub repository: &'a Path,
    /// Where sign-in and approval URLs open.
    pub browser: BrowserMode,
    /// Dedicated automation profile, used to fill forms.
    pub browser_profile: &'a Path,
    /// How the automated browser launches or attaches.
    pub browser_options: &'a BrowserOptions,
    pub execute: bool,
    pub yes: bool,
    pub no_browser: bool,
    /// Application that opens sign-in and approval links instead of the
    /// default browser (`--open-with`).
    pub open_with: Option<&'a str>,
    /// Keep the npm session after setup instead of signing out (`--keep-session`).
    pub keep_session: bool,
    pub verbose: bool,
    /// Caller-selected registry secret naming template.
    pub secret_name: Option<&'a str>,
    /// Disable browser protocol tracing while credentials may be in memory.
    pub quiet_browser: bool,
    /// Continue account setup after an individual repository fails.
    pub repository_batch: bool,
    /// Selected repositories sharing each registry/secret name during account setup.
    pub secret_repositories: Option<&'a BTreeMap<String, Vec<String>>>,
    /// Registry API base URLs for lookups and polling.
    pub endpoints: Endpoints,
    /// Delay between registry polls.
    pub poll_interval: Duration,
    /// How long to wait for the registry before failing.
    pub wait_timeout: Duration,
}

struct Session<'a> {
    plan: &'a SetupPlan,
    options: &'a ExecuteOptions<'a>,
    client: RegistryClient,
    conditions: BTreeSet<&'static str>,
    values: BTreeMap<String, String>,
    token_secrets: Vec<String>,
    browser: Option<Automation>,
    domains: Vec<&'static str>,
    temporary: Option<tempfile::TempDir>,
    shim: Option<(tempfile::TempDir, PathBuf)>,
    /// The default browser's name, when detected.
    browser_name: Option<&'static str>,
    /// Whether the default browser was already looked up.
    browser_detected: bool,
}

#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
impl<'a> Session<'a> {
    fn new(plan: &'a SetupPlan, options: &'a ExecuteOptions<'a>) -> Self {
        let mut conditions = BTreeSet::from(["verify-release", "release-dispatch"]);
        if plan.package.exists_on_registry == Some(false) {
            conditions.insert("package-missing");
        }
        if plan.package.trusted_publishing != Some(true)
            || plan.mode == Some(crate::model::PlanMode::Repair)
        {
            conditions.insert("trust-missing");
        }
        Self {
            plan,
            options,
            client: RegistryClient::new(options.endpoints.clone(), options.verbose),
            conditions,
            values: BTreeMap::new(),
            token_secrets: Vec::new(),
            browser: None,
            domains: crate::sign_in_import::sign_in_domains(plan.registry).to_vec(),
            temporary: None,
            shim: None,
            browser_name: None,
            browser_detected: false,
        }
    }

    fn holds(&self, condition: &str) -> bool {
        self.conditions.contains(condition)
    }

    fn toggle(&mut self, condition: &'static str, enabled: bool) {
        if enabled {
            self.conditions.insert(condition);
        } else {
            self.conditions.remove(condition);
        }
    }

    async fn run(&mut self) -> Result<()> {
        for step in &self.plan.steps {
            match step.when.as_deref() {
                // cleanup() runs these, even after a failed step.
                Some(when) if CLEANUP_CONDITIONS.contains(&when) => {}
                Some(when) if !self.holds(when) => {
                    if self.options.verbose {
                        eprintln!("skip {}: condition {when} does not hold", step.id);
                    }
                }
                _ => {
                    println!("==> {}", step.title);
                    self.run_step(step).await?;
                    if self.values.contains_key("manifest_pr") {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Runs every cleanup step of the plan whose condition holds, including
    /// those after a step that failed (#24).
    async fn cleanup(&mut self, defer_auth: bool, close_browser: bool) -> Vec<SetupStep> {
        let mut deferred = Vec::new();
        let plan = self.plan;
        let cleanup = plan.steps.iter().filter(|step| {
            step.when
                .as_deref()
                .is_some_and(|when| CLEANUP_CONDITIONS.contains(&when))
        });
        for step in cleanup {
            if defer_auth
                && ["sign-out", "crates-sign-out"].contains(&step.id.as_str())
                && step.when.as_deref().is_some_and(|when| self.holds(when))
            {
                if step.id == "sign-out" && self.keeps_session(step) {
                    println!(
                        "Keeping the npm session (--keep-session); run npm logout to revoke it."
                    );
                } else {
                    deferred.push(step.clone());
                }
            } else if step.id == "sign-out" && self.keeps_session(step) {
                println!(
                    "Keeping the npm session (--keep-session): its token stays in npm's user \
                     configuration until you run npm logout, and the next run reuses it while \
                     npm whoami succeeds."
                );
            } else if step.when.as_deref().is_some_and(|when| self.holds(when)) {
                println!("==> {}", step.title);
                if step.kind == StepKind::Api {
                    if let Err(error) = self.api_step(step).await {
                        eprintln!("warning: {} failed: {error:#}", step.id);
                    }
                    continue;
                }
                match self.run_process(step, true).await {
                    Ok(result) if result.code == 0 => {}
                    Ok(result) => eprintln!("warning: {} exited with {}", step.id, result.code),
                    Err(error) => eprintln!("warning: {} failed: {error:#}", step.id),
                }
            }
        }
        if close_browser {
            if let Some(browser) = self.browser.take() {
                browser.close().await;
            }
        }
        self.temporary = None;
        self.shim = None;
        deferred
    }

    fn keeps_session(&self, step: &SetupStep) -> bool {
        self.options.keep_session
            && step.when.as_deref().is_some_and(|when| self.holds(when))
            && program(step) == "npm"
    }

    async fn run_step(&mut self, step: &SetupStep) -> Result<()> {
        if [
            "check-repository-publisher",
            "verify-repository-publisher",
            "remove-old-publisher",
            "fix-manifest-repository",
        ]
        .contains(&step.id.as_str())
            || (step.id == "rerun-release"
                && self.plan.mode == Some(crate::model::PlanMode::Repair))
        {
            return self.repository_repair_step(step).await;
        }
        match step.kind {
            StepKind::Check if step.id == "check-name-policy" => {
                crate::npm_policy::check_name_policy(
                    &self.plan.package.name,
                    &self
                        .options
                        .endpoints
                        .base(Registry::Npm)
                        .unwrap_or_default(),
                )
                .await
            }
            StepKind::Check if step.command.is_some() => self.check(step).await,
            StepKind::Check if step.id == "verify-bins" => self.verify_bins().await,
            StepKind::Check if step.id == "verify-token-revoked" => {
                self.verify_token_revoked().await
            }
            StepKind::Check => self.check_registry().await,
            StepKind::Command if step.id == "delete-token-secret" => {
                self.delete_token_secrets(step).await
            }
            StepKind::Command => self.command(step).await,
            StepKind::Wait => self.wait(step).await,
            StepKind::Browser => self.browser_step(step).await,
            StepKind::Api if step.id == "manage-registry-token" => self.setup_credential().await,
            StepKind::Api => self.api_step(step).await,
            StepKind::Manual => {
                println!("{}", step.description);
                if step.id == "confirm-oidc-cleanup" {
                    if self.holds("oidc-release-verified") {
                        return Ok(());
                    }
                    if !is_yes(&prompt("Has a new release succeeded through OIDC without a stored registry token? [y/N] ")?) {
                        bail!("a verified OIDC release is required before deleting token secrets");
                    }
                    return Ok(());
                }
                if let Some(url) = &step.url {
                    println!("  {url}");
                }
                prompt("Press Enter when this is done...").map(drop)
            }
        }
    }

    async fn check_registry(&mut self) -> Result<()> {
        let state = self.client.probe_package(&self.plan.package).await;
        let Some(exists) = state.exists else {
            bail!(
                "could not determine whether {} exists on {}",
                self.plan.package.name,
                self.plan.registry
            );
        };
        self.toggle("package-missing", !exists);
        if state.trusted == Some(true) && self.plan.mode != Some(crate::model::PlanMode::Repair) {
            self.conditions.remove("trust-missing");
        }
        if let Some(version) = state.version {
            self.values.insert("previous_version".to_owned(), version);
        }
        if !exists
            && !self
                .plan
                .steps
                .iter()
                .any(|step| step.when.as_deref() == Some("package-missing"))
        {
            bail!(
                "{} is missing from the registry; re-run plan to get the bootstrap steps",
                self.plan.package.name
            );
        }
        println!(
            "{}",
            if exists {
                "  The package exists; attaching trusted publishing."
            } else {
                "  The package is not published yet; bootstrapping it."
            }
        );
        Ok(())
    }

    async fn check(&mut self, step: &SetupStep) -> Result<()> {
        let result = if INTERACTIVE_CHECKS.contains(&step.id.as_str()) {
            self.run_process(step, false).await?
        } else {
            self.capture(step).await?
        };
        let output = result.stdout.as_str();
        match step.id.as_str() {
            "check-sign-in" => self.toggle("signed-out", result.code != 0),
            "check-trust" | "verify-trusted-publisher" => {
                let trusted = result.code == 0
                    && if self.plan.mode == Some(crate::model::PlanMode::Repair) {
                        self.plan.oidc_publisher.as_ref().is_some_and(|expected| {
                            crate::repository_identity::npm_publishers(output)
                                .iter()
                                .any(|publisher| {
                                    crate::repository_identity::publisher_matches(
                                        publisher, expected,
                                    )
                                })
                        })
                    } else {
                        lists_trusted_publisher(output)
                    };
                self.toggle("trust-missing", !trusted);
                if step.id == "verify-trusted-publisher" && !trusted {
                    bail!("npm does not list a trusted publisher for the package");
                }
            }
            "check-2fa" | "verify-2fa" => self.check_two_factor(step, &result)?,
            "inspect-release-run" => self.inspect_release_run(&result)?,
            "read-release-failure" => {
                if result.code != 0 {
                    eprintln!("warning: could not read the failed release log");
                } else if Regex::new(PUBLISH_REJECTED)?.is_match(output) {
                    println!(
                        "  It failed at publish: npm rejected the workflow (E404/invalid-publisher) because no trusted publisher is attached yet. After attaching trust, its failed jobs can be re-run."
                    );
                    self.conditions.insert("release-failed-publish");
                } else {
                    println!(
                        "  It failed for another reason, so trusted publishing alone will not fix it; it is not re-run."
                    );
                }
            }
            "audit-token-secrets" => {
                if result.code == 0 {
                    self.token_secrets = report_token_secrets(output, &self.plan.package)?;
                    self.toggle("token-secret-present", !self.token_secrets.is_empty());
                } else {
                    eprintln!("warning: could not list repository secrets with gh");
                }
            }
            "check-pages" => {
                let state = report_pages(&result, &self.plan.repository);
                self.toggle("pages-missing", state == Some(PagesState::Missing));
                self.toggle("pages-legacy", state == Some(PagesState::Legacy));
            }
            "find-release-run" => {
                let runs = if result.code == 0 {
                    json_list(output)?
                } else {
                    Vec::new()
                };
                let run = runs
                    .first()
                    .ok_or_else(|| anyhow!("no release workflow run was found"))?;
                let id = match &run["databaseId"] {
                    Value::String(id) => id.clone(),
                    other => other.to_string(),
                };
                self.values.insert("run_id".to_owned(), id);
            }
            _ => {
                if result.code != 0 {
                    bail!("{} exited with status {}", program(step), result.code);
                }
            }
        }
        Ok(())
    }

    fn check_two_factor(&mut self, step: &SetupStep, result: &CommandOutput) -> Result<()> {
        let mode = if result.code == 0 {
            two_factor_mode(&result.stdout).ok()
        } else {
            None
        };
        let Some(mode) = mode else {
            if step.id == "verify-2fa" {
                bail!("could not read the npm profile to verify 2FA");
            }
            eprintln!(
                "warning: could not read the npm profile; npm trust requires two-factor authentication"
            );
            return Ok(());
        };
        if let Some(mode) = mode {
            println!("  Two-factor authentication is on ({mode}).");
            self.conditions.remove("tfa-disabled");
            return Ok(());
        }
        if step.id == "verify-2fa" {
            bail!(
                "two-factor authentication is still off; npm trust requires it, so nothing was published"
            );
        }
        println!(
            "  Two-factor authentication is off; npm trust requires it, so turn it on before anything is published."
        );
        self.conditions.insert("tfa-disabled");
        Ok(())
    }

    fn inspect_release_run(&mut self, result: &CommandOutput) -> Result<()> {
        if result.code != 0 {
            eprintln!("warning: could not list the release workflow runs");
            return Ok(());
        }
        let Some(run) = json_list(&result.stdout)?.into_iter().next() else {
            println!("  The release workflow has not run yet.");
            return Ok(());
        };
        let outcome = run["conclusion"]
            .as_str()
            .filter(|text| !text.is_empty())
            .map_or_else(|| json_text(&run["status"]), str::to_owned);
        println!(
            "  The latest release run {outcome}: {}",
            json_text(&run["url"])
        );
        if run["conclusion"] == "failure" {
            self.values
                .insert("failed_run_id".to_owned(), json_text(&run["databaseId"]));
            self.conditions.insert("release-failed");
        }
        Ok(())
    }

    async fn verify_bins(&self) -> Result<()> {
        let pack = self
            .plan
            .steps
            .iter()
            .find(|step| step.id == "pack")
            .context("the plan has no pack step")?;
        let destination = self
            .values
            .get("pack_destination")
            .context("the tarball was not packed")?;
        verify_bins(
            &self.cwd(pack),
            &Path::new(destination).join("install"),
            &self.plan.package.name,
            self.options.verbose,
        )
        .await
    }

    /// Delete each leftover token secret, with one confirmation each.
    async fn delete_token_secrets(&mut self, step: &SetupStep) -> Result<()> {
        for name in self.token_secrets.clone() {
            self.values.insert("token_secret".to_owned(), name);
            self.command(step).await?;
        }
        Ok(())
    }

    /// Confirm that crates.io rejects the first-publish token `cargo login`
    /// stored, asking the maintainer to revoke it and re-checking up to three
    /// times.
    async fn verify_token_revoked(&self) -> Result<()> {
        let token = cargo_home(
            std::env::var_os("CARGO_HOME"),
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
        )
        .and_then(|directory| read_cargo_token(&directory));
        let Some(token) = token else {
            eprintln!(
                "warning: cargo holds no readable crates.io token, so its revocation cannot be verified; check {CRATES_TOKENS_URL}"
            );
            return Ok(());
        };
        let base = self
            .options
            .endpoints
            .base(Registry::CratesIo)
            .unwrap_or_default();
        for attempt in 1..=3 {
            match self.client.crates_token_state(&base, &token).await {
                Some(TokenState::Revoked) => {
                    println!("  crates.io rejects the first-publish token: it is revoked.");
                    return Ok(());
                }
                None => {
                    eprintln!(
                        "warning: crates.io gave no clear answer about the first-publish token; check that it is revoked at {CRATES_TOKENS_URL}"
                    );
                    return Ok(());
                }
                Some(TokenState::Active) if attempt < 3 => {
                    prompt(&format!(
                        "crates.io still accepts the first-publish token. Revoke it at {CRATES_TOKENS_URL}, then press Enter..."
                    ))?;
                }
                Some(TokenState::Active) => {}
            }
        }
        bail!("the first-publish token still authenticates on crates.io; revoke it at {CRATES_TOKENS_URL}")
    }

    async fn command(&mut self, step: &SetupStep) -> Result<()> {
        if step.confirm && !self.options.yes {
            let rendered = render(&self.expand(command_of(step)?));
            let answer = prompt(&format!("Run `{rendered}`? [y/N] "))?;
            if !is_yes(&answer) {
                if step.id == "first-publish" {
                    bail!("the first publish was declined");
                }
                println!("  skipped {}", step.id);
                return Ok(());
            }
        }
        if step.id == "prepare-worktree" {
            let temporary = tempfile::Builder::new().prefix("prm-release-").tempdir()?;
            let destination = temporary.path().to_string_lossy().into_owned();
            let worktree = temporary.path().join("worktree");
            self.values.insert(
                "worktree".to_owned(),
                worktree.to_string_lossy().into_owned(),
            );
            self.values
                .insert("pack_destination".to_owned(), destination);
            self.temporary = Some(temporary);
        }
        // Pack output is captured so npm's warnings can be reviewed.
        let result = if step.id == "pack" {
            self.capture(step).await?
        } else {
            self.run_process(step, true).await?
        };
        if result.code != 0 {
            if step.id == "first-publish"
                && crate::npm_policy::policy_refusal(&format!("{}{}", result.stderr, result.stdout))
            {
                bail!("npm refused this name under its policy; no retry. Choose @owner/name or a longer descriptive name.");
            }
            match step.id.as_str() {
                "attach-trusted-publisher" => {
                    eprintln!("warning: npm trust failed; falling back to the browser form");
                    self.conditions.insert("trust-cli-failed");
                    return Ok(());
                }
                id if PAGES_CHANGES.contains(&id) => {
                    eprintln!("{}", pages_change_warning(&self.plan.repository));
                    return Ok(());
                }
                "trigger-release" => {
                    eprintln!("warning: could not dispatch the workflow; watching its latest run");
                    return Ok(());
                }
                _ => bail!("{} exited with status {}", program(step), result.code),
            }
        }
        match step.id.as_str() {
            "attach-trusted-publisher" if trust_created(&result.stdout) => {
                println!("  npm stored the trusted publisher.");
                self.conditions.remove("trust-missing");
            }
            "sign-in" => {
                self.conditions.insert("tool-signed-in");
            }
            "prepare-worktree" => {
                self.conditions.insert("worktree-created");
            }
            "pack" => {
                report_pack_warnings(&result.stderr);
                self.record_pack(&result.stdout)?;
            }
            "rerun-release" => {
                println!("  Re-running the failed release jobs.");
                if let Some(id) = self.values.get("failed_run_id").cloned() {
                    self.values.insert("run_id".to_owned(), id);
                }
                self.conditions.remove("release-dispatch");
            }
            _ => {}
        }
        Ok(())
    }

    fn record_pack(&mut self, output: &str) -> Result<()> {
        let packed = packed_entry(output)?;
        if packed["name"].as_str() != Some(&self.plan.package.name)
            || self
                .plan
                .package
                .version
                .as_deref()
                .is_some_and(|version| packed["version"].as_str() != Some(version))
        {
            bail!("bootstrap ref package identity/version differs from inspection; inspect the selected ref again before publication");
        }
        for file in packed["files"].as_array().into_iter().flatten() {
            println!(
                "  {:>8}  {}",
                json_text(&file["size"]),
                json_text(&file["path"])
            );
        }
        let filename = json_text(&packed["filename"]);
        println!(
            "  {filename}: {} bytes packed, {} bytes unpacked, {} files",
            json_text(&packed["size"]),
            json_text(&packed["unpackedSize"]),
            json_text(&packed["entryCount"])
        );
        let destination = self
            .values
            .get("pack_destination")
            .map_or_else(|| self.options.repository.to_path_buf(), PathBuf::from);
        self.values.insert(
            "tarball".to_owned(),
            destination.join(&filename).to_string_lossy().into_owned(),
        );
        self.values
            .insert("version".to_owned(), json_text(&packed["version"]));
        Ok(())
    }

    async fn wait(&mut self, step: &SetupStep) -> Result<()> {
        let url = self.expand_text(step.url.as_deref().unwrap_or_default());
        let deadline = Instant::now() + self.options.wait_timeout;
        loop {
            if let Lookup::Found(document) = self.client.get_json(&url).await {
                if truthy(&document)
                    && (step.id != "confirm-provenance" || self.is_trusted_release(&document))
                {
                    if step.id == "confirm-provenance" {
                        self.conditions.insert("oidc-release-verified");
                    }
                    println!("  {url} is ready");
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for {url}");
            }
            tokio::time::sleep(self.options.poll_interval).await;
        }
    }

    fn is_trusted_release(&self, document: &Value) -> bool {
        npm_trusted(document)
            && truthy(&document["dist"]["attestations"])
            && document["version"].as_str()
                != self.values.get("previous_version").map(String::as_str)
    }

    async fn browser_step(&mut self, step: &SetupStep) -> Result<()> {
        if step.confirm && !self.options.yes {
            println!("  {}", step.description);
            if !is_yes(&prompt(&format!("{}? [y/N] ", step.title))?) {
                bail!("the one-time first-publish token was declined; nothing was published");
            }
        }
        let fill =
            PREFILLED_FORMS.contains(&step.id.as_str()) && self.plan.oidc_publisher.is_some();
        self.open(step.url.as_deref().unwrap_or_default(), fill)
            .await?;
        if fill && self.browser.is_some() {
            self.prefill().await?;
        }
        prompt("Finish this step in the browser, then press Enter...")?;
        if step.id == "create-pending-publisher" {
            self.conditions.remove("trust-missing");
        }
        Ok(())
    }

    async fn prefill(&self) -> Result<()> {
        let (Some(browser), Some(prefill)) = (&self.browser, &self.plan.oidc_publisher) else {
            return Ok(());
        };
        prompt("Press Enter when the form is visible...")?;
        let script = npm_prefill_script(prefill, false)?;
        let mut result = browser.evaluate(&script).await?;
        if result["filled"].as_array().is_some_and(Vec::is_empty) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            result = browser.evaluate(&script).await?;
        }
        println!("Prefill result: {result}");
        let submit = self.options.yes
            || is_yes(&prompt(
                "Submit this trusted-publisher configuration? [y/N] ",
            )?);
        if submit {
            let submission = browser
                .evaluate(&npm_prefill_script(prefill, true)?)
                .await?;
            if submission["submitted"] != Value::Bool(true) {
                bail!("the form was not submitted; review the visible browser and submit manually");
            }
        }
        Ok(())
    }

    /// Open a URL in the default browser, or in the automation profile when
    /// `automate` is set (form filling) or `--browser automated` was chosen.
    async fn open(&mut self, url: &str, automate: bool) -> Result<()> {
        if self.options.no_browser {
            println!("Open {url}");
            return Ok(());
        }
        if !automate && self.options.browser == BrowserMode::Default {
            let app = self.options.open_with;
            let label = self.default_browser_label();
            println!("Opening {url} in {}", app.unwrap_or(&label));
            let opened = match app {
                Some(app) => self.open_with(url, app).await,
                None => open_in_user_browser(url).await,
            };
            if let Err(error) = opened {
                eprintln!(
                    "warning: could not open {} ({error:#}); open the URL yourself",
                    app.unwrap_or("your default browser")
                );
            }
            return Ok(());
        }
        self.automated().await?.goto(url).await
    }

    async fn open_with(&self, url: &str, app: &str) -> Result<()> {
        let command = open_with_command(url, app, std::env::consts::OS)?;
        if self.options.verbose {
            eprintln!("+ {}", render(&command));
        }
        // Like the default browser's opener, a slow application keeps running.
        let opener = tokio::spawn(async move {
            StreamingRunner::from_argv(resolve_program(&command.program), &command.args)
                .collect()
                .await
        });
        match tokio::time::timeout(Duration::from_secs(5), opener).await {
            Err(_) => Ok(()),
            Ok(joined) => match joined?? {
                result if result.code == 0 => Ok(()),
                result => bail!("{app} exited with code {}", result.code),
            },
        }
    }

    /// Names the default browser once per run, so maintainers know where to
    /// look. Runs before npm starts, while no link is waiting to be opened.
    async fn detect_default_browser(&mut self) {
        let options = self.options;
        if options.no_browser
            || options.open_with.is_some()
            || options.browser != BrowserMode::Default
        {
            return;
        }
        if !self.browser_detected {
            self.browser_name = detect_default_browser(options.verbose).await;
            self.browser_detected = true;
        }
    }

    fn default_browser_label(&self) -> String {
        self.browser_name.map_or_else(
            || "your default browser".to_owned(),
            |name| format!("{name}, your default browser"),
        )
    }

    fn cwd(&self, step: &SetupStep) -> PathBuf {
        let relative = step
            .cwd
            .as_deref()
            .unwrap_or_else(|| package_directory(&self.plan.package.manifest));
        let expanded = PathBuf::from(self.expand_text(relative));
        if expanded.is_absolute() {
            expanded
        } else {
            self.options.repository.join(expanded)
        }
    }

    fn prepare(&self, step: &SetupStep) -> Result<(CommandSpec, PathBuf)> {
        let command = self.expand(command_of(step)?);
        let cwd = self.cwd(step);
        if self.options.verbose {
            eprintln!("+ (cd {} && {})", cwd.display(), render(&command));
        }
        Ok((command, cwd))
    }

    async fn capture(&self, step: &SetupStep) -> Result<CommandOutput> {
        let (command, cwd) = self.prepare(step)?;
        let result = StreamingRunner::from_argv(resolve_program(&command.program), &command.args)
            .cwd(cwd)
            .collect()
            .await
            .with_context(|| format!("failed to run {}", command.program))?;
        if self.options.verbose || (result.code != 0 && !QUIET_CHECKS.contains(&step.id.as_str())) {
            io::stdout().write_all(result.stdout.to_string().as_bytes())?;
            io::stderr().write_all(result.stderr.to_string().as_bytes())?;
        }
        Ok(CommandOutput {
            code: result.code,
            stdout: result.stdout.to_string(),
            stderr: result.stderr.to_string(),
            legacy_login: false,
            approval_expired: false,
        })
    }

    /// Run a step's command with the terminal. npm's links expire after about
    /// 5 minutes, so an npm step that ends on an expired link is run again for
    /// a fresh one.
    async fn run_process(&mut self, step: &SetupStep, mirror: bool) -> Result<CommandOutput> {
        self.run_process_with(step, mirror, &BTreeMap::new()).await
    }

    /// [`Self::run_process`] with `extra` added to the child's environment
    /// only, never to this process's.
    async fn run_process_with(
        &mut self,
        step: &SetupStep,
        mirror: bool,
        extra: &BTreeMap<String, String>,
    ) -> Result<CommandOutput> {
        let (command, cwd) = self.prepare(step)?;
        let npm = matches!(command.program.as_str(), "npm" | "npx");
        if !npm {
            return self.run_once(&command, &cwd, mirror, false, extra).await;
        }
        self.detect_default_browser().await;
        for attempt in 1.. {
            let result = self.run_once(&command, &cwd, mirror, true, extra).await?;
            match next_link(&result, attempt, APPROVAL_ATTEMPTS)? {
                Some(message) => println!("{message}"),
                None => return Ok(result),
            }
        }
        unreachable!("next_link fails after the last attempt")
    }

    async fn run_once(
        &mut self,
        command: &CommandSpec,
        cwd: &Path,
        mirror: bool,
        npm: bool,
        extra: &BTreeMap<String, String>,
    ) -> Result<CommandOutput> {
        let mut env = extra.clone();
        if npm {
            if self.shim.is_none() {
                self.shim = Some(write_tty_shim()?);
            }
            if let Some((_, shim)) = &self.shim {
                let existing = std::env::var("NODE_OPTIONS").ok();
                env.insert(
                    "NODE_OPTIONS".to_owned(),
                    node_options_with_shim(existing.as_deref(), &shim.to_string_lossy()),
                );
            }
        }
        let (sender, mut receiver) = unbounded_channel();
        let (result, ()) = tokio::join!(
            run_interactive(command, cwd, &env, mirror, Some(sender), npm, npm),
            async {
                while let Some((url, kind)) = receiver.recv().await {
                    println!("{}", approval_deadline(kind, chrono::Local::now().time()));
                    if kind == LinkKind::Approve {
                        println!("{TWO_FACTOR_HINT}");
                    }
                    if let Err(error) = self.open(&url, false).await {
                        eprintln!("warning: could not open {url}: {error:#}");
                    }
                }
            }
        );
        result
    }

    fn expand(&self, command: &CommandSpec) -> CommandSpec {
        CommandSpec {
            program: command.program.clone(),
            args: command
                .args
                .iter()
                .map(|arg| self.expand_text(arg))
                .collect(),
        }
    }

    fn expand_text(&self, text: &str) -> String {
        Regex::new(r"\{(\w+)\}")
            .expect("static pattern must compile")
            .replace_all(text, |captures: &regex::Captures<'_>| {
                self.values
                    .get(&captures[1])
                    .cloned()
                    .unwrap_or_else(|| captures[0].to_owned())
            })
            .into_owned()
    }
}

fn command_of(step: &SetupStep) -> Result<&CommandSpec> {
    step.command
        .as_ref()
        .with_context(|| format!("step {} has no command", step.id))
}

fn program(step: &SetupStep) -> &str {
    step.command
        .as_ref()
        .map_or(step.id.as_str(), |command| command.program.as_str())
}

fn json_list(output: &str) -> Result<Vec<Value>> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str(trimmed).context("the command printed invalid JSON")? {
        Value::Array(items) => Ok(items),
        other => Ok(vec![other]),
    }
}

fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Render an argument vector for display only; it is never passed to a shell.
#[must_use]
pub fn render(command: &CommandSpec) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

pub(crate) fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer)
}

pub use crate::profile::default_browser_profile;
