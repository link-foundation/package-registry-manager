//! `NuGet` profile, publisher identity and publication evidence remain independent.
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Reject unresolved account, repository and package placeholders before mutation.
pub fn validate(policy: &Value) -> Result<()> {
    let placeholders = regex::Regex::new(
        r"(?i)^(MyPackage|Your[ -]?Name|your[-_].*|OWNER|REPO|example|placeholder)$",
    )?;
    for field in [
        "username",
        "package_owner",
        "organization",
        "repository",
        "workflow",
        "owner_id",
        "repository_id",
    ] {
        let value = policy[field].as_str().unwrap_or_default();
        if value.trim().is_empty() || placeholders.is_match(value) {
            bail!("NuGet policy requires an actual {field}");
        }
    }
    if ["owner_id", "repository_id"].iter().any(|field| {
        !policy[field]
            .as_str()
            .is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
    }) {
        bail!("NuGet policy requires immutable numeric GitHub IDs");
    }
    if !regex::Regex::new(r"^[\w.-]+\.ya?ml$")?
        .is_match(policy["workflow"].as_str().unwrap_or_default())
    {
        bail!("NuGet policy requires a workflow filename");
    }
    let ids = policy["package_ids"].as_array().filter(|v| !v.is_empty());
    if ids.is_none_or(|ids| {
        ids.iter().any(|v| {
            v.as_str().is_none_or(|v| {
                !regex::Regex::new(r"^[a-zA-Z0-9_.-]+$")
                    .expect("regex")
                    .is_match(v)
                    || placeholders.is_match(v)
            })
        })
    }) {
        bail!("NuGet policy requires exact, non-template package IDs");
    }
    if policy["operations"]
        .as_array()
        .filter(|v| v.len() == 1)
        .is_none_or(|v| {
            v.iter()
                .any(|v| !matches!(v.as_str(), Some("package:push" | "package:pushversion")))
        })
    {
        bail!("NuGet policy requires one explicit publish operation");
    }
    Ok(())
}
fn same(a: &Value, b: &Value) -> bool {
    a.as_str()
        .unwrap_or_default()
        .eq_ignore_ascii_case(b.as_str().unwrap_or_default())
}
fn set(value: &Value) -> Vec<String> {
    let mut v = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    v.sort();
    v
}
/// Equivalent names with unbound immutable IDs are pending, never recreated.
pub fn state(policies: &[Value], expected: &Value, actual_username: &str) -> Result<Value> {
    validate(expected)?;
    if !actual_username.eq_ignore_ascii_case(expected["username"].as_str().unwrap_or_default()) {
        return Ok(json!({"status":"wrong-account","verified":false}));
    }
    let matches = policies
        .iter()
        .filter(|p| {
            ["package_owner", "organization", "repository"]
                .iter()
                .all(|k| same(&p[k], &expected[k]))
                && ["workflow", "environment"].iter().all(|k| {
                    p[k].as_str().unwrap_or_default() == expected[k].as_str().unwrap_or_default()
                })
                && ["package_ids", "operations"]
                    .iter()
                    .all(|k| set(&p[k]) == set(&expected[k]))
        })
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Ok(json!({"status":"duplicate","verified":false}));
    }
    let Some(policy) = matches.first() else {
        return Ok(json!({"status":"missing-or-mismatched","verified":false}));
    };
    let bound = ["owner_id", "repository_id"]
        .iter()
        .all(|k| same(&policy[k], &expected[k]));
    let activation = policy["activation"].as_str().unwrap_or("unknown");
    Ok(
        json!({"status":if bound{activation}else{"immutable-identity-unverified"},"verified":bound&&activation=="active","policy_id":policy["id"]}),
    )
}
/// `PackageVerify` is not a push authorization check. Never print credentials.
#[must_use]
pub fn verification_evidence(
    status: u16,
    body: &str,
    secrets: &[&str],
    derived_key: bool,
) -> Value {
    let mut diagnostic = body.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        diagnostic = diagnostic.replace(secret, "[REDACTED]");
    }
    diagnostic = diagnostic.chars().take(4000).collect();
    json!({"http_status":status,"diagnostic":diagnostic,"authentication":if [401,403].contains(&status){"rejected-by-verification-endpoint"}else if (200..300).contains(&status){"accepted-by-verification-endpoint"}else{"unknown"},"authorization":"unknown","publication":"unverified","reason":if derived_key{"derived verification key changes the subject; original policy package scope is unproven"}else if status==400{"PackageVerify may require package:verify; a push-only key is not proven invalid"}else{"verification endpoint evidence is separate from actual push acceptance"}})
}
/// Secret values deliberately have no Debug/Serialize implementation.
pub struct Login {
    /// Job-local key; never serialize or log it.
    pub key: Option<String>,
    /// Exchange outcome, independent of publication.
    pub status: String,
    /// Sanitized explanation of an unavailable or rejected exchange.
    pub reason: Option<String>,
}
#[allow(async_fn_in_trait)]
/// The same validated artifact is reused for at most one existing-token fallback.
pub trait PublishAdapter {
    /// Explicit `NuGet` profile username, independent of the package owner.
    fn username(&self) -> Option<&str>;
    /// Existing caller-designated fallback; never create a persistent key.
    fn fallback(&self) -> Option<String>;
    /// Exchange GitHub OIDC through the official login operation.
    async fn login(&mut self) -> Result<Login>;
    /// Fingerprint the already validated artifact before every attempt.
    async fn fingerprint(&mut self, artifact: &str) -> Result<String>;
    /// Attempt publication and return classified, sanitized method evidence.
    async fn push(&mut self, artifact: &str, key: &str) -> Result<Value>;
    /// Separate absence from transport or authorization uncertainty.
    async fn exact_version_exists(&mut self, artifact: &str) -> Result<Option<bool>>;
}
/// OIDC success never checks an invalid fallback and never creates persistent keys.
pub async fn publish(artifact: &str, adapter: &mut impl PublishAdapter) -> Result<Value> {
    let login = if adapter.username().is_some_and(|v| !v.is_empty()) {
        adapter.login().await.unwrap_or_else(|_| Login {
            key: None,
            status: "failed".into(),
            reason: Some("OIDC exchange failed".into()),
        })
    } else {
        Login {
            key: None,
            status: "unavailable".into(),
            reason: Some("NuGet profile username is missing".into()),
        }
    };
    let mut attempts =
        vec![json!({"method":"trusted-publishing","status":login.status,"reason":login.reason})];
    let fingerprint = adapter.fingerprint(artifact).await?;
    for (method, key) in [
        ("trusted-publishing", login.key),
        ("token-fallback", adapter.fallback()),
    ] {
        let Some(key) = key.filter(|v| !v.is_empty()) else {
            continue;
        };
        if adapter.fingerprint(artifact).await? != fingerprint {
            bail!("validated NuGet artifact changed before retry");
        }
        let result = adapter.push(artifact, &key).await?;
        attempts.push(json!({"method":method,"status":result["status"],"reason":result["reason"]}));
        let publication = if result["status"] == "accepted" {
            "accepted"
        } else if !matches!(
            result["status"].as_str(),
            Some("authentication-failed" | "authorization-failed")
        ) {
            "unverified"
        } else {
            match adapter.exact_version_exists(artifact).await? {
                Some(true) => "already-present",
                None => "unknown",
                Some(false) => continue,
            }
        };
        return Ok(
            json!({"selected_method":method,"attempts":attempts,"publication":publication,"trust_repaired":method=="trusted-publishing"&&publication=="accepted"}),
        );
    }
    Ok(
        json!({"selected_method":"none","attempts":attempts,"publication":"unverified","trust_repaired":false}),
    )
}
