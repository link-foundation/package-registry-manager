use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use browser_commander::browser::real_browser::{
    launch_real_browser, RealBrowserLaunchResult, RealBrowserOptions,
};
use command_stream::StreamingRunner;
use regex::Regex;
use serde_json::Value;
use tokio::sync::mpsc::unbounded_channel;

use crate::auth_urls::{
    node_options_with_shim, resolve_program, run_interactive, write_tty_shim, CommandOutput,
};
use crate::browser::{npm_prefill_script, open_in_user_browser};
use crate::flows::CLEANUP_CONDITIONS;
use crate::model::{CommandSpec, PlanMode, SetupPlan, SetupStep, StepKind};
use crate::npm_package::{report_pack_warnings, verify_bins, PUBLISH_REJECTED};
use crate::plan::package_directory;
use crate::prerequisites::two_factor_mode;
use crate::profile::{ensure_profile_ignored, protect_legacy_profile};
use crate::registry_state::{npm_trusted, truthy, Endpoints, Lookup, RegistryClient};

const PREFILLED_FORMS: [&str; 2] = ["configure-trusted-publisher", "create-pending-publisher"];
const INTERACTIVE_CHECKS: [&str; 2] = ["check-trust", "verify-trusted-publisher"];

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
#[allow(clippy::struct_excessive_bools)] // These booleans mirror independent CLI switches.
pub struct ExecuteOptions<'a> {
    pub repository: &'a Path,
    /// Where sign-in and approval URLs open.
    pub browser: BrowserMode,
    /// Dedicated automation profile, used to fill forms.
    pub browser_profile: &'a Path,
    pub browser_channel: &'a str,
    pub execute: bool,
    pub yes: bool,
    pub no_browser: bool,
    pub verbose: bool,
    /// Registry API base URLs for lookups and polling.
    pub endpoints: Endpoints,
    /// Delay between registry polls.
    pub poll_interval: Duration,
    /// How long to wait for the registry before failing.
    pub wait_timeout: Duration,
}

/// Run a setup plan. Without `execute` it only reports a dry run. Steps whose
/// `when` condition does not hold are skipped, so a re-run after a partial
/// success resumes where the previous run stopped.
#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
pub async fn execute_plan(plan: &SetupPlan, options: &ExecuteOptions<'_>) -> Result<()> {
    if !plan.package.publishable {
        bail!(
            "{} is not publishable: {}",
            plan.package.name,
            plan.package.problems.join("; ")
        );
    }
    if plan.mode == Some(PlanMode::Complete) {
        println!(
            "{} already publishes through trusted publishing; nothing to do.",
            plan.package.name
        );
        return Ok(());
    }
    if !options.execute {
        println!("Dry run only. Re-run with --execute to run these steps and open the registry.");
        return Ok(());
    }
    protect_legacy_profile(options.repository, options.browser_profile, options.verbose).await?;
    let mut session = Session::new(plan, options);
    let result = session.run().await;
    session.cleanup().await;
    result
}

struct Session<'a> {
    plan: &'a SetupPlan,
    options: &'a ExecuteOptions<'a>,
    client: RegistryClient,
    conditions: BTreeSet<&'static str>,
    values: BTreeMap<String, String>,
    deferred: Vec<&'a SetupStep>,
    browser: Option<RealBrowserLaunchResult>,
    temporary: Option<tempfile::TempDir>,
    shim: Option<(tempfile::TempDir, PathBuf)>,
}

#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
impl<'a> Session<'a> {
    fn new(plan: &'a SetupPlan, options: &'a ExecuteOptions<'a>) -> Self {
        let mut conditions = BTreeSet::from(["verify-release", "release-dispatch"]);
        if plan.package.exists_on_registry == Some(false) {
            conditions.insert("package-missing");
        }
        if plan.package.trusted_publishing != Some(true) {
            conditions.insert("trust-missing");
        }
        Self {
            plan,
            options,
            client: RegistryClient::new(options.endpoints.clone(), options.verbose),
            conditions,
            values: BTreeMap::new(),
            deferred: Vec::new(),
            browser: None,
            temporary: None,
            shim: None,
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
                Some(when) if CLEANUP_CONDITIONS.contains(&when) => self.deferred.push(step),
                Some(when) if !self.holds(when) => {
                    if self.options.verbose {
                        eprintln!("skip {}: condition {when} does not hold", step.id);
                    }
                }
                _ => {
                    println!("==> {}", step.title);
                    self.run_step(step).await?;
                }
            }
        }
        Ok(())
    }

    async fn cleanup(&mut self) {
        for step in std::mem::take(&mut self.deferred) {
            if step.when.as_deref().is_some_and(|when| self.holds(when)) {
                println!("==> {}", step.title);
                match self.run_process(step, true).await {
                    Ok(result) if result.code == 0 => {}
                    Ok(result) => eprintln!("warning: {} exited with {}", step.id, result.code),
                    Err(error) => eprintln!("warning: {} failed: {error:#}", step.id),
                }
            }
        }
        self.browser = None;
        self.temporary = None;
        self.shim = None;
    }

    async fn run_step(&mut self, step: &SetupStep) -> Result<()> {
        match step.kind {
            StepKind::Check if step.command.is_some() => self.check(step).await,
            StepKind::Check if step.id == "verify-bins" => self.verify_bins().await,
            StepKind::Check => self.check_registry().await,
            StepKind::Command => self.command(step).await,
            StepKind::Wait => self.wait(step).await,
            StepKind::Browser => self.browser_step(step).await,
            StepKind::Manual => {
                println!("{}", step.description);
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
        if state.trusted == Some(true) {
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
                let trusted =
                    result.code == 0 && Regex::new(r#""(?:id|type|file)"\s*:"#)?.is_match(output);
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
                if result.code != 0 {
                    eprintln!("warning: could not list repository secrets with gh");
                } else if json_list(output)?
                    .iter()
                    .any(|item| item["name"] == "NPM_TOKEN")
                {
                    println!("  NPM_TOKEN is no longer needed with trusted publishing.");
                    self.conditions.insert("token-secret-present");
                }
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
            match step.id.as_str() {
                "attach-trusted-publisher" => {
                    eprintln!("warning: npm trust failed; falling back to the browser form");
                    self.conditions.insert("trust-cli-failed");
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
        let packed = json_list(output)?
            .into_iter()
            .next()
            .context("npm pack printed no package")?;
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

    async fn wait(&self, step: &SetupStep) -> Result<()> {
        let url = self.expand_text(step.url.as_deref().unwrap_or_default());
        let deadline = Instant::now() + self.options.wait_timeout;
        loop {
            if let Lookup::Found(document) = self.client.get_json(&url).await {
                if truthy(&document)
                    && (step.id != "confirm-provenance" || self.is_trusted_release(&document))
                {
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
        let fill =
            PREFILLED_FORMS.contains(&step.id.as_str()) && self.plan.trusted_publisher.is_some();
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
        let (Some(browser), Some(prefill)) = (&self.browser, &self.plan.trusted_publisher) else {
            return Ok(());
        };
        prompt("Press Enter when the form is visible...")?;
        let script = npm_prefill_script(prefill, false)?;
        let mut result = browser.page.evaluate(&script).await?;
        if result["filled"].as_array().is_some_and(Vec::is_empty) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            result = browser.page.evaluate(&script).await?;
        }
        println!("Prefill result: {result}");
        let submit = self.options.yes
            || is_yes(&prompt(
                "Submit this trusted-publisher configuration? [y/N] ",
            )?);
        if submit {
            let submission = browser
                .page
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
            println!("Opening {url} in your default browser");
            if let Err(error) = open_in_user_browser(url) {
                eprintln!(
                    "warning: could not open your default browser ({error:#}); open the URL yourself"
                );
            }
            return Ok(());
        }
        if self.browser.is_none() {
            ensure_profile_ignored(self.options.browser_profile, self.options.verbose).await?;
            // Boxed: the launch future is large, and every step awaits it.
            self.browser = Some(
                Box::pin(launch_real_browser(
                    RealBrowserOptions::chromiumoxide()
                        .channel(self.options.browser_channel)
                        .user_data_dir(self.options.browser_profile)
                        .headless(false)
                        .verbose(self.options.verbose),
                ))
                .await
                .context("could not launch an installed Chrome-family browser")?,
            );
        }
        if let Some(browser) = &self.browser {
            browser.page.goto(url).await?;
        }
        Ok(())
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
        if self.options.verbose || (result.code != 0 && step.id != "check-sign-in") {
            io::stdout().write_all(result.stdout.to_string().as_bytes())?;
            io::stderr().write_all(result.stderr.to_string().as_bytes())?;
        }
        Ok(CommandOutput {
            code: result.code,
            stdout: result.stdout.to_string(),
            stderr: result.stderr.to_string(),
            legacy_login: false,
        })
    }

    async fn run_process(&mut self, step: &SetupStep, mirror: bool) -> Result<CommandOutput> {
        let (command, cwd) = self.prepare(step)?;
        let mut env = BTreeMap::new();
        let npm = matches!(command.program.as_str(), "npm" | "npx");
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
            run_interactive(&command, &cwd, &env, mirror, Some(sender), npm),
            async {
                while let Some(url) = receiver.recv().await {
                    if let Err(error) = self.open(&url, false).await {
                        eprintln!("warning: could not open {url}: {error:#}");
                    }
                }
            }
        );
        let result = result?;
        if result.legacy_login {
            println!();
            bail!(
                "the browser login was not completed in time, so npm fell back to its legacy \
                 username prompt; re-run the command to get a fresh login link"
            );
        }
        Ok(result)
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

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer)
}

pub use crate::profile::default_browser_profile;
