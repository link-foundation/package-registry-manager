//! Conservative evidence of a real upload through a token-free OIDC workflow.
use crate::Registry;
use serde_json::Value;
/// Skipped publishing steps and no-op releases never authorize secret deletion.
#[must_use]
pub fn published_through_oidc(job: &Value, log: &str, registry: Registry) -> bool {
    if job["conclusion"] != "success" {
        return false;
    }
    let steps: Vec<_> = job["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|step| step["conclusion"] == "success")
        .collect();
    let publication = regex::Regex::new("(?i)publish|upload|twine").expect("valid pattern");
    if !steps
        .iter()
        .any(|step| publication.is_match(step["name"].as_str().unwrap_or_default()))
    {
        return false;
    }
    if registry != Registry::CratesIo
        && (steps.iter().any(|step| {
            step["name"]
                .as_str()
                .is_some_and(|name| name.to_lowercase().contains("token fallback"))
        }) || !steps.iter().any(|step| {
            step["name"]
                .as_str()
                .is_some_and(|name| name.contains("OIDC"))
        }))
    {
        return false;
    }
    match registry {
        Registry::CratesIo => {
            steps.iter().any(|step| {
                regex::Regex::new("(?i)Authenticate with crates.io|crates-io-auth-action")
                    .expect("valid pattern")
                    .is_match(step["name"].as_str().unwrap_or_default())
            }) && regex::Regex::new(r"Published\s+\S+\s+v\d")
                .expect("valid pattern")
                .is_match(log)
        }
        Registry::Npm => regex::Regex::new(r"(?m)^.*\+\s+(?:@[^/\s]+/)?[^@\s]+@\d")
            .expect("valid pattern")
            .is_match(log),
        Registry::PyPi => regex::Regex::new(r"Successfully uploaded|View at:\s*https://pypi\.org")
            .expect("valid pattern")
            .is_match(log),
        _ => false,
    }
}
