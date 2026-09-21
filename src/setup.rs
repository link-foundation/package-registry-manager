use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use browser_commander::browser::real_browser::{launch_real_browser, RealBrowserOptions};
use command_stream::StreamingRunner;

use crate::browser::npm_prefill_script;
use crate::model::{CommandSpec, Registry, SetupPlan};
use crate::plan::package_directory;

#[allow(clippy::struct_excessive_bools)] // These booleans mirror independent CLI switches.
pub struct ExecuteOptions<'a> {
    pub repository: &'a Path,
    pub browser_profile: &'a Path,
    pub browser_channel: &'a str,
    pub execute: bool,
    pub yes: bool,
    pub no_browser: bool,
    pub verbose: bool,
}

/// Run validation commands and guide the authenticated browser step.
#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
pub async fn execute_plan(plan: &SetupPlan, options: &ExecuteOptions<'_>) -> Result<()> {
    if !plan.package.publishable {
        bail!(
            "{} is not publishable: {}",
            plan.package.name,
            plan.package.problems.join("; ")
        );
    }
    if !options.execute {
        println!("Dry run only. Re-run with --execute to perform checks and open the registry.");
        return Ok(());
    }

    let package_root = options
        .repository
        .join(package_directory(&plan.package.manifest));
    for step in &plan.steps {
        if let Some(command) = &step.command {
            run_command(command, &package_root, options.verbose).await?;
        }
    }

    let Some(browser_step) = plan.steps.iter().find(|step| step.url.is_some()) else {
        return Ok(());
    };
    if options.no_browser {
        println!("Open {}", browser_step.url.as_deref().unwrap_or_default());
        return Ok(());
    }
    let url = browser_step
        .url
        .as_deref()
        .context("browser step has no URL")?;
    let browser = launch_real_browser(
        RealBrowserOptions::chromiumoxide()
            .channel(options.browser_channel)
            .user_data_dir(options.browser_profile)
            .headless(false)
            .verbose(options.verbose),
    )
    .await
    .context("could not launch an installed Chrome-family browser")?;
    browser.page.goto(url).await?;
    println!("Sign in and navigate to the setup form in the opened browser.");
    wait_for_enter("Press Enter when the form is visible...")?;

    if plan.registry == Registry::Npm {
        let prefill = plan
            .trusted_publisher
            .as_ref()
            .context("npm setup needs GitHub owner, repository, and release workflow metadata")?;
        let first = npm_prefill_script(prefill, false)?;
        let mut result = browser.page.evaluate(&first).await?;
        if result
            .get("filled")
            .and_then(serde_json::Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            tokio::time::sleep(Duration::from_millis(500)).await;
            result = browser.page.evaluate(&first).await?;
        }
        println!("Prefill result: {result}");
        let submit = options.yes || confirm("Submit this trusted-publisher configuration? [y/N] ")?;
        if submit {
            let result = browser
                .page
                .evaluate(&npm_prefill_script(prefill, true)?)
                .await?;
            if !result
                .get("submitted")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                bail!("the form was not submitted; review the visible browser and submit manually");
            }
        }
    }
    wait_for_enter("Review the registry result, then press Enter to close the managed browser...")?;
    Ok(())
}

async fn run_command(command: &CommandSpec, cwd: &Path, verbose: bool) -> Result<()> {
    if verbose {
        eprintln!(
            "+ (cd {} && {} {})",
            cwd.display(),
            command.program,
            command.args.join(" ")
        );
    }
    let result = StreamingRunner::from_argv(&command.program, &command.args)
        .cwd(cwd)
        .collect()
        .await
        .with_context(|| format!("failed to run {}", command.program))?;
    if verbose || result.code != 0 {
        io::stdout().write_all(result.stdout.as_bytes())?;
        io::stderr().write_all(result.stderr.as_bytes())?;
    }
    if result.code != 0 {
        bail!("{} exited with status {}", command.program, result.code);
    }
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn wait_for_enter(prompt: &str) -> Result<()> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(())
}

#[must_use]
pub fn default_browser_profile(repository: &Path) -> PathBuf {
    repository.join(".package-registry-manager/browser-profile")
}
