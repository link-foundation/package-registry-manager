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
use browser_commander::browser::system_browser::assert_dedicated_user_data_dir;
use browser_commander::{
    find_browser_source, launch_snapshot, write_extension_directory, EngineAdapter, EngineType,
    ExtensionRelay, ManagedWebDriver, RelayOptions, RelaySession, SnapshotLaunchResult,
    SnapshotOptions,
};
use serde_json::{json, Value};

use crate::browser_options::{snapshot_browser, AttachMode, BrowserOptions, ImportScope};
use crate::profile::{default_browser_profile, ensure_profile_ignored};
use crate::sign_in_import::IMPORT_CHOICES;

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

/// The `launch_real_browser` options for the automated browser.
///
/// The dedicated profile is used by default, optionally seeded from a real
/// profile. A snapshot launch leaves the profile to [`snapshot_options`]. With the `domains`
/// import scope only the cookies of `domains`, the registry's sign-in
/// domains, are imported.
#[must_use]
pub fn launch_options(
    browser: &BrowserOptions,
    profile: &Path,
    verbose: bool,
    domains: &[&str],
) -> RealBrowserOptions {
    let engine = if find_browser_source(&browser.channel)
        .is_some_and(|source| source.control_protocol.as_deref() == Some("cdp"))
    {
        EngineType::Chromiumoxide
    } else {
        EngineType::Fantoccini
    };
    let mut options = RealBrowserOptions {
        engine,
        ..RealBrowserOptions::default()
    }
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
    // `default` and `auto` are resolved to an installed browser before launch.
    if let Some(source) = browser
        .import
        .as_ref()
        .filter(|source| !IMPORT_CHOICES.contains(&source.browser.as_str()))
    {
        let mut migration = MigrationSource::new(&source.browser);
        if let Some(name) = &source.profile {
            migration = migration.profile(name);
        }
        options = options.migrate_from(migration);
        if browser.import_scope == ImportScope::Domains && !domains.is_empty() {
            options = options
                .migrate_include(["cookies"])
                .migrate_domains(domains.iter().copied());
        }
    }
    options
}

/// The storage state that deletes every cookie of `domains` or their
/// subdomains: the same cookies, empty and expired long ago.
#[must_use]
pub fn expired_cookies(state: &Value, domains: &[&str]) -> Value {
    let cookies: Vec<Value> = state["cookies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|cookie| {
            let host = cookie["domain"].as_str().unwrap_or_default();
            let host = host.strip_prefix('.').unwrap_or(host);
            domains.iter().any(|domain| {
                host == *domain
                    || host
                        .strip_suffix(domain)
                        .is_some_and(|prefix| prefix.ends_with('.'))
            })
        })
        .map(|cookie| {
            let mut expired = cookie.clone();
            expired["value"] = json!("");
            expired["expires"] = json!(1.0);
            expired
        })
        .collect();
    json!({ "cookies": cookies, "origins": [] })
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
    WebDriver(Box<ManagedWebDriver>),
    Extension {
        relay: ExtensionRelay,
        session: Option<RelaySession>,
    },
}

#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
impl Automation {
    /// Connects the browser that fills forms; `domains` are the sign-in
    /// domains a scoped import brings in.
    pub async fn connect(
        browser: &BrowserOptions,
        profile: &Path,
        verbose: bool,
        domains: &[&str],
    ) -> Result<Self> {
        if browser.attach == Some(AttachMode::Extension) {
            return connect_extension().await;
        }
        let options = launch_options(browser, profile, verbose, domains);
        if let Some(source) = snapshot_options(browser) {
            // Boxed: the launch future is large, and every step awaits it.
            let launched = Box::pin(launch_snapshot(source, options))
                .await
                .context("could not launch a snapshot of your browser profile")?;
            return Ok(Self::Snapshot(Box::new(launched)));
        }
        if find_browser_source(&browser.channel)
            .is_some_and(|source| source.control_protocol.as_deref() == Some("bidi"))
        {
            assert_dedicated_user_data_dir(profile)?;
            ensure_profile_ignored(profile, verbose).await?;
            let driver = Box::pin(crate::webdriver_automation::connect(options, domains))
                .await
                .with_context(|| {
                    format!(
                        "could not launch {} through WebDriver BiDi",
                        browser.channel
                    )
                })?;
            return Ok(Self::WebDriver(Box::new(driver)));
        }
        ensure_profile_ignored(profile, verbose).await?;
        let launched = Box::pin(launch_real_browser(options))
            .await
            .context("could not launch the selected installed browser")?;
        Ok(Self::Launched(Box::new(launched)))
    }

    fn launched(&self) -> Option<&RealBrowserLaunchResult> {
        match self {
            Self::Launched(launched) => Some(launched),
            Self::Snapshot(launched) => Some(&launched.launch),
            Self::Extension { .. } | Self::WebDriver(_) => None,
        }
    }

    /// Opens `url`; the extension opens a new tab first, then navigates it.
    pub async fn goto(&mut self, url: &str) -> Result<()> {
        if let Self::WebDriver(driver) = self {
            return Ok(driver.goto(url).await?);
        }
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
        if let Self::WebDriver(driver) = self {
            return Ok(driver.evaluate(script).await?);
        }
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

    /// Removes the cookies of `domains` from a launched profile; returns
    /// whether it could. The user's own browser is never touched.
    pub async fn clear_cookies(&self, domains: &[&str]) -> Result<bool> {
        if let Self::WebDriver(driver) = self {
            crate::webdriver_automation::clear_cookies(driver, domains).await?;
            return Ok(true);
        }
        let Some(launched) = self.launched() else {
            return Ok(false);
        };
        let state = launched.page.export_storage_state().await?;
        launched
            .page
            .restore_storage_state(expired_cookies(&state, domains))
            .await?;
        Ok(true)
    }

    /// Detaches from the user's browser or closes the launched one.
    pub async fn close(self) {
        match self {
            Self::Extension { mut relay, session } => {
                if let Some(session) = session {
                    let _ = session.detach().await;
                }
                relay.close().await;
            }
            // Gracefully, so a relaunch can open the same profile again.
            Self::Launched(launched) => {
                let _ = launched.close().await;
            }
            Self::WebDriver(driver) => {
                let _ = driver.close().await;
            }
            Self::Snapshot(_) => {}
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
