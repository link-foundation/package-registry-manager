//! Checks of the packed npm tarball before its first publish: npm's pack
//! warnings, the packed bin entries, and each installed bin run with
//! `--version`, which must print the version.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use command_stream::StreamingRunner;
use regex::Regex;
use serde_json::Value;

use crate::auth_urls::ANSI_PATTERN;

/// npm's rejection of a publish from a workflow it does not trust.
pub const PUBLISH_REJECTED: &str = r"(?i)\bE404\b|404 Not Found|invalid-publisher";

/// npm's warnings from `npm pack`, such as fields it auto-corrected or removed
/// from the packed package.json, without npm's advice to run its fixer, which
/// rewrites package.json in place.
#[must_use]
pub fn pack_warnings(stderr: &str) -> Vec<String> {
    let ansi = Regex::new(ANSI_PATTERN).expect("static pattern must compile");
    let warning = Regex::new(r"(?i)^npm warn").expect("static pattern must compile");
    let advice = Regex::new(r#"(?i)\s*Please run "npm pkg fix"[^.]*\.?"#)
        .expect("static pattern must compile");
    let fixer = Regex::new(r"(?i)npm pkg fix").expect("static pattern must compile");
    stderr
        .split('\n')
        .map(|line| ansi.replace_all(line, "").trim().to_owned())
        .filter(|line| warning.is_match(line))
        .map(|line| advice.replace(&line, "").trim().to_owned())
        .filter(|line| !fixer.is_match(line))
        .collect()
}

/// Prints npm's pack warnings and whether npm changed the packed package.json.
pub fn report_pack_warnings(stderr: &str) {
    let warnings = pack_warnings(stderr);
    if warnings.is_empty() {
        return;
    }
    println!("  npm warned while packing:");
    for line in &warnings {
        println!("    {line}");
    }
    let changed =
        Regex::new(r"(?i)corrected|invalid and removed").expect("static pattern must compile");
    if warnings.iter().any(|line| changed.is_match(line)) {
        println!(
            "  npm changed the packed package.json; correct these fields in package.json itself."
        );
    }
}

/// The first package in `npm pack --json` output: an array before npm 12, an
/// object keyed by package name since.
///
/// # Errors
/// Fails when the output is not JSON or names no package.
pub fn packed_entry(output: &str) -> Result<Value> {
    let parsed: Value =
        serde_json::from_str(output.trim()).context("npm pack printed invalid JSON")?;
    let packed = match parsed {
        Value::Array(items) => items.into_iter().next(),
        Value::Object(entries) => entries.into_iter().next().map(|(_, packed)| packed),
        _ => None,
    };
    packed.context("npm pack printed no package")
}

/// The `bin` entries of a package.json as `(name, file)` pairs; a string `bin`
/// is named after the unscoped package name.
#[must_use]
pub fn bin_entries(manifest: &Value) -> Vec<(String, String)> {
    match &manifest["bin"] {
        Value::String(file) => {
            let name = manifest["name"].as_str().unwrap_or_default();
            let unscoped = if name.starts_with('@') {
                name.split_once('/').map_or(name, |(_, rest)| rest)
            } else {
                name
            };
            vec![(unscoped.to_owned(), file.clone())]
        }
        Value::Object(entries) => entries
            .iter()
            .map(|(name, file)| {
                let file = match file {
                    Value::String(file) => file.clone(),
                    other => other.to_string(),
                };
                (name.clone(), file)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn read_manifest(file: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("could not read {}", file.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", file.display()))
}

/// Compares the bin entries of the packed package.json, installed under
/// `prefix`, with the source package.json in `source`, then runs each
/// installed bin with `--version`.
///
/// # Errors
/// Fails when npm removed a bin while packing, or a bin exits with an error.
pub async fn verify_bins(source: &Path, prefix: &Path, name: &str, verbose: bool) -> Result<()> {
    let source = read_manifest(&source.join("package.json"))?;
    let root = name
        .split('/')
        .fold(prefix.join("node_modules"), |path, part| path.join(part));
    // npm extracts the tarball's package/package.json unchanged.
    let packed = bin_entries(&read_manifest(&root.join("package.json"))?);
    let removed = bin_entries(&source)
        .into_iter()
        .filter(|(bin, _)| !packed.iter().any(|(packed, _)| packed == bin))
        .map(|(bin, _)| bin)
        .collect::<Vec<_>>();
    if !removed.is_empty() {
        bail!(
            "the packed package.json has no bin {}; npm removed it while packing, so correct the bin entries in package.json before the first publish",
            removed.join(", ")
        );
    }
    if packed.is_empty() {
        println!("  The package has no bin entries.");
    }
    for (bin, file) in &packed {
        // Windows links bins as .cmd shims, which cannot run without a shell.
        let (program, args) = if cfg!(windows) {
            (
                "node".into(),
                vec![root.join(file).into_os_string(), "--version".into()],
            )
        } else {
            (
                prefix
                    .join("node_modules")
                    .join(".bin")
                    .join(bin)
                    .into_os_string(),
                vec!["--version".into()],
            )
        };
        if verbose {
            eprintln!(
                "+ (cd {} && {} --version)",
                prefix.display(),
                Path::new(&program).display()
            );
        }
        // A bin that cannot be started reports 127, as a shell would.
        let (code, stdout, stderr) = match StreamingRunner::from_argv(program, args)
            .cwd(prefix)
            .collect()
            .await
        {
            Ok(result) => (
                result.code,
                result.stdout.to_string(),
                result.stderr.to_string(),
            ),
            Err(error) => (127, String::new(), format!("{error}\n")),
        };
        if verbose || code != 0 {
            io::stdout().write_all(stdout.as_bytes())?;
            io::stderr().write_all(stderr.as_bytes())?;
        }
        if code != 0 {
            bail!(
                "bin {bin} ({file}) exited with status {code} when run with --version from the installed tarball"
            );
        }
        // A bin whose entry point never runs exits 0 without output.
        let output = match stdout.trim() {
            "" => stderr.trim(),
            text => text,
        };
        if output.is_empty() {
            bail!(
                "bin {bin} ({file}) printed nothing when run with --version from the installed tarball; make sure it runs when started through the node_modules/.bin symlink"
            );
        }
        println!(
            "  {bin} --version: {}",
            output.lines().next().unwrap_or_default()
        );
    }
    Ok(())
}
