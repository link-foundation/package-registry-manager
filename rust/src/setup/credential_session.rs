//! gh-manager stdin transport and verified token rotation in the dedicated browser.
use super::{is_yes, prompt, Session};
use crate::automation::Automation;
use crate::browser_options::ImportScope;
use crate::credential_browser::{
    revoked_script, token_form_script, token_provider, TokenProvider, READ_TOKEN,
};
use crate::credential_cycle::{
    needs_rotation, rotate_credential, Credential, CredentialAdapter, CredentialMetadata,
};
use crate::model::{SetupStep, StepKind};
use anyhow::{anyhow, bail, Result};
use chrono::{Duration as Days, Utc};
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

async fn secret_command(
    args: &[String],
    input: Option<&str>,
    cwd: &std::path::Path,
) -> Result<String> {
    let mut child = Command::new("gh-manager")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(if input.is_some() {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            anyhow!("gh-manager with secret get-metadata/ensure support is required (gh-manager#6)")
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(value) = input {
            let _ = stdin.write_all(value.as_bytes()).await;
        }
        drop(stdin);
    }
    let result = child.wait_with_output().await?;
    if !result.status.success() {
        bail!("gh-manager secret operation failed; verify installation, permissions and secret support (gh-manager#6)");
    }
    Ok(String::from_utf8(result.stdout)?)
}

struct TokenHost<'s, 'a> {
    session: &'s mut Session<'a>,
    settings: Value,
    provider: TokenProvider,
    scope: Vec<String>,
    visibility: Vec<String>,
    secret: String,
}

#[allow(clippy::future_not_send)]
impl CredentialAdapter for TokenHost<'_, '_> {
    async fn create(&mut self) -> Result<Credential> {
        self.session
            .automated()
            .await?
            .goto(self.provider.url)
            .await?;
        prompt(&format!(
            "Sign in, then press Enter to fill the narrow publishing scope ({})...",
            self.provider.scope
        ))?;
        let expiry = (Utc::now() + Days::days(self.settings["expiry_days"].as_i64().unwrap_or(30)))
            .to_rfc3339();
        let name = format!(
            "prm-{}",
            self.session
                .plan
                .repository
                .github_repository
                .as_deref()
                .unwrap_or_default()
        );
        let script = token_form_script(&self.provider, &name, &expiry);
        self.session.automated().await?.evaluate(&script).await?;
        prompt("Review the scope and expiry, create the token in this browser, then press Enter (never paste its value)...")?;
        let value = self.session.automated().await?.evaluate(READ_TOKEN).await?;
        Ok(Credential {
            value: value["value"].as_str().unwrap_or_default().into(),
            id: value["id"].as_str().unwrap_or_default().into(),
            expires_at: value["expires_at"].as_str().unwrap_or_default().into(),
        })
    }
    async fn store(&mut self, credential: &Credential) -> Result<()> {
        let mut args = vec!["secret".into(), "ensure".into(), self.secret.clone()];
        args.extend(self.scope.clone());
        args.extend(self.visibility.clone());
        args.extend([
            "--expires-at".into(),
            credential.expires_at.clone(),
            "--token-id".into(),
            credential.id.clone(),
            "--registry".into(),
            self.session.plan.registry.to_string(),
        ]);
        secret_command(
            &args,
            Some(&credential.value),
            self.session.options.repository,
        )
        .await?;
        Ok(())
    }
    async fn verify(&mut self) -> Result<()> {
        self.session
            .verify_credential_workflow(
                self.settings["verification_workflow"]
                    .as_str()
                    .unwrap_or_default(),
            )
            .await
    }
    async fn revoke(&mut self, id: &str) -> Result<()> {
        self.session
            .automated()
            .await?
            .goto(self.provider.url.trim_end_matches("/create"))
            .await?;
        prompt(&format!(
            "Revoke registry token {id} in this browser, then press Enter..."
        ))?;
        Ok(())
    }
    async fn revoked(&mut self, id: &str) -> Result<bool> {
        Ok(self
            .session
            .automated()
            .await?
            .evaluate(&revoked_script(id))
            .await?
            == json!(true))
    }
}

#[allow(clippy::future_not_send)]
impl Session<'_> {
    pub(super) async fn setup_credential(&mut self) -> Result<()> {
        let provider = token_provider(self.plan.registry)
            .ok_or_else(|| anyhow!("no token provider for {}", self.plan.registry))?;
        let config = match std::fs::read_to_string(
            self.options
                .repository
                .join(".package-registry-manager.json"),
        ) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}".into(),
            Err(error) => return Err(error.into()),
        };
        let config: Value = serde_json::from_str(&config)?;
        let settings = config["tokens"][self.plan.registry.to_string()].clone();
        let workflow = settings["verification_workflow"]
            .as_str()
            .unwrap_or_default();
        if !regex::Regex::new(r"^[\w.-]+\.ya?ml$")
            .expect("static pattern")
            .is_match(workflow)
        {
            bail!("configure tokens.<registry>.verification_workflow for a reviewed dry-run login workflow before creating credentials");
        }
        let contents = std::fs::read_to_string(
            self.options
                .repository
                .join(".github/workflows")
                .join(workflow),
        )?;
        if !contents.contains("workflow_dispatch")
            || !contents.contains("inputs.prm_nonce")
            || !contents.contains("run-name:")
        {
            bail!("verification workflow requires workflow_dispatch input prm_nonce and run-name containing inputs.prm_nonce");
        }
        if self.options.no_browser
            || self.options.browser_options.attach.is_some()
            || self.options.browser_options.import_scope != ImportScope::Domains
        {
            bail!("token setup requires a dedicated profile with domain-scoped sign-in import");
        }
        let days = if settings["expiry_days"].is_null() {
            30
        } else {
            settings["expiry_days"]
                .as_i64()
                .ok_or_else(|| anyhow!("token expiry_days must be between 1 and 90"))?
        };
        if !(1..=90).contains(&days) {
            bail!("token expiry_days must be between 1 and 90");
        }
        let owner = self
            .plan
            .repository
            .github_owner
            .as_deref()
            .ok_or_else(|| anyhow!("token setup requires GitHub repository coordinates"))?;
        let repo = self
            .plan
            .repository
            .github_repository
            .as_deref()
            .ok_or_else(|| anyhow!("token setup requires GitHub repository coordinates"))?;
        let slug = format!("{owner}/{repo}");
        let secret = settings["secret"]
            .as_str()
            .or_else(|| self.plan.package.token_secrets.first().map(String::as_str))
            .unwrap_or(provider.secret)
            .to_owned();
        if !regex::Regex::new(r"^[A-Z][A-Z0-9_]*$")
            .expect("static pattern")
            .is_match(&secret)
        {
            bail!("invalid registry secret name");
        }
        let (scope, visibility) = match settings["level"].as_str() {
            Some("repo") => (vec!["--repo".into(), slug.clone()], Vec::new()),
            None | Some("org") => (
                vec!["--org".into(), owner.to_owned()],
                vec![
                    "--visibility".into(),
                    "selected".into(),
                    "--repos".into(),
                    slug.clone(),
                ],
            ),
            _ => bail!("token secret level must be repo or org"),
        };
        let mut args = vec!["secret".into(), "get-metadata".into(), secret.clone()];
        args.extend(scope.clone());
        args.push("--json".into());
        let previous: CredentialMetadata =
            serde_json::from_str(&secret_command(&args, None, self.options.repository).await?)?;
        if !needs_rotation(&previous, Utc::now())
            && self.verify_credential_workflow(workflow).await.is_ok()
        {
            return Ok(());
        }
        if !self.options.yes && !is_yes(&prompt(&format!("Create and store scoped {secret}, verify it, then revoke the replaced token? [y/N] "))?) { bail!("credential creation declined"); }
        // Raw browser protocol tracing is disabled while values are read.
        if let Some(browser) = self.browser.take() {
            browser.close().await;
        }
        let browser_options = self.launch_browser().await?;
        self.browser = Some(
            Automation::connect(
                &browser_options,
                self.options.browser_profile,
                false,
                &self.domains,
            )
            .await?,
        );
        let mut host = TokenHost {
            session: self,
            settings,
            provider,
            scope,
            visibility,
            secret: secret.clone(),
        };
        let token = rotate_credential(&previous, &mut host).await?;
        println!(
            "  {secret} verified; expiry {}; replaced token revocation verified.",
            token.expires_at.unwrap_or_default()
        );
        Ok(())
    }

    async fn credential_capture(&self, args: &[&str]) -> Result<String> {
        let step = SetupStep::new(
            "verify-registry-login",
            "Verify registry login",
            StepKind::Check,
            "Dispatch and correlate a dry-run login workflow.",
        )
        .command("gh", args)
        .cwd(".");
        let result = self.capture(&step).await?;
        if result.code != 0 {
            bail!("credential verification command failed; old token remains active");
        }
        Ok(result.stdout)
    }

    async fn verify_credential_workflow(&self, workflow: &str) -> Result<()> {
        let slug = format!(
            "{}/{}",
            self.plan
                .repository
                .github_owner
                .as_deref()
                .unwrap_or_default(),
            self.plan
                .repository
                .github_repository
                .as_deref()
                .unwrap_or_default()
        );
        let repo: Value = serde_json::from_str(
            &self
                .credential_capture(&["repo", "view", &slug, "--json", "defaultBranchRef"])
                .await?,
        )?;
        let branch = repo["defaultBranchRef"]["name"]
            .as_str()
            .ok_or_else(|| anyhow!("cannot determine verification workflow branch"))?;
        let nonce = format!(
            "prm-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        self.credential_capture(&[
            "workflow",
            "run",
            workflow,
            "--repo",
            &slug,
            "--ref",
            branch,
            "-f",
            &format!("prm_nonce={nonce}"),
        ])
        .await?;
        let deadline = Instant::now() + self.options.wait_timeout;
        let mut id = None;
        while Instant::now() < deadline {
            if id.is_none() {
                let runs: Value = serde_json::from_str(
                    &self
                        .credential_capture(&[
                            "run",
                            "list",
                            "--repo",
                            &slug,
                            "--workflow",
                            workflow,
                            "--event",
                            "workflow_dispatch",
                            "--limit",
                            "50",
                            "--json",
                            "databaseId,displayTitle",
                        ])
                        .await?,
                )?;
                let matching: Vec<_> = runs
                    .as_array()
                    .ok_or_else(|| anyhow!("invalid workflow run list"))?
                    .iter()
                    .filter(|run| {
                        run["displayTitle"]
                            .as_str()
                            .is_some_and(|title| title.contains(&nonce))
                    })
                    .collect();
                if matching.len() > 1 {
                    bail!("ambiguous credential verification runs; old token remains active");
                }
                id = matching.first().and_then(|run| run["databaseId"].as_u64());
            }
            if let Some(id) = id {
                let run: Value = serde_json::from_str(
                    &self
                        .credential_capture(&[
                            "run",
                            "view",
                            &id.to_string(),
                            "--repo",
                            &slug,
                            "--json",
                            "status,conclusion",
                        ])
                        .await?,
                )?;
                if run["status"] == "completed" {
                    if run["conclusion"] != "success" {
                        bail!(
                            "registry rejected credential verification; old token remains active"
                        );
                    }
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        bail!("credential verification timed out; old token remains active")
    }
}
