//! The browser that fills forms: the dedicated profile by default, a
//! temporary snapshot of the user's own profile, or the user's running browser
//! through the Browser Commander extension.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use browser_commander::browser::migration::MigrationSource;
use browser_commander::browser::real_browser::{
    launch_real_browser, RealBrowserLaunchResult, RealBrowserOptions,
};
use browser_commander::{
    launch_snapshot, write_extension_directory, ExtensionRelay, RelayOptions, RelaySession,
    SnapshotLaunchResult, SnapshotOptions,
};
use serde_json::{json, Value};

use crate::browser_options::{snapshot_browser, AttachMode, BrowserOptions};
use crate::profile::{default_browser_profile, ensure_profile_ignored};

/// How long `--browser-attach extension` waits for the extension to connect.
pub const EXTENSION_TIMEOUT: Duration = Duration::from_secs(300);

/// Where the companion extension is written for Chrome's "Load unpacked".
pub fn relay_extension_directory() -> Result<PathBuf> {
    let profile = default_browser_profile()?;
    Ok(profile.parent().map_or_else(
        || PathBuf::from("relay-extension"),
        |state| state.join("relay-extension"),
    ))
}

/// The `launch_real_browser` options for the automated browser: the dedicated
/// profile by default, optionally seeded from a real profile. A snapshot
/// launch leaves the profile to [`snapshot_options`].
#[must_use]
pub fn launch_options(
    browser: &BrowserOptions,
    profile: &Path,
    verbose: bool,
) -> RealBrowserOptions {
    let mut options = RealBrowserOptions::chromiumoxide()
        .channel(&browser.channel)
        .headless(false)
        .verbose(verbose)
        .restrictions(browser.restrictions.iter().cloned());
    if let Some(executable) = &browser.executable {
        options = options.executable_path(executable);
    }
    options.preferences.clone_from(&browser.preferences);
    if browser.attach.is_some() {
        return options;
    }
    options = options.user_data_dir(profile);
    if let Some(source) = &browser.import {
        let mut migration = MigrationSource::new(&source.browser);
        if let Some(name) = &source.profile {
            migration = migration.profile(name);
        }
        options = options.migrate_from(migration);
    }
    options
}

/// The profile `--browser-attach snapshot` copies, if requested.
#[must_use]
pub fn snapshot_options(browser: &BrowserOptions) -> Option<SnapshotOptions> {
    match &browser.attach {
        Some(AttachMode::Snapshot { profile }) => Some(SnapshotOptions {
            browser: snapshot_browser(&browser.channel).to_owned(),
            profile: profile.clone().unwrap_or_else(|| "Default".to_owned()),
            user_data_dir: None,
        }),
        _ => None,
    }
}

/// The instructions printed while waiting for the companion extension.
#[must_use]
pub fn extension_instructions(directory: &Path) -> String {
    format!(
        "Waiting up to {} minutes for the Browser Commander extension in your own browser.\n\
         If it is not installed, open chrome://extensions, turn on Developer mode, click \"Load unpacked\", and choose:\n  {}",
        EXTENSION_TIMEOUT.as_secs() / 60,
        directory.display()
    )
}

/// A connected browser that fills forms.
pub enum Automation {
    Launched(Box<RealBrowserLaunchResult>),
    Snapshot(Box<SnapshotLaunchResult>),
    Extension {
        relay: ExtensionRelay,
        session: Option<RelaySession>,
    },
}

#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
impl Automation {
    /// Connects the browser that fills forms.
    pub async fn connect(browser: &BrowserOptions, profile: &Path, verbose: bool) -> Result<Self> {
        if browser.attach == Some(AttachMode::Extension) {
            return connect_extension().await;
        }
        let options = launch_options(browser, profile, verbose);
        if let Some(source) = snapshot_options(browser) {
            // Boxed: the launch future is large, and every step awaits it.
            let launched = Box::pin(launch_snapshot(source, options))
                .await
                .context("could not launch a snapshot of your browser profile")?;
            return Ok(Self::Snapshot(Box::new(launched)));
        }
        ensure_profile_ignored(profile, verbose).await?;
        let launched = Box::pin(launch_real_browser(options))
            .await
            .context("could not launch an installed Chrome-family browser")?;
        Ok(Self::Launched(Box::new(launched)))
    }

    fn launched(&self) -> Option<&RealBrowserLaunchResult> {
        match self {
            Self::Launched(launched) => Some(launched),
            Self::Snapshot(launched) => Some(&launched.launch),
            Self::Extension { .. } => None,
        }
    }

    /// Opens `url`; the extension opens a new tab first, then navigates it.
    pub async fn goto(&mut self, url: &str) -> Result<()> {
        if let Some(launched) = self.launched() {
            launched.page.goto(url).await?;
            return Ok(());
        }
        let Self::Extension { relay, session } = self else {
            unreachable!("only the extension has no launched browser")
        };
        if let Some(session) = session {
            session.send("Page.navigate", json!({ "url": url })).await?;
        } else {
            let tab = relay.new_tab(Some(url)).await?;
            *session = Some(relay.session(tab).await?);
        }
        Ok(())
    }

    /// Evaluates `script` in the page and returns its value.
    pub async fn evaluate(&self, script: &str) -> Result<Value> {
        if let Some(launched) = self.launched() {
            return Ok(launched.page.evaluate(script).await?);
        }
        let Self::Extension {
            session: Some(session),
            ..
        } = self
        else {
            bail!("no page is open in your browser");
        };
        let response = session
            .send(
                "Runtime.evaluate",
                json!({ "expression": script, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(details) = response.get("exceptionDetails") {
            let message = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("unknown error");
            return Err(anyhow!("the page script failed: {message}"));
        }
        Ok(response["result"]["value"].clone())
    }

    /// Detaches from the user's browser or closes the launched one.
    pub async fn close(self) {
        if let Self::Extension { mut relay, session } = self {
            if let Some(session) = session {
                let _ = session.detach().await;
            }
            relay.close().await;
        }
    }
}

async fn connect_extension() -> Result<Automation> {
    let directory = write_extension_directory(relay_extension_directory()?)?;
    let relay = ExtensionRelay::listen(RelayOptions {
        timeout: EXTENSION_TIMEOUT,
        ..RelayOptions::default()
    })
    .await?;
    println!("{}", extension_instructions(&directory));
    relay.wait_for_extension().await?;
    Ok(Automation::Extension {
        relay,
        session: None,
    })
}
