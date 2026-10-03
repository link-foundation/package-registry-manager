#!/usr/bin/env rust-script
//! Publish package to crates.io
//!
//! This script publishes the Rust package to crates.io and handles
//! the case where the version already exists.
//!
//! Supports both single-language and multi-language repository structures:
//! - Single-language: Cargo.toml in repository root
//! - Multi-language: Cargo.toml in rust/ subfolder
//!
//! Usage: rust-script scripts/publish-crate.rs [--rust-root <path>]
//!
//! Authentication (issue #16): crates.io trusted publishing. In CI,
//! rust-lang/crates-io-auth-action exchanges the job's OIDC token for a
//! short-lived crates.io token, which the publish step receives as
//! CARGO_REGISTRY_TOKEN -- Cargo's native variable, read by `cargo publish`
//! itself. The token is never put on the command line (where it would be
//! visible in the process list) and there is no long-lived CARGO_TOKEN
//! fallback. Trusted publishing only publishes new versions of an existing
//! crate; the first version is published once from a maintainer's machine:
//!   package-registry-manager setup --registry crates-io --execute
//!
//! Environment variables:
//!   - CARGO_REGISTRY_TOKEN: the short-lived trusted-publishing token
//!
//! Outputs (written to GITHUB_OUTPUT):
//!   - publish_result: one of
//!       'success'        - the crate version was published to crates.io
//!       'already_exists' - the version is already on crates.io (version-bump bug)
//!       'auth_failed'    - missing or invalid crates.io authentication token
//!       'rate_limited'   - crates.io returned HTTP 429 (too many versions in 24h);
//!                          deferred, automatically-recoverable outcome — the
//!                          script exits 0 and the same version is retried on the
//!                          next push to 'main' once the throttle window clears
//!       'skipped'        - publish skipped (e.g. template default package name)
//!       'failed'         - publish failed for an unrecognised reason
//!
//! ```cargo
//! [dependencies]
//! regex = "1"
//! ```

use std::env;
use std::fs;
use std::io::Write;
use std::process::{Command, exit};

#[path = "rust-paths.rs"]
mod rust_paths;

const BOOTSTRAP_COMMAND: &str = "package-registry-manager setup --registry crates-io --execute";

fn needs_cd(rust_root: &str) -> bool {
    rust_root != "."
}

fn set_output(key: &str, value: &str) {
    if let Ok(output_file) = env::var("GITHUB_OUTPUT") {
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&output_file) {
            let _ = writeln!(file, "{}={}", key, value);
        }
    }
    println!("Output: {}={}", key, value);
}

/// Classification of a failed `cargo publish` attempt.
///
/// Every failure branch funnels through this single enum so the `publish_result`
/// output value and the catch-all behaviour cannot drift apart over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    AlreadyExists,
    AuthFailed,
    RateLimited,
    Unknown,
}

impl FailureKind {
    fn output_value(self) -> &'static str {
        match self {
            FailureKind::AlreadyExists => "already_exists",
            FailureKind::AuthFailed => "auth_failed",
            FailureKind::RateLimited => "rate_limited",
            FailureKind::Unknown => "failed",
        }
    }

    /// Whether this failure is a *deferred*, automatically-recoverable outcome
    /// rather than a hard error.
    ///
    /// A crates.io HTTP 429 throttle is transient: the same version is re-tried
    /// on the next push to `main` once the 24-hour window clears (see
    /// `scripts/check-release-needed.rs`). The script therefore exits
    /// successfully for this case so a recoverable throttle does not turn the
    /// whole release job red. Every other failure stays non-zero.
    fn is_deferred(self) -> bool {
        matches!(self, FailureKind::RateLimited)
    }
}

/// Classify a combined stdout/stderr blob from `cargo publish` into a
/// [`FailureKind`].
///
/// Note: the rate-limit checks come before the auth checks because a crates.io
/// 429 body never contains the auth-token markers, while the rate-limit markers
/// are unambiguous.
fn classify_failure(combined: &str) -> FailureKind {
    if combined.contains("already uploaded") || combined.contains("already exists") {
        FailureKind::AlreadyExists
    } else if combined.contains("429 Too Many Requests")
        || combined.contains("Too Many Requests")
        || combined.contains("too many versions")
        || combined.contains("too many requests")
    {
        FailureKind::RateLimited
    } else if combined.contains("non-empty token")
        || combined.contains("please provide a")
        || combined.contains("unauthorized")
        || combined.contains("authentication")
        || combined.contains("403 Forbidden")
    {
        FailureKind::AuthFailed
    } else {
        FailureKind::Unknown
    }
}

fn main() {
    let rust_root = match rust_paths::get_rust_root(None, true) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("Error: {}", e);
            exit(1);
        }
    };
    let cargo_toml = rust_paths::get_cargo_toml_path(&rust_root);
    let package_manifest = match rust_paths::get_package_manifest_path(&cargo_toml) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error: {}", e);
            exit(1);
        }
    };

    // `cargo publish` reads CARGO_REGISTRY_TOKEN from the environment itself;
    // only its presence is checked here, never its value.
    let has_token = env::var("CARGO_REGISTRY_TOKEN").is_ok_and(|s| !s.is_empty());

    let package_info = match rust_paths::read_package_info(&package_manifest) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Error: {}", e);
            exit(1);
        }
    };
    let name = package_info.name;
    let version = package_info.version;

    println!("Package: {}@{}", name, version);

    if name == "example-sum-package-name" {
        println!("Skipping publish: package name is the template default 'example-sum-package-name'");
        println!("Rename the package in Cargo.toml before publishing to crates.io");
        set_output("publish_result", "skipped");
        return;
    }

    println!();
    println!("=== Attempting to publish to crates.io ===");

    if has_token {
        println!("Using the crates.io token from CARGO_REGISTRY_TOKEN (trusted publishing)");
    } else {
        println!("::warning::CARGO_REGISTRY_TOKEN is not set; attempting publish with Cargo's own credentials");
        println!();
        println!("In CI the token comes from crates.io trusted publishing: the release job runs");
        println!("rust-lang/crates-io-auth-action (step id `crates-io-auth`, job permission");
        println!("`id-token: write`) and passes its output to this step:");
        println!("  env:");
        println!("    CARGO_REGISTRY_TOKEN: ${{{{ steps.crates-io-auth.outputs.token }}}}");
        println!();
        println!("If {} is not on crates.io yet, publish the first version once with:", name);
        println!("  {}", BOOTSTRAP_COMMAND);
        println!();
    }

    // Build the cargo publish command. The token stays in the environment,
    // never on argv.
    let mut cmd = Command::new("cargo");
    cmd.arg("publish").arg("--allow-dirty").arg("-p").arg(&name);

    // For multi-language repos, change to the rust directory
    if needs_cd(&rust_root) {
        cmd.current_dir(&rust_root);
    }

    let output = cmd.output().expect("Failed to execute cargo publish");

    if output.status.success() {
        println!("Successfully published {}@{} to crates.io", name, version);
        set_output("publish_result", "success");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{}\n{}", stdout, stderr);

        let kind = classify_failure(&combined);
        match kind {
            FailureKind::AlreadyExists => {
                eprintln!();
                eprintln!("=== VERSION ALREADY PUBLISHED ===");
                eprintln!();
                eprintln!("Version {} already exists on crates.io.", version);
                eprintln!("The release pipeline must always publish a version greater than what is already published.");
                eprintln!("This indicates a bug in version bumping: the pipeline should have computed a new, unpublished version.");
                eprintln!();
            }
            FailureKind::RateLimited => {
                eprintln!();
                eprintln!("=== CRATES.IO RATE LIMIT (HTTP 429) ===");
                eprintln!();
                eprintln!("crates.io rejected the publish because too many versions of this");
                eprintln!("crate have been published in the last 24 hours.");
                eprintln!();
                eprintln!("Original cargo publish error:");
                eprintln!("{}", combined.trim());
                eprintln!();
                eprintln!("This is a TRANSIENT, automatically-recoverable throttle, not a pipeline bug.");
                eprintln!("No action is required other than waiting for the 24-hour window to roll over.");
                eprintln!("scripts/check-release-needed.rs will re-attempt the same version on the next");
                eprintln!("push to 'main' once the throttle window has cleared.");
                eprintln!();
                eprintln!("See: https://doc.rust-lang.org/cargo/reference/publishing.html#publishing-a-new-version-of-an-existing-crate");
                eprintln!();
            }
            FailureKind::AuthFailed => {
                eprintln!();
                eprintln!("=== AUTHENTICATION FAILURE ===");
                eprintln!();
                eprintln!("crates.io refused the trusted-publishing token for {}.", name);
                eprintln!();
                eprintln!("Check, in this order:");
                eprintln!("  1. The crate exists on crates.io. Trusted publishing cannot create a crate;");
                eprintln!("     publish the first version once from a maintainer machine with:");
                eprintln!("       {}", BOOTSTRAP_COMMAND);
                eprintln!("  2. The crate's trusted publisher (crates.io -> crate settings -> Trusted");
                eprintln!("     Publishing) names this repository and the workflow file");
                eprintln!("     .github/workflows/release.yml, with no environment unless the job uses one.");
                eprintln!("  3. The publish step receives the token from the auth action:");
                eprintln!("       CARGO_REGISTRY_TOKEN: ${{{{ steps.crates-io-auth.outputs.token }}}}");
                eprintln!("     and the job grants `id-token: write`.");
                eprintln!();
                eprintln!("Original cargo publish error:");
                eprintln!("{}", combined.trim());
                eprintln!();
                eprintln!("See: https://crates.io/docs/trusted-publishing");
                eprintln!();
            }
            FailureKind::Unknown => {
                eprintln!("Failed to publish for unknown reason");
                eprintln!("{}", combined);
            }
        }

        set_output("publish_result", kind.output_value());

        // A rate-limit is a deferred, automatically-recoverable outcome: exit
        // successfully so the release job does not go red over a transient
        // crates.io throttle. Downstream release-artifact steps are gated on a
        // successful publish (see .github/workflows/release.yml), so a deferred
        // upload never produces partial Docker/GitHub release artifacts.
        if kind.is_deferred() {
            return;
        }

        exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_already_exists_response() {
        let body = "error: crate version 0.1.0 is already uploaded";
        assert_eq!(classify_failure(body), FailureKind::AlreadyExists);
        assert_eq!(FailureKind::AlreadyExists.output_value(), "already_exists");
    }

    #[test]
    fn classifies_rate_limit_response() {
        let body = "\
error: failed to publish my-crate v0.42.0 to registry at https://crates.io

Caused by:
  the remote server responded with an error (status 429 Too Many Requests): \
You have published too many versions of this crate in the last 24 hours
";
        assert_eq!(classify_failure(body), FailureKind::RateLimited);
        assert_eq!(FailureKind::RateLimited.output_value(), "rate_limited");
    }

    #[test]
    fn classifies_rate_limit_from_too_many_versions_marker() {
        let body = "the remote server responded: You have published too many versions";
        assert_eq!(classify_failure(body), FailureKind::RateLimited);
    }

    #[test]
    fn classifies_auth_failure_response() {
        let body = "error: failed to publish: please provide a non-empty token";
        assert_eq!(classify_failure(body), FailureKind::AuthFailed);
        assert_eq!(FailureKind::AuthFailed.output_value(), "auth_failed");
    }

    #[test]
    fn classifies_trusted_publishing_refusal_as_auth_failure() {
        let body = "\
error: failed to publish to registry at https://crates.io

Caused by:
  the remote server responded with an error (status 403 Forbidden): \
this token does not have the required permissions to perform this action
";
        assert_eq!(classify_failure(body), FailureKind::AuthFailed);
    }

    #[test]
    fn bootstrap_command_names_the_crates_io_registry() {
        assert_eq!(
            BOOTSTRAP_COMMAND,
            "package-registry-manager setup --registry crates-io --execute"
        );
    }

    #[test]
    fn classifies_unknown_response() {
        let body = "error: some brand new failure mode nobody has seen before";
        assert_eq!(classify_failure(body), FailureKind::Unknown);
        assert_eq!(FailureKind::Unknown.output_value(), "failed");
    }

    #[test]
    fn rate_limit_takes_precedence_over_auth_markers() {
        // A 429 body that also happens to mention "authentication" must still be
        // classified as rate-limited, since the throttle is the actionable cause.
        let body = "status 429 Too Many Requests: authentication retry later";
        assert_eq!(classify_failure(body), FailureKind::RateLimited);
    }

    #[test]
    fn only_rate_limit_is_deferred() {
        // A rate-limit is the single deferred (exit-0) outcome; every other
        // failure must remain a hard, non-zero error.
        assert!(FailureKind::RateLimited.is_deferred());
        assert!(!FailureKind::AlreadyExists.is_deferred());
        assert!(!FailureKind::AuthFailed.is_deferred());
        assert!(!FailureKind::Unknown.is_deferred());
    }
}
