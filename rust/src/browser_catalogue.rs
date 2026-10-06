//! Browser identities and launch capabilities from Browser Commander's catalogue.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{bail, Result};
use browser_commander::{
    browser_sources, find_browser_source, resolve_browser_roots, BrowserSource, Environment,
};

static NAMES: LazyLock<HashMap<&'static str, String>> = LazyLock::new(|| {
    browser_sources()
        .iter()
        .map(|source| (source.id.as_str(), display_name(source)))
        .collect()
});

fn display_name(source: &BrowserSource) -> String {
    if let Some(app) = source
        .executables
        .get("darwin")
        .and_then(|paths| paths.first())
        .and_then(|path| path.split('/').find_map(|part| part.strip_suffix(".app")))
    {
        return if app == "Brave Browser" { "Brave" } else { app }.to_owned();
    }
    let label = std::iter::once(source.id.as_str())
        .chain(source.aliases.iter().map(String::as_str))
        .map(|name| name.strip_prefix("apple-").unwrap_or(name))
        .fold("", |longest, name| {
            if name.len() > longest.len() {
                name
            } else {
                longest
            }
        });
    label
        .split('-')
        .map(|word| {
            if word.len() <= 2 {
                word.to_uppercase()
            } else {
                let mut characters = word.chars();
                characters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().to_string() + characters.as_str()
                })
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .replace("Duckduckgo", "DuckDuckGo")
}

/// A readable name from the application path or canonical alias in the catalogue.
#[must_use]
pub fn catalogue_name(id: &str) -> &str {
    find_browser_source(id)
        .and_then(|source| NAMES.get(source.id.as_str()))
        .map_or(id, String::as_str)
}

/// Channels and aliases with a launch control protocol in the catalogue.
#[must_use]
pub fn launch_channels() -> Vec<&'static str> {
    browser_sources()
        .iter()
        .filter(|source| {
            source
                .control_protocol
                .as_deref()
                .is_some_and(|protocol| ["cdp", "bidi"].contains(&protocol))
        })
        .flat_map(|source| {
            std::iter::once(source.id.as_str()).chain(source.aliases.iter().map(String::as_str))
        })
        .collect()
}

/// Installed executable or profile-root discovery, without reading cookie data.
#[must_use]
pub fn installed_browsers_with(
    platform: &str,
    home: &str,
    environment: &Environment,
    exists: impl Fn(&Path) -> bool,
) -> Vec<&'static str> {
    let platform = match platform {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    // Windows environment names are case insensitive, including in snapshots.
    let windows_environment = (platform == "win32").then(|| {
        environment
            .iter()
            .map(|(key, value)| (key.to_ascii_uppercase(), value.clone()))
            .collect::<Environment>()
    });
    let environment = windows_environment.as_ref().unwrap_or(environment);
    browser_sources()
        .iter()
        .filter(|source| {
            executable_candidates(source, platform, home, environment)
                .into_iter()
                .chain(
                    resolve_browser_roots(&source.id, platform, home, environment)
                        .unwrap_or_default(),
                )
                .any(|candidate| exists(&candidate))
        })
        .map(|source| source.id.as_str())
        .collect()
}

// The published crate exposes executable templates but keeps its executable
// resolver private. Expand those templates and PATH names for discovery only.
fn executable_candidates(
    source: &BrowserSource,
    platform: &str,
    home: &str,
    environment: &Environment,
) -> Vec<PathBuf> {
    let separator = if platform == "win32" { '\\' } else { '/' };
    let join = |base: &str, tail: &str| {
        let tail = tail
            .trim_start_matches('/')
            .replace('/', &separator.to_string());
        format!("{}{separator}{tail}", base.trim_end_matches(['/', '\\']))
    };
    let mut variables = HashMap::from([
        ("home", home.to_owned()),
        ("appSupport", join(home, "Library/Application Support")),
        (
            "localAppData",
            environment
                .get("LOCALAPPDATA")
                .cloned()
                .unwrap_or_else(|| join(home, "AppData/Local")),
        ),
        (
            "appData",
            environment
                .get("APPDATA")
                .cloned()
                .unwrap_or_else(|| join(home, "AppData/Roaming")),
        ),
        (
            "config",
            environment
                .get("XDG_CONFIG_HOME")
                .cloned()
                .unwrap_or_else(|| join(home, ".config")),
        ),
    ]);
    for (variable, key) in [
        ("programFiles", "PROGRAMFILES"),
        ("programFilesX86", "PROGRAMFILES(X86)"),
    ] {
        if let Some(value) = environment.get(key) {
            variables.insert(variable, value.clone());
        }
    }
    let mut candidates: Vec<PathBuf> = source
        .executables
        .get(platform)
        .into_iter()
        .flatten()
        .filter_map(|template| {
            if let Some(rest) = template.strip_prefix('{') {
                let (key, tail) = rest.split_once('}')?;
                variables
                    .get(key)
                    .map(|base| PathBuf::from(join(base, tail)))
            } else {
                Some(PathBuf::from(template))
            }
        })
        .collect();
    for directory in environment
        .get("PATH")
        .map_or("", String::as_str)
        .split(if platform == "win32" { ';' } else { ':' })
        .filter(|entry| !entry.is_empty())
    {
        for name in &source.executable_names {
            let name = if platform == "win32" {
                format!("{name}.exe")
            } else {
                name.clone()
            };
            candidates.push(PathBuf::from(join(directory, &name)));
        }
    }
    candidates
}

/// Discovery information included in invalid browser-option errors.
#[must_use]
pub fn installed_description() -> String {
    let environment: Environment = std::env::vars().collect();
    let home = environment
        .get(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map_or("", String::as_str);
    let found = installed_browsers_with(std::env::consts::OS, home, &environment, Path::exists);
    format!(
        "Installed browsers found: {}.",
        if found.is_empty() {
            "none".to_owned()
        } else {
            found.join(", ")
        }
    )
}

/// Reject unavailable engines before creating an automated profile.
pub fn validate_channel(channel: &str) -> Result<()> {
    let source = find_browser_source(channel);
    if source.is_some_and(|source| {
        source
            .control_protocol
            .as_deref()
            .is_some_and(|protocol| ["cdp", "bidi"].contains(&protocol))
    }) {
        return Ok(());
    }
    let reason = source.map_or_else(
        || format!("unknown --browser-channel '{channel}'"),
        |source| {
            let upstream = if source.family == "safari" {
                "; Safari launch support is tracked at https://github.com/link-foundation/browser-commander/issues/126"
            } else { "" };
            format!("{channel} has no launch control protocol supported by this manager{upstream}")
        },
    );
    bail!(
        "{reason}; choose from {}. {}",
        launch_channels().join(", "),
        installed_description()
    )
}
