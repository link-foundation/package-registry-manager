//! Reviewed manifest URL repairs on a branch and draft pull request.

use std::path::Path;

use anyhow::{bail, Result};

use crate::model::SetupPlan;
use crate::repository_identity::{github_slug, manifest_urls, repository_slug};
use crate::workflow_proposal::run;

fn safe_manifest(manifest: &str) -> Result<()> {
    if manifest.is_empty()
        || Path::new(manifest).is_absolute()
        || manifest.contains('\\')
        || manifest
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        bail!("unsafe manifest path for repository repair");
    }
    Ok(())
}

/// Update stale source URLs while preserving npm repository.directory and other fields.
pub fn manifest_repository_proposal(contents: &str, manifest: &str, slug: &str) -> Result<String> {
    if github_slug(slug).is_none() {
        bail!("invalid canonical repository");
    }
    let stale: Vec<_> = manifest_urls(contents, manifest)
        .into_iter()
        .filter(|url| github_slug(url).is_some_and(|old| !old.eq_ignore_ascii_case(slug)))
        .collect();
    if stale.is_empty() {
        return Ok(contents.to_owned());
    }
    let url = format!("https://github.com/{slug}");
    if manifest.ends_with("package.json") {
        let mut document: serde_json::Value =
            serde_json::from_str(contents.trim_start_matches('\u{feff}'))?;
        if document["repository"].is_string() {
            document["repository"] = url.into();
        } else {
            document["repository"]["url"] = format!("git+{url}.git").into();
        }
        return Ok(format!("{}\n", serde_json::to_string_pretty(&document)?));
    }
    let mut proposal = contents.to_owned();
    for old in stale {
        let replacement = old.replace(&github_slug(&old).unwrap_or_default(), slug);
        proposal = proposal.replace(&old, &replacement);
    }
    Ok(proposal)
}

fn valid_reference(reference: &str) -> Result<()> {
    if reference.is_empty()
        || reference.starts_with('-')
        || reference.contains(|ch: char| ch.is_whitespace() || ch == ':')
    {
        bail!("invalid remote manifest reference");
    }
    Ok(())
}

/// Require the selected remote commit to have corrected metadata before retrying it.
pub async fn verify_remote_manifest(
    plan: &SetupPlan,
    repository: &Path,
    reference: Option<&str>,
    verbose: bool,
) -> Result<bool> {
    safe_manifest(&plan.package.manifest)?;
    let slug = repository_slug(&plan.repository).unwrap_or_default();
    let branch;
    let reference = if let Some(reference) = reference {
        reference
    } else {
        branch = run(
            repository,
            "gh",
            &["api", &format!("repos/{slug}"), "--jq", ".default_branch"],
            verbose,
        )
        .await?;
        &branch
    };
    valid_reference(reference)?;
    run(repository, "git", &["fetch", "origin", reference], verbose).await?;
    let contents = run(
        repository,
        "git",
        &["show", &format!("FETCH_HEAD:{}", plan.package.manifest)],
        verbose,
    )
    .await?;
    Ok(manifest_repository_proposal(&contents, &plan.package.manifest, &slug)? == contents)
}

/// Create a draft PR from the remote default branch and pause release retries for review.
/// An empty string means the default branch already has corrected metadata.
pub async fn offer_manifest_repository(
    plan: &SetupPlan,
    repository: &Path,
    yes: bool,
    verbose: bool,
) -> Result<Option<String>> {
    let slug = repository_slug(&plan.repository).unwrap_or_default();
    let manifest = &plan.package.manifest;
    safe_manifest(manifest)?;
    let remote = run(repository, "git", &["remote", "get-url", "origin"], verbose).await?;
    let Some(remote) = github_slug(&remote) else {
        bail!("origin must be a GitHub repository for the manifest proposal");
    };
    let canonical = run(
        repository,
        "gh",
        &["api", &format!("repos/{remote}"), "--jq", ".full_name"],
        verbose,
    )
    .await?;
    if !canonical.eq_ignore_ascii_case(&slug) {
        bail!("origin must resolve to the repository being repaired");
    }
    let base = run(
        repository,
        "gh",
        &["api", &format!("repos/{slug}"), "--jq", ".default_branch"],
        verbose,
    )
    .await?;
    valid_reference(&base)?;
    run(repository, "git", &["fetch", "origin", &base], verbose).await?;
    let contents = run(
        repository,
        "git",
        &["show", &format!("FETCH_HEAD:{manifest}")],
        verbose,
    )
    .await?;
    let proposal = manifest_repository_proposal(&contents, manifest, &slug)?;
    if contents == proposal {
        return Ok(Some(String::new()));
    }
    println!("Proposed {manifest}:\n{proposal}");
    if !yes
        && !crate::setup::is_yes(&crate::setup::prompt(
            "Create a branch and draft pull request with this repository URL fix? [y/N] ",
        )?)
    {
        return Ok(None);
    }
    let branch = format!(
        "prm/repository-url-{}",
        chrono::Utc::now().timestamp_millis()
    );
    let temporary = tempfile::Builder::new().prefix("prm-manifest-").tempdir()?;
    let checkout = temporary.path().join("checkout");
    let checkout_arg = checkout.to_string_lossy().into_owned();
    run(
        repository,
        "git",
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &checkout_arg,
            "FETCH_HEAD",
        ],
        verbose,
    )
    .await?;
    let result = async {
        let target = checkout.join(manifest);
        let root = checkout.canonicalize()?;
        let physical = target.canonicalize()?;
        if !physical.starts_with(&root) || physical != root.join(manifest) {
            bail!("manifest must stay inside the proposal worktree without symlinks");
        }
        let current = std::fs::read_to_string(&target)?;
        std::fs::write(&target, manifest_repository_proposal(&current, manifest, &slug)?)?;
        run(&checkout, "git", &["add", "--", manifest], verbose).await?;
        run(&checkout, "git", &["commit", "-m", "Fix manifest repository URL after repository transfer"], verbose).await?;
        run(&checkout, "git", &["push", "origin", &format!("HEAD:refs/heads/{branch}")], verbose).await?;
        let body = format!("Update {manifest} to name {slug}. Review and merge this change, then re-run package-registry-manager setup to verify the repository metadata and retry the release.");
        let url = run(&checkout, "gh", &["pr", "create", "--repo", &slug, "--base", &base, "--head", &branch, "--draft", "--title", "Fix manifest repository URL after repository transfer", "--body", &body], verbose).await?;
        println!("Review and merge {url}, then re-run setup. Release retries are paused.");
        Ok(Some(url))
    }.await;
    let cleanup = run(
        repository,
        "git",
        &["worktree", "remove", "--force", &checkout_arg],
        verbose,
    )
    .await;
    cleanup?;
    result
}
