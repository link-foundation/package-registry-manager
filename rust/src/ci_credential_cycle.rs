//! CI-evidence-driven token lifecycle, with bounded authentication-only retry.
use crate::{credential_cycle::Credential, Registry};
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Registry secret naming templates, resolved before any mutation.
pub fn secret_name(template: &str, registry: Registry, slug: &str) -> Result<String> {
    let (owner, repo) = slug.split_once('/').unwrap_or(("", ""));
    let normalize = |text: &str| {
        text.to_ascii_uppercase()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let mut name = template.to_owned();
    for (key, value) in [
        ("REGISTRY", normalize(&registry.to_string())),
        ("REPO", normalize(repo)),
        ("ORG", normalize(owner)),
        ("OWNER", normalize(owner)),
    ] {
        name = name.replace(&format!("{{{key}}}"), &value);
    }
    if !regex::Regex::new(r"^[A-Z_][A-Z0-9_]{0,244}$")
        .expect("static pattern")
        .is_match(&name)
        || name.starts_with("GITHUB_")
    {
        bail!("invalid registry secret name or naming template");
    }
    Ok(name)
}

/// Operations required by CI-driven setup; token values never enter diagnostics.
#[allow(async_fn_in_trait)]
pub trait CiCredentialAdapter {
    /// Inspect the latest CI evidence.
    async fn health(&mut self) -> Result<Value>;
    /// Read secret presence and nonsensitive token identifiers.
    async fn metadata(&mut self) -> Result<Value>;
    /// Create a scoped token in the dedicated browser.
    async fn create(&mut self) -> Result<Credential>;
    /// Ensure organization access, falling back to repository storage.
    async fn ensure(&mut self, credential: &Credential) -> Result<Value>;
    /// Trigger and classify CI after storage.
    async fn test(&mut self) -> Result<Value>;
    /// Revoke a superseded token after successful CI.
    async fn revoke(&mut self, id: &str) -> Result<()>;
    /// Verify revocation in registry settings.
    async fn revoked(&mut self, id: &str) -> Result<bool>;
}

/// IDs can be revoked only when every tracked consumer is being replaced.
#[must_use]
pub fn tracked_token_ids(state: &Value, repos: &[String], secret: &str) -> Vec<Value> {
    let keys: std::collections::BTreeSet<_> = repos
        .iter()
        .map(|repo| format!("{repo}:{secret}"))
        .collect();
    let mut ids = Vec::new();
    for key in &keys {
        for id in state[key].as_array().into_iter().flatten() {
            let retained = state.as_object().is_some_and(|entries| {
                entries.iter().any(|(entry, values)| {
                    !keys.contains(entry)
                        && values.as_array().is_some_and(|items| items.contains(id))
                })
            });
            if !retained && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

/// Remember candidates for every consumer before verification, including failures.
pub fn record_token_id(state: &mut Value, repos: &[String], secret: &str, id: &str) {
    for repo in repos {
        let key = format!("{repo}:{secret}");
        if !state[&key].is_array() {
            state[&key] = json!([]);
        }
        let ids = state[&key].as_array_mut().expect("state list");
        let id = json!(id);
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
}

/// Associate a verified replacement with all its targets after revocation completes.
pub fn complete_rotation(state: &mut Value, repos: &[String], secret: &str, id: &str) {
    for repo in repos {
        state[format!("{repo}:{secret}")] = json!([id]);
    }
}

/// Keep healthy secrets, test uncertain existing secrets, and retry auth rejection once.
pub async fn cycle_credential(adapter: &mut impl CiCredentialAdapter) -> Result<Value> {
    let health = adapter.health().await?;
    if health["status"] == "ok" {
        return Ok(json!({"status":"ok","changed":false}));
    }
    let previous = adapter.metadata().await?;
    if health["status"] == "unknown" && !previous.is_null() && previous["present"] != false {
        let tested = adapter.test().await?;
        if tested["status"] == "ok" {
            return Ok(json!({"status":"ok","changed":false}));
        }
        if tested["status"] != "auth-failing" {
            bail!("credential verification is unknown; existing credential retained");
        }
    }
    let mut ids = std::collections::BTreeSet::new();
    if let Some(id) = previous["token_id"].as_str() {
        ids.insert(id.to_owned());
    }
    if let Some(previous_ids) = previous["token_ids"].as_array() {
        ids.extend(
            previous_ids
                .iter()
                .filter_map(|id| id.as_str().map(str::to_owned)),
        );
    }
    for _ in 0..2 {
        let mut candidate = adapter.create().await?;
        if candidate.value.is_empty()
            || candidate.id.is_empty()
            || (!candidate.expires_at.is_empty()
                && !chrono::DateTime::parse_from_rfc3339(&candidate.expires_at)
                    .is_ok_and(|date| date > chrono::Utc::now()))
        {
            candidate.value.clear();
            bail!("registry returned no usable credential");
        }
        let stored = adapter.ensure(&candidate).await;
        candidate.value.clear();
        let mut stored = stored?;
        let tested = adapter.test().await?;
        if tested["status"] == "ok" {
            ids.remove(&candidate.id);
            for id in ids {
                adapter.revoke(&id).await?;
                if !adapter.revoked(&id).await? {
                    bail!("previous token revocation could not be verified");
                }
            }
            stored["status"] = json!("ok");
            stored["changed"] = json!(true);
            stored["token_id"] = json!(candidate.id);
            return Ok(stored);
        }
        ids.insert(candidate.id);
        if tested["status"] != "auth-failing" {
            bail!("credential verification is unknown; old and replacement credentials retained");
        }
    }
    bail!("credential verification is auth-failing after two candidates; old credentials retained")
}
