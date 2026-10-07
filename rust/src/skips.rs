//! Rules that keep test fixtures, examples, and ignored paths out of inspection.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde_json::Value as JsonValue;

use crate::workflows::Workflow;

/// The optional per-repository settings file read during inspection.
pub const CONFIG_FILE: &str = ".package-registry-manager.json";

/// Directories that hold tests and examples rather than released packages.
pub const TEST_DIRECTORIES: [&str; 9] = [
    "tests",
    "test",
    "fixtures",
    "__fixtures__",
    "__tests__",
    "examples",
    "docs",
    "experiments",
    "case-studies",
];

/// Read the ignore list from `.package-registry-manager.json`, whose `ignore`
/// array holds glob patterns (`*`, `**`, `?`) relative to the root.
pub fn read_ignore_list(root: &Path) -> Result<Vec<String>> {
    let contents = match fs::read_to_string(root.join(CONFIG_FILE)) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {CONFIG_FILE}")),
    };
    let config: JsonValue = serde_json::from_str(&contents)
        .with_context(|| format!("invalid JSON in {CONFIG_FILE}"))?;
    match config.get("ignore") {
        None | Some(JsonValue::Null) => Ok(Vec::new()),
        Some(JsonValue::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .map_or_else(
                || bail!("{CONFIG_FILE}: \"ignore\" must be an array of strings"),
                Ok,
            ),
        Some(_) => bail!("{CONFIG_FILE}: \"ignore\" must be an array of strings"),
    }
}

/// Return the first ignore pattern that matches a path or one of its parents.
#[must_use]
pub fn ignored_by<'a>(ignore: &'a [String], manifest: &str) -> Option<&'a str> {
    let segments = manifest.split('/').collect::<Vec<_>>();
    let prefixes = (1..=segments.len())
        .map(|count| segments[..count].join("/"))
        .collect::<Vec<_>>();
    ignore
        .iter()
        .find(|pattern| {
            let expression = glob_expression(pattern);
            prefixes.iter().any(|prefix| expression.is_match(prefix))
        })
        .map(String::as_str)
}

/// Return the test or example directory that holds a path, if any.
#[must_use]
pub fn test_directory(manifest: &str) -> Option<&str> {
    let (directories, _) = manifest.rsplit_once('/')?;
    directories
        .split('/')
        .find(|segment| TEST_DIRECTORIES.contains(segment))
}

/// Report whether any workflow mentions the directory that holds a file.
#[must_use]
pub fn referenced_by_workflow(workflows: &[Workflow], manifest: &str) -> bool {
    let directory = manifest
        .rsplit_once('/')
        .map_or(".", |(directory, _)| directory);
    workflows.iter().any(|workflow| {
        workflow
            .contents
            .lines()
            .any(|line| !line.trim_start().starts_with('#') && line.contains(directory))
    })
}

/// Explain why a manifest in a test or example directory was skipped.
#[must_use]
pub fn test_directory_reason(directory: &str) -> String {
    format!("under {directory}/, a test or example directory, and no workflow publishes it")
}

/// Explain why a manifest matched by the ignore list was skipped.
#[must_use]
pub fn ignore_reason(pattern: &str) -> String {
    format!("ignored by \"{pattern}\" in {CONFIG_FILE}")
}

fn glob_expression(pattern: &str) -> Regex {
    let trimmed = pattern.trim();
    // Strip a leading `./` or `/`, but not the dot of a name like `.github`.
    let normalized = trimmed
        .strip_prefix('.')
        .filter(|rest| rest.starts_with('/'))
        .unwrap_or(trimmed)
        .trim_start_matches('/')
        .trim_end_matches('/');
    let mut source = String::new();
    let mut characters = normalized.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '*' if characters.peek() == Some(&'*') => {
                characters.next();
                if characters.peek() == Some(&'/') {
                    characters.next();
                    source.push_str("(?:.*/)?");
                } else {
                    source.push_str(".*");
                }
            }
            '*' => source.push_str("[^/]*"),
            '?' => source.push_str("[^/]"),
            _ => source.push_str(&regex::escape(&character.to_string())),
        }
    }
    Regex::new(&format!("^{source}$")).expect("escaped glob must compile")
}
