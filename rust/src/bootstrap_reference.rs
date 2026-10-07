//! Inspect a pushed branch or pull-request head before merging its manifest.

use crate::{inspect_repository_with, InspectOptions, Inspection};
use anyhow::{bail, Result};
use regex::Regex;
use std::path::Path;
use std::process::Command;

/// Translate a branch, PR number, or GitHub PR URL into a safe fetch ref.
pub fn bootstrap_ref(reference: &str) -> Result<String> {
    let pull = Regex::new(r"^(?:https://github\.com/[\w.-]+/[\w.-]+/pull/)?(\d+)$")
        .expect("static pattern");
    let result = pull.captures(reference).map_or_else(
        || reference.to_owned(),
        |matched| format!("refs/pull/{}/head", &matched[1]),
    );
    if !Regex::new(r"^[\w][\w./-]*$")
        .expect("static pattern")
        .is_match(&result)
        || result.contains("..")
        || result.contains("//")
        || result.ends_with('/')
        || result.to_ascii_lowercase().ends_with(".lock")
    {
        bail!("invalid bootstrap ref");
    }
    Ok(result)
}

/// Inspect a pushed ref in a disposable worktree, always removing the worktree.
pub fn inspect_reference(
    repository: &Path,
    reference: &str,
    options: InspectOptions,
) -> Result<Inspection> {
    let run = |args: &[&str]| -> Result<String> {
        let result = Command::new("git")
            .args(args)
            .current_dir(repository)
            .output()?;
        if !result.status.success() {
            bail!(
                "git failed while inspecting ref: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        Ok(String::from_utf8(result.stdout)?.trim().to_owned())
    };
    let temporary = tempfile::tempdir()?;
    let checkout = temporary.path().join("checkout");
    let checkout_str = checkout.to_string_lossy();
    run(&["fetch", "origin", "HEAD"])?;
    let main = run(&["rev-parse", "FETCH_HEAD"])?;
    run(&["fetch", "origin", &bootstrap_ref(reference)?])?;
    run(&["worktree", "add", "--detach", &checkout_str, "FETCH_HEAD"])?;
    let result = inspect_repository_with(&checkout, options);
    let cleanup = run(&["worktree", "remove", "--force", &checkout_str]);
    let mut inspection = result?;
    cleanup?;
    for package in inspection
        .packages
        .iter_mut()
        .filter(|package| package.registry == crate::Registry::Npm)
    {
        let result = Command::new("git")
            .args(["show", &format!("{main}:{}", package.manifest)])
            .current_dir(repository)
            .output()?;
        let relationship = if result.status.success() {
            let data: serde_json::Value = serde_json::from_slice(&result.stdout)?;
            format!(
                "remote main is {}@{}",
                data["name"].as_str().unwrap_or_default(),
                data["version"].as_str().unwrap_or_default()
            )
        } else {
            "manifest is absent from remote main".into()
        };
        package.warnings.push(format!("bootstrap {}@{} from {reference}; {relationship}; the next main release must use an unpublished version",package.name,package.version.as_deref().unwrap_or_default()));
    }
    inspection.repository.root = repository.canonicalize()?.to_string_lossy().into_owned();
    Ok(inspection)
}
