//! Browser options of the automated profile, shared by the CLI, the plan
//! summary, and the launch.

use std::path::PathBuf;

use anyhow::{bail, Result};
use browser_commander::browser::restrictions::{launch_restriction_presets, launch_restrictions};
use regex::Regex;
use serde_json::{Map, Value};

use crate::sign_in_import::{import_sources, IMPORT_CHOICES};

/// What `--browser-import` accepts: every browser Browser Commander reads,
/// plus `default` and `auto`, which pick the source browser for the user (the
/// sign-in import offer, the default browser first).
#[must_use]
pub fn import_browsers() -> Vec<&'static str> {
    let mut browsers = import_sources();
    browsers.extend(IMPORT_CHOICES);
    browsers
}
/// How `--browser-attach` reaches the user's own browser.
pub const ATTACH_MODES: [&str; 2] = ["snapshot", "extension"];

/// A real profile `--browser-import` migrates into the automated profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSource {
    pub browser: String,
    pub profile: Option<String>,
}

/// What `--browser-import` migrates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ImportScope {
    /// Only the cookies of the registry's sign-in domains.
    #[default]
    Domains,
    /// The whole profile.
    Full,
}

/// How `--browser-attach` fills forms in the user's own browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachMode {
    /// A temporary copy of the user's profile; the original is never written.
    Snapshot { profile: Option<String> },
    /// The user's running browser, through the Browser Commander extension.
    Extension,
}

/// The validated browser options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserOptions {
    pub channel: String,
    pub executable: Option<PathBuf>,
    pub import: Option<ImportSource>,
    /// What the import migrates (`--browser-import-scope`).
    pub import_scope: ImportScope,
    pub attach: Option<AttachMode>,
    /// Preferences deep-merged into the profile, always a JSON object.
    pub preferences: Value,
    pub restrictions: Vec<String>,
}

impl Default for BrowserOptions {
    fn default() -> Self {
        Self {
            channel: "chrome".to_owned(),
            executable: None,
            import: None,
            import_scope: ImportScope::Domains,
            attach: None,
            preferences: Value::Object(Map::new()),
            restrictions: Vec::new(),
        }
    }
}

impl BrowserOptions {
    fn has_preferences(&self) -> bool {
        self.preferences
            .as_object()
            .is_some_and(|preferences| !preferences.is_empty())
    }
}

/// The browser options as given on the command line.
#[derive(Debug, Clone)]
pub struct BrowserArgs {
    pub channel: String,
    pub executable: Option<PathBuf>,
    pub import: Option<String>,
    /// `--browser-import-scope`: `full` or `domains`.
    pub import_scope: Option<String>,
    pub attach: Option<String>,
    pub preferences: Vec<String>,
    pub restrictions: Vec<String>,
    /// Whether `--browser-profile` was given.
    pub profile_given: bool,
}

impl Default for BrowserArgs {
    fn default() -> Self {
        Self {
            channel: "chrome".to_owned(),
            executable: None,
            import: None,
            import_scope: None,
            attach: None,
            preferences: Vec::new(),
            restrictions: Vec::new(),
            profile_given: false,
        }
    }
}

/// Every launch restriction and preset `--browser-restriction` accepts, from
/// Browser Commander's shared catalogue.
#[must_use]
pub fn restriction_names() -> Vec<String> {
    launch_restrictions()
        .iter()
        .map(|restriction| restriction.id.clone())
        .chain(
            launch_restriction_presets()
                .iter()
                .map(|(preset, _)| preset.clone()),
        )
        .collect()
}

/// Validates the browser options of the automated profile.
///
/// Nothing changes by default: a fresh dedicated profile fills forms. `--browser-import` migrates
/// a real profile into it, and `--browser-attach` uses the user's own browser
/// instead, as a temporary snapshot or through the companion extension.
pub fn parse_browser_options(args: &BrowserArgs) -> Result<BrowserOptions> {
    let given = |value: &Option<String>| value.clone().filter(|value| !value.is_empty());
    let options = BrowserOptions {
        channel: args.channel.clone(),
        executable: match args
            .executable
            .as_ref()
            .filter(|path| !path.as_os_str().is_empty())
        {
            Some(path) => Some(std::path::absolute(path)?),
            None => None,
        },
        import: given(&args.import)
            .map(|spec| parse_import(&spec))
            .transpose()?,
        import_scope: parse_import_scope(
            given(&args.import_scope).as_deref(),
            given(&args.import).as_deref(),
        )?,
        attach: given(&args.attach)
            .map(|spec| parse_attach(&spec))
            .transpose()?,
        preferences: parse_preferences(&args.preferences)?,
        restrictions: validate_restrictions(&args.restrictions)?,
    };
    if options.attach.is_some() && options.import.is_some() {
        bail!("--browser-attach cannot be combined with --browser-import");
    }
    if options.attach.is_some() && args.profile_given {
        bail!("--browser-attach cannot be combined with --browser-profile");
    }
    if options.attach == Some(AttachMode::Extension)
        && (options.executable.is_some()
            || !options.restrictions.is_empty()
            || options.has_preferences())
    {
        bail!(
            "--browser-attach extension cannot be combined with --browser-executable, --browser-pref, or --browser-restriction"
        );
    }
    Ok(options)
}

/// Parses `<browser>[:profile]`, `default`, or `auto`.
pub fn parse_import(spec: &str) -> Result<ImportSource> {
    let (browser, profile) = match spec.split_once(':') {
        Some((browser, profile)) => (browser, Some(profile)),
        None => (spec, None),
    };
    let choice = IMPORT_CHOICES.contains(&browser);
    if !import_browsers().contains(&browser) || profile == Some("") || (choice && profile.is_some())
    {
        bail!(
            "--browser-import must be <{}>[:profile], default, or auto",
            import_sources().join("|")
        );
    }
    Ok(ImportSource {
        browser: browser.to_owned(),
        profile: profile.map(str::to_owned),
    })
}

/// Parses `--browser-import-scope`.
///
/// `full` migrates the whole profile and `domains` only the registry's
/// sign-in cookies. Without it a named browser is migrated fully, as before,
/// and `default`, `auto`, and the sign-in offer import only the sign-in
/// domains.
pub fn parse_import_scope(scope: Option<&str>, import: Option<&str>) -> Result<ImportScope> {
    match scope {
        None => {
            let named = import.is_some_and(|spec| !IMPORT_CHOICES.contains(&spec));
            Ok(if named {
                ImportScope::Full
            } else {
                ImportScope::Domains
            })
        }
        Some("domains") => Ok(ImportScope::Domains),
        Some("full") => Ok(ImportScope::Full),
        Some(_) => bail!("--browser-import-scope must be full or domains"),
    }
}

/// Parses `snapshot`, `snapshot:<profile>`, or `extension`.
pub fn parse_attach(spec: &str) -> Result<AttachMode> {
    match spec {
        "extension" => Ok(AttachMode::Extension),
        "snapshot" => Ok(AttachMode::Snapshot { profile: None }),
        _ => match spec
            .strip_prefix("snapshot:")
            .filter(|profile| !profile.is_empty())
        {
            Some(profile) => Ok(AttachMode::Snapshot {
                profile: Some(profile.to_owned()),
            }),
            None => {
                bail!("--browser-attach must be 'snapshot', 'snapshot:<profile>', or 'extension'")
            }
        },
    }
}

/// Builds the preferences object from `key=value` pairs. Dotted keys nest, and
/// a value is JSON when it parses as JSON, otherwise a string.
pub fn parse_preferences(pairs: &[String]) -> Result<Value> {
    let key_pattern = Regex::new(r"^[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*$")?;
    let mut preferences = Map::new();
    for pair in pairs {
        let (key, value) = pair.split_once('=').unwrap_or(("", ""));
        if !key_pattern.is_match(key) {
            bail!("--browser-pref must be key=value, got '{pair}'");
        }
        let mut parts: Vec<&str> = key.split('.').collect();
        let last = parts.pop().unwrap_or_default();
        let mut target = &mut preferences;
        for part in parts {
            let entry = target
                .entry(part.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            let Value::Object(next) = entry else {
                unreachable!("the entry was just made an object")
            };
            target = next;
        }
        target.insert(last.to_owned(), preference_value(value));
    }
    Ok(Value::Object(preferences))
}

fn preference_value(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

/// Rejects a restriction or preset that is not in the shared catalogue.
pub fn validate_restrictions(names: &[String]) -> Result<Vec<String>> {
    let known = restriction_names();
    let mut unique = Vec::new();
    for name in names {
        if !known.contains(name) {
            bail!(
                "unknown --browser-restriction '{name}'; choose from {}",
                known.join(", ")
            );
        }
        if !unique.contains(name) {
            unique.push(name.clone());
        }
    }
    Ok(unique)
}

/// The installed browser a snapshot copies for a launch channel.
#[must_use]
pub fn snapshot_browser(channel: &str) -> &str {
    if channel.starts_with("msedge") {
        "edge"
    } else if matches!(channel, "brave" | "chromium") {
        channel
    } else {
        "chrome"
    }
}

/// Describes where forms are filled, for the browser prerequisite.
#[must_use]
pub fn automated_description(
    channel: &str,
    profile: Option<&str>,
    attach: Option<&AttachMode>,
    import: Option<&ImportSource>,
) -> String {
    match attach {
        Some(AttachMode::Extension) => {
            "your own browser through the Browser Commander extension".to_owned()
        }
        Some(AttachMode::Snapshot { profile }) => format!(
            "a temporary snapshot of your {} profile {}",
            snapshot_browser(channel),
            profile.as_deref().unwrap_or("Default")
        ),
        None => {
            let location = profile
                .map(|profile| format!(" at {profile}"))
                .unwrap_or_default();
            let imported = import
                .map(|source| {
                    let profile = source
                        .profile
                        .as_ref()
                        .map(|profile| format!(":{profile}"))
                        .unwrap_or_default();
                    format!(" with data imported from {}{profile}", source.browser)
                })
                .unwrap_or_default();
            format!("the automated {channel} profile{location}{imported}")
        }
    }
}
