//! Bounded npm similarity heuristics and verified free-name suggestions.
use crate::registry_state::{encode_uri_component, npm_name};
use anyhow::{bail, Result};

/// Compare punctuation-free names with a small Damerau edit distance.
#[must_use]
pub fn similar_name(name: &str, candidate: &str) -> bool {
    if name.starts_with('@') || candidate.starts_with('@') {
        return false;
    }
    let normalize = |text: &str| {
        text.to_ascii_lowercase()
            .chars()
            .filter(|c| !['.', '_', '-'].contains(c))
            .collect::<Vec<_>>()
    };
    let a = normalize(name);
    let b = normalize(candidate);
    if a == b {
        return true;
    }
    if a.len().abs_diff(b.len()) > 2 || a.len() > 214 || b.len() > 214 {
        return false;
    }
    let mut rows = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            rows[i][j] = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                rows[i][j] = rows[i][j].min(rows[i - 2][j - 2] + 1);
            }
        }
    }
    rows[a.len()][b.len()] <= if a.len().min(b.len()) >= 6 { 2 } else { 1 }
}

/// Only advertise names whose fresh registry lookup returns 404.
pub async fn free_alternatives(
    name: &str,
    base: &str,
    scope: Option<&str>,
    proposed: &[String],
) -> Result<Vec<String>> {
    let client = client()?;
    let leaf = name.rsplit('/').next().unwrap_or(name);
    let mut candidates = proposed.to_vec();
    if let Some(scope) = scope {
        candidates.push(format!("@{}/{leaf}", scope.trim_start_matches('@')));
    }
    candidates.extend([format!("{leaf}-package"), format!("{leaf}-toolkit")]);
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    let valid = regex::Regex::new(r"^(?:@[a-z0-9][a-z0-9._-]*/)?[a-z0-9][a-z0-9._-]*$")
        .expect("static pattern");
    let mut free = Vec::new();
    for candidate in candidates
        .into_iter()
        .filter(|v| v != name && v.len() <= 214 && valid.is_match(v))
        .take(8)
    {
        if let Ok(response) = client
            .get(format!(
                "{base}/{}?_={}",
                npm_name(&candidate),
                chrono::Utc::now().timestamp_millis()
            ))
            .header("Cache-Control", "no-cache, no-store")
            .send()
            .await
        {
            if response.status() == reqwest::StatusCode::NOT_FOUND && !free.contains(&candidate) {
                free.push(candidate);
            }
        }
    }
    Ok(free)
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("package-registry-manager")
        .build()?)
}

/// Build a naming diagnostic without retrying authentication.
pub async fn naming_error(
    name: &str,
    conflicting: &str,
    base: &str,
    scope: Option<&str>,
    proposed: &[String],
) -> String {
    let free = free_alternatives(name, base, scope, proposed)
        .await
        .unwrap_or_default();
    let alternatives = if free.is_empty() {
        "choose your own npm scope or a longer descriptive name; availability could not be confirmed".to_owned()
    } else {
        format!(
            "currently free alternatives: {} (scope ownership and npm policy still apply)",
            free.join(", ")
        )
    };
    format!("npm name '{name}' may be refused as too similar to existing '{conflicting}'; {alternatives}. Rename package.json and retry; no authentication retry was attempted.")
}

/// Search popular and related public names; private npm policy remains authoritative.
pub async fn check_similarity(name: &str, base: &str, scope: Option<&str>) -> Result<()> {
    if name.starts_with('@') {
        return Ok(());
    }
    let client = client()?;
    let tail = name.rsplit(['.', '-', '_']).next().unwrap_or(name);
    // Check common public monikers even when search does not correct a typo.
    for candidate in [
        "lodash",
        "express",
        "react",
        "typescript",
        "webpack",
        "eslint",
        "prettier",
        "commander",
        "axios",
        "chalk",
        "debug",
        "dotenv",
        "yargs",
        "semver",
        "glob",
        "minimist",
        "moment",
        "jquery",
        "rxjs",
        "tslib",
    ] {
        if similar_name(name, candidate)
            && client
                .get(format!("{base}/{candidate}"))
                .send()
                .await?
                .status()
                .is_success()
        {
            bail!("{}", naming_error(name, candidate, base, scope, &[]).await);
        }
    }
    let mut queries = vec![name, tail];
    queries.dedup();
    for query in queries {
        let response = client
            .get(format!(
                "{base}/-/v1/search?text={}&size=250&popularity=1&quality=0&maintenance=0",
                encode_uri_component(query)
            ))
            .send()
            .await?;
        if !response.status().is_success() {
            bail!(
                "npm similarity search failed ({}); no approval requested",
                response.status()
            );
        }
        let document: serde_json::Value = serde_json::from_str(&response.text().await?)?;
        for item in document["objects"]
            .as_array()
            .into_iter()
            .flatten()
            .take(250)
        {
            if let Some(candidate) = item["package"]["name"].as_str() {
                if similar_name(name, candidate) {
                    bail!("{}", naming_error(name, candidate, base, scope, &[]).await);
                }
            }
        }
    }
    Ok(())
}
