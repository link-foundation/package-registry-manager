//! Registry-specific credential policy and failure-safe rotation.
use crate::model::{Registry, SetupStep, StepKind};
use anyhow::{bail, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Registry credential lifecycle metadata and browser configuration.
pub struct CredentialPolicy {
    /// Value supplied by the registry or its credential adapter.
    pub mode: String,
    /// Value supplied by the registry or its credential adapter.
    pub description: String,
}

/// Build the registry policy or the conservative browser helper.
#[must_use]
pub fn credential_policy(registry: Registry) -> CredentialPolicy {
    let (mode, description) = match registry {
        Registry::Npm | Registry::PyPi | Registry::CratesIo | Registry::RubyGems | Registry::NuGet | Registry::Jsr => ("trusted", format!("{registry}: trusted publishing; remove unused long-lived tokens after verification")),
        Registry::Ghcr => ("github-token", "ghcr: GITHUB_TOKEN with packages: write; no registry secret".into()),
        Registry::GoModules | Registry::Packagist => ("tags", format!("{registry}: public repository tags; no publishing token secret")),
        _ => ("token", format!("{registry}: scoped expiring token; gh-manager stores selected-repository organization secrets by default")),
    };
    CredentialPolicy {
        mode: mode.into(),
        description,
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
/// Registry credential lifecycle metadata and browser configuration.
pub struct CredentialMetadata {
    #[serde(default)]
    /// Value supplied by the registry or its credential adapter.
    pub present: bool,
    /// Value supplied by the registry or its credential adapter.
    pub valid: Option<bool>,
    /// Value supplied by the registry or its credential adapter.
    pub expires_at: Option<String>,
    /// Value supplied by the registry or its credential adapter.
    pub token_id: Option<String>,
}

/// Sensitive values deliberately have no Debug implementation.
pub struct Credential {
    /// Value supplied by the registry or its credential adapter.
    pub value: String,
    /// Value supplied by the registry or its credential adapter.
    pub id: String,
    /// Value supplied by the registry or its credential adapter.
    pub expires_at: String,
}

/// Build the registry policy or the conservative browser helper.
#[must_use]
pub fn needs_rotation(metadata: &CredentialMetadata, now: DateTime<Utc>) -> bool {
    !metadata.present
        || metadata.valid == Some(false)
        || metadata
            .expires_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .is_none_or(|expiry| expiry <= now + Duration::days(7))
}

#[allow(async_fn_in_trait)]
/// Exact operations required to verify replacements before revoking old tokens.
pub trait CredentialAdapter {
    /// Complete the operation without exposing credential values.
    async fn create(&mut self) -> Result<Credential>;
    /// Complete the operation without exposing credential values.
    async fn store(&mut self, credential: &Credential) -> Result<()>;
    /// Complete the operation without exposing credential values.
    async fn verify(&mut self) -> Result<()>;
    /// Complete the operation without exposing credential values.
    async fn revoke(&mut self, id: &str) -> Result<()>;
    /// Complete the operation without exposing credential values.
    async fn revoked(&mut self, id: &str) -> Result<bool>;
}

/// Verify replacement before revocation; never revoke an unverified stored replacement.
pub async fn rotate_credential(
    previous: &CredentialMetadata,
    adapter: &mut impl CredentialAdapter,
) -> Result<CredentialMetadata> {
    let mut created = adapter.create().await?;
    let valid = !created.value.is_empty()
        && !created.id.is_empty()
        && DateTime::parse_from_rfc3339(&created.expires_at)
            .is_ok_and(|expiry| expiry > Utc::now());
    if !valid {
        created.value.clear();
        bail!("registry returned no usable expiring credential");
    }
    let stored = adapter.store(&created).await;
    created.value.clear();
    if stored.is_err() {
        bail!("credential storage failed; old and replacement tokens remain active until storage is repaired");
    }
    created.value.clear();
    adapter.verify().await?;
    if let Some(id) = previous.token_id.as_deref().filter(|id| *id != created.id) {
        adapter.revoke(id).await?;
        if !adapter.revoked(id).await? {
            bail!("previous token revocation could not be verified");
        }
    }
    Ok(CredentialMetadata {
        present: true,
        valid: Some(true),
        expires_at: Some(created.expires_at),
        token_id: Some(created.id),
    })
}

/// Build the registry policy or the conservative browser helper.
#[must_use]
pub fn credential_steps(registry: Registry, secret: &str) -> Vec<SetupStep> {
    vec![SetupStep::new("manage-registry-token",&format!("Ensure and rotate {secret}"),StepKind::Api,format!("{}. Check expiry and validity, create the narrowest publishing credential in the browser, store through gh-manager stdin, verify in a dry-run workflow, then revoke and verify the replaced token. Requires gh-manager secret support and a configured verification workflow.",credential_policy(registry).description)).confirmed()]
}
