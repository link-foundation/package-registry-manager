//! Read-only Conan remote configuration and GitHub credential evidence.
use super::{Context, Result, Session, Value};
use crate::github::{GhManager, GithubGateway};
use crate::repository_identity::repository_slug;
use serde_json::json;

impl Session<'_> {
    pub(super) async fn audit_distribution_channel(&mut self) -> Result<()> {
        let distribution = self
            .plan
            .package
            .audit
            .get("distribution")
            .cloned()
            .unwrap_or_default();
        let slug =
            repository_slug(&self.plan.repository).context("GitHub repository is missing")?;
        let mut args = vec![
            "variable".into(),
            "get".into(),
            "CONAN_REMOTE_URL".into(),
            "--effective".into(),
            "--repo".into(),
            slug.clone(),
        ];
        if let Some(environment) = distribution["environment"].as_str() {
            args.extend(["--env".into(), environment.into()]);
        }
        let configured = if distribution["remote"].is_string() {
            json!({"verified":true,"value":distribution["remote"]})
        } else {
            GhManager.call(&args, None, self.options.repository).await?
        };
        let remote_status = configured["value"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map_or(
                if configured["verified"] == true {
                    "missing"
                } else {
                    "unknown"
                },
                |value| {
                    reqwest::Url::parse(value).map_or("invalid", |url| {
                        if configured["verified"] == true
                            && matches!(url.scheme(), "http" | "https")
                            && url.username().is_empty()
                            && url.password().is_none()
                            && url.query().is_none()
                            && url.fragment().is_none()
                        {
                            "configured"
                        } else {
                            "invalid-or-secret-bearing"
                        }
                    })
                },
            );
        let mut credentials = Vec::new();
        for name in ["CONAN_LOGIN_USERNAME", "CONAN_PASSWORD"] {
            let result = GhManager
                .call(
                    &[
                        "secret".into(),
                        "health".into(),
                        name.into(),
                        "--repo".into(),
                        slug.clone(),
                        "--failure-pattern".into(),
                        "conan.*(?:unauthorized|forbidden|authentication)".into(),
                    ],
                    None,
                    self.options.repository,
                )
                .await;
            let result = result.unwrap_or_else(|_| json!({"status":"unknown","evidence":[{"reason":"credential-health-unreadable"}]}));
            let evidence: Vec<_> = result["evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| {
                    let mut safe = serde_json::Map::new();
                    for field in [
                        "reason", "status", "workflow", "job", "step", "runId", "runUrl",
                    ] {
                        if let Some(value) = entry.get(field) {
                            safe.insert(field.into(), value.clone());
                        }
                    }
                    Value::Object(safe)
                })
                .collect();
            credentials.push(json!({"name":name,"status":result["status"],"evidence":evidence,"limitation":"health is repository workflow evidence; effective environment credentials require the selected CI job"}));
        }
        println!(
            "{}",
            json!({"provider":"conan","scope":{"repo":slug,"environment":distribution["environment"]},"remote":{"status":remote_status},"credentials":credentials,"publication":"unverified","ci_validation":["conan create . --build=missing","run the recipe test_package consumer with the reviewed profiles","verify the exact reference on the chosen remote after reviewed CI publication"]})
        );
        self.values
            .insert("distribution_unverified".into(), "true".into());
        Ok(())
    }
}
