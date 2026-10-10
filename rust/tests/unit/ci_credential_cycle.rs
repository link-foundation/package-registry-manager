use anyhow::Result;
use package_registry_manager::{
    ci_credential_cycle::{cycle_credential, secret_name, CiCredentialAdapter},
    credential_cycle::Credential,
    Registry,
};
use serde_json::{json, Value};
struct Host {
    status: &'static str,
    tests: Vec<&'static str>,
    previous: Value,
    calls: Vec<String>,
    count: usize,
}
impl Host {
    fn new(status: &'static str, tests: Vec<&'static str>) -> Self {
        Self {
            status,
            tests,
            previous: json!({"token_id":"old"}),
            calls: Vec::new(),
            count: 0,
        }
    }
}
#[allow(clippy::unused_async_trait_impl)] // Matches the asynchronous production adapter contract.
impl CiCredentialAdapter for Host {
    async fn health(&mut self) -> Result<Value> {
        Ok(json!({"status":self.status}))
    }
    async fn metadata(&mut self) -> Result<Value> {
        assert_ne!(self.status, "ok");
        Ok(self.previous.clone())
    }
    async fn create(&mut self) -> Result<Credential> {
        self.calls.push("create".into());
        self.count += 1;
        Ok(Credential {
            value: "sensitive".into(),
            id: format!("new-{}", self.count),
            expires_at: "2099-01-01T00:00:00Z".into(),
        })
    }
    async fn ensure(&mut self, credential: &Credential) -> Result<Value> {
        assert_eq!(credential.value, "sensitive");
        self.calls.push("ensure".into());
        Ok(json!({"path":"repository","fallbackReason":"organization refused"}))
    }
    async fn test(&mut self) -> Result<Value> {
        self.calls.push("test".into());
        Ok(json!({"status":self.tests.remove(0)}))
    }
    async fn revoke(&mut self, id: &str) -> Result<()> {
        self.calls.push(format!("revoke:{id}"));
        Ok(())
    }
    async fn revoked(&mut self, _: &str) -> Result<bool> {
        Ok(true)
    }
}
#[tokio::test]
async fn healthy_ci_is_noop_and_fallback_survives_verification() {
    let mut host = Host::new("ok", vec![]);
    assert_eq!(cycle_credential(&mut host).await.unwrap()["changed"], false);
    assert_eq!(host.calls, Vec::<String>::new());
    host.status = "auth-failing";
    host.tests = vec!["ok"];
    let result = cycle_credential(&mut host).await.unwrap();
    assert_eq!(result["path"], "repository");
    assert_eq!(result["fallbackReason"], "organization refused");
    assert_eq!(host.calls, ["create", "ensure", "test", "revoke:old"]);
}
#[tokio::test]
async fn unknown_absent_retries_once_and_unknown_tests_preserve_old_tokens() {
    let mut host = Host::new("unknown", vec!["auth-failing", "ok"]);
    host.previous = Value::Null;
    cycle_credential(&mut host).await.unwrap();
    assert_eq!(
        host.calls,
        [
            "create",
            "ensure",
            "test",
            "create",
            "ensure",
            "test",
            "revoke:new-1"
        ]
    );
    let mut host = Host::new("auth-failing", vec!["unknown"]);
    assert!(cycle_credential(&mut host).await.is_err());
    assert_eq!(host.calls, ["create", "ensure", "test"]);
    let mut host = Host::new("auth-failing", vec!["auth-failing", "auth-failing"]);
    assert!(cycle_credential(&mut host).await.is_err());
    assert_eq!(host.count, 2);
    assert!(!host.calls.iter().any(|item| item.starts_with("revoke")));
}
#[test]
fn naming_templates_support_registry_and_repository() {
    assert_eq!(
        secret_name(
            "{REGISTRY}_TOKEN_{REPO}",
            Registry::DockerHub,
            "team/my-app"
        )
        .unwrap(),
        "DOCKER_HUB_TOKEN_MY_APP"
    );
    assert!(secret_name("{UNKNOWN}", Registry::Npm, "team/app").is_err());
}
