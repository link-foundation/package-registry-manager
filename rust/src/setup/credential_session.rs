//! Registry browser acquisition with gh-manager CI evidence and org-first secrets.
use super::{is_yes, prompt, Session};
use crate::{
    browser_options::{ImportScope, ImportSource},
    ci_credential_cycle::{
        complete_rotation, cycle_credential_with, record_token_id, secret_name, tracked_token_ids,
        CiCredentialAdapter,
    },
    credential_browser::{
        revoked_script, token_form_script, token_provider, TokenProvider, READ_TOKEN,
    },
    credential_cycle::Credential,
    github::{failure_patterns, GhManager, GithubGateway},
};
use anyhow::{anyhow, bail, Result};
use chrono::{Duration as Days, Utc};
use serde_json::{json, Value};
use std::path::PathBuf;

struct TokenHost<'s, 'a> {
    session: &'s mut Session<'a>,
    settings: Value,
    provider: TokenProvider,
    secret: String,
    slug: String,
    owner: String,
    state: Value,
    state_file: PathBuf,
    approved: bool,
    repository_secret_present: bool,
    test_targets: Vec<String>,
    crates_previous: Value,
    visibility: Option<String>,
}

impl TokenHost<'_, '_> {
    fn rotation(&self) -> bool {
        self.session.plan.registry == crate::Registry::CratesIo
    }
    fn key(&self) -> String {
        format!("{}:{}", self.slug, self.secret)
    }
    fn targets(&self) -> Vec<String> {
        if !self.crates_previous.is_null() {
            return self.test_targets.clone();
        }
        if self.settings["level"] == "repo" {
            return vec![self.slug.clone()];
        }
        self.session
            .options
            .secret_repositories
            .and_then(|groups| {
                groups.get(&format!("{}:{}", self.session.plan.registry, self.secret))
            })
            .cloned()
            .unwrap_or_else(|| vec![self.slug.clone()])
    }
    fn health_args(&self, operation: &str) -> Vec<String> {
        let mut args = vec![
            "secret".into(),
            operation.into(),
            self.secret.clone(),
            "--repo".into(),
            self.slug.clone(),
        ];
        for pattern in failure_patterns(Some(self.session.plan.registry)) {
            args.extend(["--failure-pattern".into(), pattern]);
        }
        if operation == "test" && self.settings["verification_workflow"].is_string() {
            args.extend([
                "--input".into(),
                format!("prm_nonce=prm-{}", Utc::now().timestamp_millis()),
            ]);
        }
        args
    }
    fn write_state(&self) -> Result<()> {
        std::fs::create_dir_all(self.state_file.parent().expect("state directory"))?;
        std::fs::write(&self.state_file, serde_json::to_vec(&self.state)?)?;
        Ok(())
    }
}

#[allow(clippy::future_not_send)]
impl CiCredentialAdapter for TokenHost<'_, '_> {
    async fn health(&mut self) -> Result<Value> {
        if self.rotation() {
            return Ok(json!({"status":"ok"}));
        }
        GhManager
            .call(
                &self.health_args("health"),
                None,
                self.session.options.repository,
            )
            .await
    }
    async fn metadata(&mut self) -> Result<Value> {
        let mut metadata = GhManager
            .call(
                &[
                    "secret".into(),
                    "get-metadata".into(),
                    self.secret.clone(),
                    "--repo".into(),
                    self.slug.clone(),
                ],
                None,
                self.session.options.repository,
            )
            .await?;
        self.repository_secret_present = !metadata.is_null();
        if (metadata.is_null() || self.rotation()) && self.settings["level"] != "repo" {
            // GitHub health already authenticated; inaccessible organization metadata
            // is resolved by gh-manager's org-first ensure/fallback, before storage.
            let org_metadata = GhManager
                .call(
                    &[
                        "secret".into(),
                        "get-metadata".into(),
                        self.secret.clone(),
                        "--org".into(),
                        self.owner.clone(),
                    ],
                    None,
                    self.session.options.repository,
                )
                .await
                .unwrap_or(Value::Null);
            if !org_metadata.is_null() {
                metadata = org_metadata;
            }
        }
        if self.rotation() {
            if metadata.is_null() {
                bail!("existing registry token secret was not found");
            }
            self.visibility = metadata["visibility"].as_str().map(str::to_owned);
            if self
                .visibility
                .as_deref()
                .is_some_and(|visibility| matches!(visibility, "all" | "private"))
                && !self.session.options.repository_batch
            {
                bail!("rotation of an all/private organization secret requires --org to discover every consumer");
            }
            let mut targets = self.targets();
            for repo in metadata["selected_repositories"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(repo) = repo["full_name"].as_str() {
                    if !targets.iter().any(|target| target == repo) {
                        targets.push(repo.into());
                    }
                }
            }
            self.test_targets = targets;
            let step = crate::SetupStep::new(
                "crates-sign-in",
                "Sign in to crates.io",
                crate::StepKind::Api,
                "Read private token metadata",
            );
            self.session.api_step(&step).await?;
            let answer = self
                .session
                .automated()
                .await?
                .evaluate(&crate::crates_api::page_fetch_script(
                    "GET",
                    crate::crates_tokens::TOKEN_INVENTORY_PATH,
                    None,
                ))
                .await?;
            let tokens = crate::crates_tokens::token_inventory(&answer)?;
            self.crates_previous = crate::crates_tokens::select_crates_token(
                &tokens,
                &self.secret,
                &metadata,
                self.settings.get("token_id"),
            )?;
            if self.session.options.verbose {
                eprintln!(
                    "Rotate {}: token {}; verify {}",
                    self.secret,
                    self.crates_previous["id"],
                    self.test_targets.join(", ")
                );
            }
            metadata["token_id"] = json!(self.crates_previous["id"]
                .as_str()
                .map_or_else(|| self.crates_previous["id"].to_string(), str::to_owned));
        }
        let targets = self.targets();
        let ids = tracked_token_ids(&self.state, &targets, &self.secret);
        if !metadata.is_null() || !ids.is_empty() {
            let present = !metadata.is_null();
            if !present {
                metadata = json!({});
            }
            let id = if self.rotation() {
                metadata["token_id"].clone()
            } else {
                self.settings.get("token_id").cloned().unwrap_or_else(|| {
                    self.state[self.key()]
                        .as_array()
                        .and_then(|items| items.last())
                        .cloned()
                        .unwrap_or_else(|| metadata["token_id"].clone())
                })
            };
            let retained = self.state.as_object().is_some_and(|entries| {
                entries.iter().any(|(key, values)| {
                    !targets
                        .iter()
                        .any(|repo| key == &format!("{repo}:{}", self.secret))
                        && values.as_array().is_some_and(|items| items.contains(&id))
                })
            });
            metadata["present"] = json!(present);
            metadata["token_id"] = if retained { Value::Null } else { id };
            metadata["token_ids"] = json!(ids);
        }
        Ok(metadata)
    }
    async fn create(&mut self) -> Result<Credential> {
        if self.session.options.no_browser
            || self.session.options.browser_options.attach.is_some()
            || self.session.options.browser_options.import_scope != ImportScope::Domains
        {
            bail!("token setup requires a dedicated profile with domain-scoped sign-in import");
        }
        if !self.approved && !self.session.options.yes && !is_yes(&prompt("Create and store a scoped credential, verify it, then revoke the replaced token? [y/N] ")?) { bail!("credential creation declined"); }
        self.approved = true;
        let days = if self.settings["expiry_days"].is_null() {
            30
        } else {
            self.settings["expiry_days"]
                .as_i64()
                .ok_or_else(|| anyhow!("token expiry_days must be between 1 and 90"))?
        };
        if !(1..=90).contains(&days) {
            bail!("token expiry_days must be between 1 and 90");
        }
        if self.rotation() {
            let body = crate::crates_tokens::replacement_token_request(
                &self.crates_previous,
                days,
                Utc::now(),
            )?;
            let answer = self
                .session
                .automated()
                .await?
                .evaluate(&crate::crates_api::page_fetch_script(
                    "PUT",
                    "/api/v1/me/tokens",
                    Some(&body),
                ))
                .await?;
            let token = &answer["body"]["api_token"];
            if answer["status"] != 200 || !token["token"].is_string() || token["id"].is_null() {
                bail!("crates.io token creation failed");
            }
            return Ok(Credential {
                id: token["id"]
                    .as_str()
                    .map_or_else(|| token["id"].to_string(), str::to_owned),
                value: token["token"].as_str().unwrap_or_default().into(),
                expires_at: token["expired_at"]
                    .as_str()
                    .or_else(|| body["api_token"]["expired_at"].as_str())
                    .unwrap_or_default()
                    .into(),
            });
        }
        if self.session.browser.is_none() {
            let mut browser = self.session.options.browser_options.clone();
            if browser.import.is_none() {
                browser.import = Some(ImportSource {
                    browser: "auto".into(),
                    profile: None,
                });
            }
            let original = self.session.launch_browser_with(&browser).await?;
            self.session.browser = Some(
                crate::automation::Automation::connect(
                    &original,
                    self.session.options.browser_profile,
                    false,
                    &self.session.domains,
                )
                .await?,
            );
        }
        self.session
            .automated()
            .await?
            .goto(self.provider.url)
            .await?;
        let name = format!(
            "prm-{}-{}",
            self.session
                .plan
                .repository
                .github_repository
                .as_deref()
                .unwrap_or_default(),
            self.secret
        );
        let script = token_form_script(
            &self.provider,
            &name,
            &(Utc::now() + Days::days(days)).to_rfc3339(),
        );
        self.session.automated().await?.evaluate(&script).await?;
        prompt(&format!("Review the publishing scope ({}) and expiry, create the token in this browser, then press Enter (never paste its value)...",self.provider.scope))?;
        let value = self.session.automated().await?.evaluate(READ_TOKEN).await?;
        Ok(Credential {
            value: value["value"].as_str().unwrap_or_default().into(),
            id: value["id"].as_str().unwrap_or_default().into(),
            expires_at: value["expires_at"].as_str().unwrap_or_default().into(),
        })
    }
    async fn ensure(&mut self, credential: &Credential) -> Result<Value> {
        let mut args = vec![
            "secret".into(),
            "ensure".into(),
            self.secret.clone(),
            "--broken".into(),
            "--rotate-before".into(),
            "0".into(),
        ];
        if self.settings["level"] == "repo" {
            args.extend(["--repo".into(), self.slug.clone()]);
        } else {
            args.extend([
                "--org".into(),
                self.owner.clone(),
                "--visibility".into(),
                "selected".into(),
                "--repos".into(),
                self.slug.clone(),
            ]);
        }
        if !credential.expires_at.is_empty() {
            args.extend(["--expires-at".into(), credential.expires_at.clone()]);
        }
        for pattern in failure_patterns(Some(self.session.plan.registry)) {
            args.extend(["--failure-pattern".into(), pattern]);
        }
        let targets = self.targets();
        if self.settings["level"] != "repo" {
            let index = args
                .iter()
                .position(|arg| arg == "--repos")
                .expect("organization targets")
                + 1;
            args[index] = targets.join(",");
        }
        let broad = self.rotation()
            && self.settings["level"] != "repo"
            && self
                .visibility
                .as_deref()
                .is_some_and(|visibility| matches!(visibility, "all" | "private"));
        if broad {
            args = vec![
                "secret".into(),
                "set".into(),
                self.secret.clone(),
                "--org".into(),
                self.owner.clone(),
                "--visibility".into(),
                self.visibility.clone().expect("existing visibility"),
            ];
            if !credential.expires_at.is_empty() {
                args.extend(["--expires-at".into(), credential.expires_at.clone()]);
            }
        }
        let mut result = GhManager
            .call(
                &args,
                Some(&credential.value),
                self.session.options.repository,
            )
            .await?;
        if broad {
            result["path"] = json!("organization");
        }
        if result["path"] == "organization" {
            for repo in &targets {
                let present = if repo == &self.slug {
                    self.repository_secret_present
                } else {
                    !GhManager
                        .call(
                            &[
                                "secret".into(),
                                "get-metadata".into(),
                                self.secret.clone(),
                                "--repo".into(),
                                repo.clone(),
                            ],
                            None,
                            self.session.options.repository,
                        )
                        .await?
                        .is_null()
                };
                if !present {
                    continue;
                }
                let mut repo_args = vec![
                    "secret".into(),
                    "ensure".into(),
                    self.secret.clone(),
                    "--repo".into(),
                    repo.clone(),
                    "--broken".into(),
                    "--rotate-before".into(),
                    "0".into(),
                ];
                if !credential.expires_at.is_empty() {
                    repo_args.extend(["--expires-at".into(), credential.expires_at.clone()]);
                }
                let replacement = GhManager
                    .call(
                        &repo_args,
                        Some(&credential.value),
                        self.session.options.repository,
                    )
                    .await?;
                if replacement["valueChanged"] == false {
                    bail!("gh-manager did not replace the repository credential override");
                }
                if repo == &self.slug {
                    result = replacement;
                    result["path"] = json!("repository");
                    result["fallbackReason"] = json!("An existing repository secret overrides the organization secret; replaced the repository credential too.");
                }
            }
        }
        self.test_targets = if self.settings["level"] == "repo" {
            vec![self.slug.clone()]
        } else {
            targets
        };
        if result["valueChanged"] == false {
            bail!("gh-manager did not store the replacement credential");
        }
        record_token_id(
            &mut self.state,
            &self.test_targets,
            &self.secret,
            &credential.id,
        );
        self.write_state()?;
        Ok(result)
    }
    async fn test(&mut self) -> Result<Value> {
        let mut status = "ok";
        for repo in &self.test_targets {
            let mut args = self.health_args("test");
            args[4].clone_from(repo);
            let result = GhManager
                .call(&args, None, self.session.options.repository)
                .await?;
            if result["status"] == "auth-failing" {
                status = if self.rotation() {
                    "unknown"
                } else {
                    "auth-failing"
                };
            } else if result["status"] != "ok" && status != "auth-failing" {
                status = "unknown";
            }
        }
        Ok(json!({"status":status}))
    }
    async fn revoke(&mut self, id: &str) -> Result<()> {
        if self.rotation() {
            let path = format!(
                "/api/v1/me/tokens/{}",
                crate::registry_state::encode_uri_component(id)
            );
            let answer = self
                .session
                .automated()
                .await?
                .evaluate(&crate::crates_api::page_fetch_script("DELETE", &path, None))
                .await?;
            if !matches!(answer["status"].as_u64(), Some(200 | 204)) {
                bail!("crates.io token revocation failed");
            }
            return Ok(());
        }
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
        if self.rotation() {
            let answer = self
                .session
                .automated()
                .await?
                .evaluate(&crate::crates_api::page_fetch_script(
                    "GET",
                    crate::crates_tokens::TOKEN_INVENTORY_PATH,
                    None,
                ))
                .await?;
            return Ok(!crate::crates_tokens::token_inventory(&answer)?
                .iter()
                .any(|token| {
                    token["id"]
                        .as_str()
                        .map_or_else(|| token["id"].to_string(), str::to_owned)
                        == id
                }));
        }
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
        let contents = match std::fs::read_to_string(
            self.options
                .repository
                .join(".package-registry-manager.json"),
        ) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "{}".into(),
            Err(error) => return Err(error.into()),
        };
        let config: Value = serde_json::from_str(&contents)?;
        let settings = config["tokens"][self.plan.registry.to_string()].clone();
        if settings["level"]
            .as_str()
            .is_some_and(|level| !["repo", "org"].contains(&level))
        {
            bail!("token secret level must be repo or org");
        }
        let owner = self
            .plan
            .repository
            .github_owner
            .clone()
            .ok_or_else(|| anyhow!("token setup requires GitHub repository coordinates"))?;
        let repo = self
            .plan
            .repository
            .github_repository
            .as_ref()
            .ok_or_else(|| anyhow!("token setup requires GitHub repository coordinates"))?;
        let slug = format!("{owner}/{repo}");
        let template = self
            .options
            .secret_name
            .or_else(|| settings["secret"].as_str())
            .or_else(|| self.plan.package.token_secrets.first().map(String::as_str))
            .unwrap_or(provider.secret);
        let secret = secret_name(template, self.plan.registry, &slug)?;
        let state_file = self
            .options
            .browser_profile
            .parent()
            .ok_or_else(|| anyhow!("browser profile has no state directory"))?
            .join("credential-ids.json");
        let state = match std::fs::read_to_string(&state_file) {
            Ok(value) => serde_json::from_str(&value)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(error) => return Err(error.into()),
        };
        let mut host = TokenHost {
            crates_previous: Value::Null,
            visibility: None,
            test_targets: vec![slug.clone()],
            session: self,
            settings,
            provider,
            secret: secret.clone(),
            slug,
            owner,
            state,
            state_file,
            approved: false,
            repository_secret_present: false,
        };
        let rotation = host.rotation();
        if rotation {
            if host.session.options.no_browser
                || host.session.options.browser_options.attach.is_some()
                || host.session.options.browser_options.import_scope != ImportScope::Domains
            {
                bail!(
                    "token rotation requires a dedicated profile with domain-scoped sign-in import"
                );
            }
            if !host.session.options.yes && !is_yes(&prompt("Replace the legacy token, verify consumers in CI, then revoke the old token? [y/N] ")?) { bail!("credential creation declined"); }
            host.approved = true;
        }
        let result = cycle_credential_with(&mut host, rotation).await?;
        if rotation {
            println!(
                "Migrate these repositories to crates.io trusted publishing before deleting {}: {}",
                host.secret,
                host.test_targets.join(", ")
            );
        }
        if result["changed"] == true {
            complete_rotation(
                &mut host.state,
                &host.test_targets,
                &host.secret,
                result["token_id"].as_str().expect("verified token ID"),
            );
            host.write_state()?;
        }
        if let Some(scope) = result["path"].as_str() {
            host.session
                .values
                .insert("secret_scope".into(), scope.into());
        }
        if let Some(reason) = result["fallbackReason"].as_str() {
            host.session
                .values
                .insert("fallback_reason".into(), reason.into());
        }
        println!(
            "  Credential CI status: {}",
            result["status"].as_str().unwrap_or("unknown")
        );
        Ok(())
    }
}
