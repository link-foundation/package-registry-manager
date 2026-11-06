//! First-release signing proof, encrypted settings and independent public verification.
use crate::central_packagist::{plan_slug, setting_presence, step_setting_names, StepContext};
use crate::github::GithubGateway;
use crate::model::SetupStep;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::{io::AsyncWriteExt, process::Command};

/// The shared GnuPG driver receives private configuration on stdin only.
async fn signing(
    settings: &Value,
    slug: &str,
    proof: &Value,
    cwd: &std::path::Path,
) -> Result<Value> {
    let source = include_str!("central-gpg.mjs");
    let driver = format!("{source}\nlet input=''; for await (const chunk of process.stdin) input += chunk; const request=JSON.parse(input); try {{ process.stdout.write(JSON.stringify(await provisionSigning(request.settings, request.slug, request.proof))); }} catch {{ process.exitCode=1; }}");
    let mut child = Command::new("node")
        .args(["--input-type=module", "-e", &driver])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Node.js and GnuPG are required for Central signing")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(
                json!({"settings":settings,"slug":slug,"proof":proof})
                    .to_string()
                    .as_bytes(),
            )
            .await?;
    }
    let output = child.wait_with_output().await?;
    anyhow::ensure!(
        output.status.success(),
        "signing or independent keyserver verification failed; private diagnostics withheld"
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

async fn metadata(
    github: &impl GithubGateway,
    slug: &str,
    names: &[String],
    cwd: &std::path::Path,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for name in names {
        result.push(
            github
                .call(
                    &[
                        "secret".into(),
                        "get-metadata".into(),
                        name.clone(),
                        "--repo".into(),
                        slug.into(),
                    ],
                    None,
                    cwd,
                )
                .await?,
        );
    }
    Ok(result)
}

/// Verification never treats a public .asc URL as proof of private signing capability.
#[allow(clippy::future_not_send)]
pub async fn ensure(
    context: &StepContext<'_, impl GithubGateway>,
    step: &SetupStep,
    authorized: bool,
) -> Result<Value> {
    let names = step_setting_names(step);
    let Some(slug) = plan_slug(context.plan).filter(|_| names.len() == 2) else {
        return Ok(
            json!({"status":"unsupported","reason":"signing requires exact repository and two selected secrets"}),
        );
    };
    let cwd = context.cwd;
    let github = context.github;
    let original = metadata(github, &slug, &names, cwd).await?;
    let (present, _) = setting_presence(github, &slug, &names, cwd).await?;
    let config: Value = match std::fs::read(cwd.join(".package-registry-manager.json")) {
        Ok(body) => serde_json::from_slice(&body)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.into()),
    };
    let settings = &config["central"]["signing"];
    let args = [
        "variable".into(),
        "get".into(),
        "CENTRAL_SIGNING_PROOF".into(),
        "--repo".into(),
        slug.clone(),
    ];
    let saved = github.call(&args, None, cwd).await.unwrap_or(Value::Null);
    let mut proof = saved["value"]
        .as_str()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null);
    let bindings: Vec<_> = original.iter().map(|v| v["updated_at"].clone()).collect();
    if !proof.is_null()
        && (bindings.iter().any(Value::is_null)
            || (proof["secret_updates"] != json!(bindings)
                || proof["secret_names"] != json!(names)))
    {
        proof = Value::Null;
    }
    if proof.is_null()
        && (!authorized
            || !settings.is_object()
            || (!present.is_empty()
                && settings["private_key_file"].is_null()
                && settings["fingerprint"].is_null()))
    {
        return Ok(
            json!({"status":"manual-prerequisite","secrets":present,"reason":"configure central.signing and explicitly authorize reuse of local private material or generation; existing encrypted secrets cannot be decrypted or silently replaced"}),
        );
    }
    let settings = if settings.is_object() {
        settings.clone()
    } else {
        json!({})
    };
    let result = signing(&settings, &slug, &proof, cwd).await;
    let Ok(mut result) = result else {
        return Ok(
            json!({"status":"unverified","reason":"signing or independent keyserver verification failed; private diagnostics withheld; signing configuration remains incomplete"}),
        );
    };
    if proof.is_null() {
        let expiry = result["proof"]["expires_at"]
            .as_str()
            .context("actual key expiry missing")?;
        for (name, field) in names.iter().zip(["private_key", "passphrase"]) {
            github
                .call(
                    &[
                        "secret".into(),
                        "set".into(),
                        name.clone(),
                        "--repo".into(),
                        slug.clone(),
                        "--expires-at".into(),
                        expiry.into(),
                    ],
                    Some(
                        result[field]
                            .as_str()
                            .context("signing value unavailable")?,
                    ),
                    cwd,
                )
                .await?;
        }
        let fresh = metadata(github, &slug, &names, cwd).await?;
        if fresh.iter().any(Value::is_null) {
            return Ok(
                json!({"status":"unverified","reason":"encrypted signing settings read-back failed"}),
            );
        }
        result["proof"]["secret_names"] = json!(names);
        result["proof"]["secret_updates"] = json!(fresh
            .iter()
            .map(|v| v["updated_at"].clone())
            .collect::<Vec<_>>());
        github
            .call(
                &[
                    "variable".into(),
                    "ensure".into(),
                    "CENTRAL_SIGNING_PROOF".into(),
                    "--repo".into(),
                    slug.clone(),
                    "--value".into(),
                    result["proof"].to_string(),
                ],
                None,
                cwd,
            )
            .await?;
    }
    Ok(
        json!({"status":"verified","fingerprint":result["proof"]["fingerprint"],"expires_at":result["proof"]["expires_at"],"reused":!proof.is_null(),"evidence":{"configuration":"detached-signature-verified","public_key":"keyserver-fingerprint-verified","publication":"unverified","public_release_signatures":"unverified","consumers":"unverified"}}),
    )
}
