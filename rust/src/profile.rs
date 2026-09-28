//! Where the dedicated automation browser profile lives, and how a profile
//! inside a Git work tree is kept out of commits.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};

use crate::auth_urls::resolve_program;

const APPLICATION: &str = "package-registry-manager";
const PROFILE: &str = "browser-profile";
const REPOSITORY_DIRECTORY: &str = ".package-registry-manager";

/// The dedicated automation profile, shared by every repository of the user.
///
/// It lives in the per-user state directory, never inside a repository, so its
/// registry session cookies cannot be committed:
/// `~/Library/Application Support` on macOS, `%LOCALAPPDATA%` on Windows, and
/// `$XDG_STATE_HOME` (or `~/.local/state`) elsewhere.
pub fn default_browser_profile() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .context("cannot find the home directory; pass --browser-profile")?;
    Ok(default_browser_profile_for(
        std::env::consts::OS,
        |name| std::env::var(name).ok(),
        &home,
    ))
}

/// [`default_browser_profile`] for an explicit operating system (a value of
/// [`std::env::consts::OS`]), environment, and home directory.
#[must_use]
pub fn default_browser_profile_for(
    os: &str,
    var: impl Fn(&str) -> Option<String>,
    home: &Path,
) -> PathBuf {
    let base = match os {
        "windows" => var("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join("AppData").join("Local"), PathBuf::from),
        "macos" => home.join("Library").join("Application Support"),
        // The XDG specification ignores relative values.
        _ => var("XDG_STATE_HOME")
            .filter(|value| value.starts_with('/'))
            .map_or_else(|| home.join(".local").join("state"), PathBuf::from),
    };
    base.join(APPLICATION).join(PROFILE)
}

/// The profile location used before it moved out of the repository.
#[must_use]
pub fn legacy_browser_profile(repository: &Path) -> PathBuf {
    repository.join(REPOSITORY_DIRECTORY).join(PROFILE)
}

async fn git(profile: &Path, args: &[&str]) -> Option<(bool, String)> {
    let output = tokio::process::Command::new(resolve_program("git"))
        .args(args)
        .current_dir(profile)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    Some((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

fn create_private_directory(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(path)
        .with_context(|| format!("cannot create {}", path.display()))
}

/// Make sure Git ignores a browser profile before a browser writes cookies to it.
///
/// Outside a Git work tree (or without Git) this only creates the directory.
/// Inside one it writes a `.gitignore` containing `*` (into
/// `.package-registry-manager/` for the legacy location, otherwise into the
/// profile itself) and fails when Git still does not ignore the profile or
/// already tracks files in it.
pub async fn ensure_profile_ignored(profile: &Path, verbose: bool) -> Result<()> {
    create_private_directory(profile)?;
    let Some((true, top)) = git(profile, &["rev-parse", "--show-toplevel"]).await else {
        if verbose {
            eprintln!("{} is not inside a Git work tree", profile.display());
        }
        return Ok(());
    };
    let guarded = match profile.parent() {
        Some(parent) if parent.file_name() == Some(REPOSITORY_DIRECTORY.as_ref()) => parent,
        _ => profile,
    };
    let ignore = guarded.join(".gitignore");
    std::fs::write(&ignore, "*\n").with_context(|| format!("cannot write {}", ignore.display()))?;
    // Relative to the profile, so symbolic links in its path do not matter.
    let ignored = git(
        profile,
        &["check-ignore", "--quiet", "--", "Default/Cookies"],
    )
    .await;
    let tracked = git(profile, &["ls-files", "--", "."]).await;
    let untracked = matches!(tracked, Some((true, ref files)) if files.is_empty());
    if !matches!(ignored, Some((true, _))) || !untracked {
        bail!(
            "the browser profile {} is inside the Git work tree {top} and Git does not ignore it; \
             move it with --browser-profile or ignore it (and untrack its files) before signing in",
            profile.display()
        );
    }
    if verbose {
        eprintln!(
            "{} is ignored by Git ({})",
            profile.display(),
            ignore.display()
        );
    }
    Ok(())
}

/// Protect a browser profile left inside the repository by an older release
/// and suggest deleting it, because it can hold registry session cookies.
pub async fn protect_legacy_profile(
    repository: &Path,
    profile: &Path,
    verbose: bool,
) -> Result<()> {
    let legacy = legacy_browser_profile(repository);
    let same = |a: &Path, b: &Path| {
        a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
    };
    if same(&legacy, profile) || !legacy.is_dir() {
        return Ok(());
    }
    ensure_profile_ignored(&legacy, verbose).await?;
    eprintln!(
        "warning: {} is no longer used and may hold registry session cookies; \
         delete it (the browser profile is now {})",
        legacy.display(),
        profile.display()
    );
    Ok(())
}
