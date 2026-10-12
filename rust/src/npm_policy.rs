//! Bounded npm name-policy checks before browser approvals.

use crate::registry_state::npm_name;
use anyhow::{bail, Result};

/// Enumerate punctuation variants with a finite upper bound.
#[must_use]
pub fn name_variants(name: &str) -> Vec<String> {
    let (scope, leaf) = name
        .rsplit_once('/')
        .map_or((String::new(), name), |(scope, leaf)| {
            (format!("{scope}/"), leaf)
        });
    let mut pieces = leaf
        .split(['.', '_', '-'])
        .filter(|piece| !piece.is_empty());
    let mut variants = vec![pieces.next().unwrap_or_default().to_owned()];
    for piece in pieces {
        variants = variants
            .iter()
            .flat_map(|prefix| {
                ["", "-", ".", "_"].map(|separator| format!("{prefix}{separator}{piece}"))
            })
            .take(64)
            .collect();
    }
    variants
        .into_iter()
        .map(|variant| format!("{scope}{variant}"))
        .filter(|variant| variant != name)
        .collect()
}

/// Check exact/normalized variants; dry-run cannot guarantee server acceptance.
pub async fn check_name_policy(name: &str, base: &str) -> Result<()> {
    check_name_policy_with_scope(name, base, None).await
}

/// Check naming policy with the repository owner's proposed npm scope.
pub async fn check_name_policy_with_scope(
    name: &str,
    base: &str,
    scope: Option<&str>,
) -> Result<()> {
    let valid = regex::Regex::new(r"^(?:@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*$")
        .expect("static pattern");
    if name.len() > 214 || !valid.is_match(name) || ["node_modules", "favicon.ico"].contains(&name)
    {
        bail!("npm name '{name}' is invalid; choose a lowercase descriptive name or @owner/name");
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("package-registry-manager")
        .build()?;
    for variant in std::iter::once(name.to_owned()).chain(name_variants(name)) {
        let response = client
            .get(format!(
                "{base}/{}?_={}",
                npm_name(&variant),
                chrono::Utc::now().timestamp_millis()
            ))
            .header("Cache-Control", "no-cache, no-store")
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            continue;
        }
        if !response.status().is_success() {
            bail!(
                "npm name policy lookup failed ({}); no approval requested",
                response.status()
            );
        }
        if variant != name {
            bail!(
                "{}",
                crate::npm_similarity::naming_error(name, &variant, base, scope, &[]).await
            );
        }
        return Ok(());
    }
    crate::npm_similarity::check_similarity(name, base, scope).await
}

/// Stop server-side name policy refusals instead of retrying authentication.
#[must_use]
pub fn policy_refusal(output: &str) -> bool {
    regex::Regex::new(r"(?i)\bE403\b|403 Forbidden")
        .expect("static pattern")
        .is_match(output)
        && regex::Regex::new(r"(?i)too similar|package name.*(?:forbidden|blocked|not allowed)")
            .expect("static pattern")
            .is_match(output)
}
