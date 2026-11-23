//! npm's browser sign-in and approval links.
//!
//! How long a link stays valid, how the tool recognizes an expired one, and
//! how often it asks npm for a fresh one.

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{bail, Result};
use chrono::NaiveTime;
use regex::Regex;

use crate::auth_urls::{CommandOutput, ANSI_PATTERN};

pub(crate) mod browser;

static DONE_NOT_FOUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\b404\b[^\r\n]*/-/v1/done\b|/-/v1/done\b[^\r\n]*\b404\b)")
        .expect("static pattern must compile")
});

static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ANSI_PATTERN).expect("static pattern must compile"));

static TRUST_STORED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Trust configuration created successfully|npm stored the trusted publisher")
        .expect("static pattern must compile")
});

pub(crate) fn trust_stored(output: &str) -> bool {
    TRUST_STORED.is_match(&ANSI.replace_all(output, ""))
}

/// How long npm keeps a web login or approval session open.
pub const APPROVAL_WINDOW: Duration = Duration::from_secs(5 * 60);
/// How many links the tool requests before it gives up on one step.
pub const APPROVAL_ATTEMPTS: usize = 3;
/// npm's error once its approval session expired while it polled for the result.
pub const EXPIRED_APPROVAL: &str = r"(?i)Invalid response from web login endpoint";
/// Shown with every npm approval link.
pub const TWO_FACTOR_HINT: &str = "If npm's approval page offers to skip two-factor checks for the next 5 minutes, choose it so the approvals that follow need no new confirmation.";

/// What a web-authentication link asks the maintainer to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// `Login at:`: sign in to npm.
    Login,
    /// `Authenticate your account at:`: approve a publish or a trust change.
    Approve,
}

/// The deadline printed with a sign-in or approval link, such as
/// "Sign in within about 5 minutes (until 19:05).".
#[must_use]
pub fn approval_deadline(kind: LinkKind, now: NaiveTime) -> String {
    let window = chrono::Duration::from_std(APPROVAL_WINDOW).unwrap_or_default();
    let (until, _) = now.overflowing_add_signed(window);
    let action = match kind {
        LinkKind::Login => "Sign in",
        LinkKind::Approve => "Approve",
    };
    format!(
        "{action} within about 5 minutes (until {}).",
        until.format("%H:%M")
    )
}

/// Why an npm run ended on an expired link, or `None` when it did not: a web
/// login falls back to npm's legacy `Username:` prompt, and a publish or trust
/// approval fails with npm's web-login error.
#[must_use]
pub const fn expired_approval(output: &CommandOutput) -> Option<&'static str> {
    if output.legacy_login {
        Some("npm fell back to its legacy username prompt")
    } else if output.code != 0 && output.approval_expired {
        Some("npm's approval session ended")
    } else {
        None
    }
}

/// Decides what follows attempt number `attempt` (counted from 1) of `attempts`.
///
/// Returns `None` when its link did not expire, and the message announcing a
/// fresh link when another attempt remains.
///
/// # Errors
/// Fails once the link expired `attempts` times.
pub fn next_link(
    output: &CommandOutput,
    attempt: usize,
    attempts: usize,
) -> Result<Option<String>> {
    expired_approval(output)
        .map(|reason| retry_expired(reason, attempt, attempts))
        .transpose()
}

fn retry_expired(reason: &str, attempt: usize, attempts: usize) -> Result<String> {
    if attempt >= attempts {
        bail!(
            "the browser link expired {attempts} times ({reason}); re-run the command when you are ready to approve within 5 minutes"
        );
    }
    Ok(format!(
        "\nThe link expired before it was approved ({reason}); requesting a fresh one (attempt {} of {attempts}).",
        attempt + 1
    ))
}

pub(crate) fn next_approval(
    output: &CommandOutput,
    linked: bool,
    page_failure: Option<&str>,
    attempt: usize,
) -> Result<Option<String>> {
    if output.code == 0 {
        return Ok(None);
    }
    if let Some(reason) = page_failure {
        if attempt >= APPROVAL_ATTEMPTS {
            bail!("npm approval failed {APPROVAL_ATTEMPTS} times ({reason})");
        }
        return Ok(Some(retry(reason, attempt)));
    }
    if linked && DONE_NOT_FOUND.is_match(&output.stderr) {
        return retry_expired("npm's approval session ended", attempt, APPROVAL_ATTEMPTS).map(Some);
    }
    if let Some(message) = next_link(output, attempt, APPROVAL_ATTEMPTS)? {
        return Ok(Some(message));
    }
    Ok((linked && attempt < APPROVAL_ATTEMPTS)
        .then(|| retry(&format!("npm exited with status {}", output.code), attempt)))
}

fn retry(reason: &str, attempt: usize) -> String {
    format!(
        "\nnpm approval failed ({reason}); requesting a fresh one (attempt {} of {APPROVAL_ATTEMPTS}).",
        attempt + 1
    )
}

/// The closing note once a package publishes through trusted publishing.
#[must_use]
pub fn oidc_release_note(workflow: &str) -> String {
    format!(
        "Future releases publish from {workflow} through trusted publishing; no login is needed."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_approval_retry_classifies_page_errors_done_404_and_exit_status() -> Result<()> {
        let mut output = CommandOutput {
            code: 1,
            stdout: String::new(),
            stderr: String::new(),
            legacy_login: false,
            approval_expired: false,
        };
        let page = next_approval(&output, true, Some("Something went wrong"), 1)?;
        assert!(page.unwrap().contains("Something went wrong"));
        let failure = next_approval(&output, true, None, 1)?;
        assert!(failure.unwrap().contains("npm exited with status 1"));
        assert!(next_approval(&output, false, None, 1)?.is_none());
        output.stderr =
            "npm error 404 Not Found - GET https://registry.npmjs.org/-/v1/done?authId=fake"
                .to_owned();
        let expired = next_approval(&output, true, None, 2)?;
        assert!(expired.unwrap().contains("approval session ended"));
        output.code = 0;
        assert!(next_approval(&output, true, Some("Something went wrong"), 2)?.is_none());
        Ok(())
    }
}
