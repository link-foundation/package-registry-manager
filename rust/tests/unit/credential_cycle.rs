use anyhow::{bail, Result};
use chrono::{Duration, Utc};
use package_registry_manager::credential_cycle::{
    credential_policy, needs_rotation, rotate_credential, Credential, CredentialAdapter,
    CredentialMetadata,
};
use package_registry_manager::Registry;

#[test]
fn oidc_and_builtin_credentials_take_precedence() {
    for registry in [
        Registry::Npm,
        Registry::PyPi,
        Registry::CratesIo,
        Registry::RubyGems,
        Registry::NuGet,
        Registry::Jsr,
    ] {
        assert_eq!(credential_policy(registry).mode, "trusted");
    }
    assert_eq!(credential_policy(Registry::Ghcr).mode, "github-token");
    for registry in [
        Registry::DockerHub,
        Registry::MavenCentral,
        Registry::VsCodeMarketplace,
        Registry::OpenVsx,
        Registry::ChromeWebStore,
    ] {
        assert_eq!(credential_policy(registry).mode, "token");
    }
}
#[test]
fn rotation_detects_missing_expiring_and_rejected_credentials() {
    let mut state = CredentialMetadata::default();
    assert!(needs_rotation(&state, Utc::now()));
    state.present = true;
    state.expires_at = Some((Utc::now() + Duration::days(30)).to_rfc3339());
    assert!(!needs_rotation(&state, Utc::now()));
    state.valid = Some(false);
    assert!(needs_rotation(&state, Utc::now()));
    state.valid = Some(true);
    state.expires_at = Some((Utc::now() + Duration::days(2)).to_rfc3339());
    assert!(needs_rotation(&state, Utc::now()));
}
struct Host {
    calls: Vec<String>,
    fail: bool,
}
impl CredentialAdapter for Host {
    async fn create(&mut self) -> Result<Credential> {
        tokio::task::yield_now().await;
        self.calls.push("create".into());
        Ok(Credential {
            value: "sensitive".into(),
            id: "new".into(),
            expires_at: (Utc::now() + Duration::days(30)).to_rfc3339(),
        })
    }
    async fn store(&mut self, credential: &Credential) -> Result<()> {
        assert_eq!(credential.value, "sensitive");
        tokio::task::yield_now().await;
        self.calls.push("store".into());
        Ok(())
    }
    async fn verify(&mut self) -> Result<()> {
        tokio::task::yield_now().await;
        self.calls.push("verify".into());
        if self.fail {
            bail!("rejected login");
        }
        Ok(())
    }
    async fn revoke(&mut self, id: &str) -> Result<()> {
        tokio::task::yield_now().await;
        self.calls.push(format!("revoke:{id}"));
        Ok(())
    }
    async fn revoked(&mut self, id: &str) -> Result<bool> {
        tokio::task::yield_now().await;
        self.calls.push(format!("revoked:{id}"));
        Ok(true)
    }
}
#[tokio::test]
async fn validates_replacement_before_revoking_old_token() {
    let mut host = Host {
        calls: Vec::new(),
        fail: false,
    };
    let previous = CredentialMetadata {
        token_id: Some("old".into()),
        ..CredentialMetadata::default()
    };
    rotate_credential(&previous, &mut host).await.unwrap();
    assert_eq!(
        host.calls,
        ["create", "store", "verify", "revoke:old", "revoked:old"]
    );
}
#[tokio::test]
async fn failed_validation_retains_both_active_tokens() {
    let mut host = Host {
        calls: Vec::new(),
        fail: true,
    };
    let previous = CredentialMetadata {
        token_id: Some("old".into()),
        ..CredentialMetadata::default()
    };
    assert!(rotate_credential(&previous, &mut host).await.is_err());
    assert_eq!(host.calls, ["create", "store", "verify"]);
}
