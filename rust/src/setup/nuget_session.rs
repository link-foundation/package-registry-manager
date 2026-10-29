//! `NuGet` Gallery policy setup with an exact authenticated account and fresh readback.
use super::{prompt, Session};
use crate::browser_options::{ImportScope, ImportSource};
use crate::github::{GhManager, GithubGateway};
use crate::sign_in_import::{find_saved_login_sources, find_sign_in_sources, InstalledBrowsers};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const URL: &str = "https://www.nuget.org/account/TrustedPublishing";
fn script(expected: &Value, create: bool) -> String {
    let source = include_str!("../nuget-page.mjs")
        .split_once("export default ")
        .expect("page function")
        .1;
    format!("({source})({expected},{create})")
}

#[allow(clippy::future_not_send)]
impl Session<'_> {
    async fn inspect_nuget(&mut self, expected: &Value) -> Result<Value> {
        let deadline = tokio::time::Instant::now() + self.options.wait_timeout;
        let interval = self.options.poll_interval;
        let page = self.visible_automated().await?;
        page.goto(URL).await?;
        loop {
            let snapshot = page.evaluate(&script(expected, false)).await?;
            if snapshot["ready"] == true
                || snapshot["username"].is_null()
                || !snapshot["error"].is_null()
            {
                return Ok(snapshot);
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("NuGet policy page did not become ready");
            }
            tokio::time::sleep(interval).await;
        }
    }

    async fn reuse_nuget_sign_in(&mut self, expected: &Value) -> Result<Value> {
        let browser = self.options.browser_options;
        if browser.attach.is_some()
            || browser.import.as_ref().is_some_and(|s| {
                !crate::sign_in_import::IMPORT_CHOICES.contains(&s.browser.as_str())
            })
        {
            return self.inspect_nuget(expected).await;
        }
        let domains = self.domains.clone();
        let cookies =
            find_sign_in_sources(&InstalledBrowsers, &domains, None, self.options.verbose);
        let mut snapshot = Value::Null;
        for saved in [false, true] {
            let sources = if saved {
                find_saved_login_sources(&InstalledBrowsers, &domains, None, self.options.verbose)
            } else {
                cookies.clone()
            };
            if sources.is_empty() {
                continue;
            }
            if browser.import.is_none()
                && !super::is_yes(&prompt(&format!(
                    "Try domain-scoped {} from discovered local profiles for {}? [y/N] ",
                    if saved { "saved logins" } else { "sessions" },
                    domains.join(" / ")
                ))?)
            {
                continue;
            }
            for source in sources {
                // browser-commander #189: never replace an existing Login Data file.
                self.imported_profiles
                    .push(tempfile::Builder::new().prefix("prm-signin-").tempdir()?);
                let mut imported = browser.clone();
                imported.import = Some(ImportSource {
                    browser: source.browser,
                    profile: source.profile,
                });
                imported.import_scope = if saved {
                    ImportScope::SavedLogins
                } else {
                    ImportScope::Domains
                };
                if self.connect(&imported).await.is_err() {
                    continue;
                }
                let Ok(candidate) = self.inspect_nuget(expected).await else {
                    continue;
                };
                snapshot = candidate;
                if snapshot["username"].as_str().is_some_and(|v| {
                    v.eq_ignore_ascii_case(expected["username"].as_str().unwrap_or_default())
                }) {
                    return Ok(snapshot);
                }
            }
        }
        Ok(snapshot)
    }

    pub(super) async fn configure_nuget(&mut self) -> Result<()> {
        let config: Value = match std::fs::read_to_string(
            self.options
                .repository
                .join(".package-registry-manager.json"),
        ) {
            Ok(value) => serde_json::from_str(&value)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(error) => return Err(error.into()),
        };
        let publisher = self
            .plan
            .oidc_publisher
            .as_ref()
            .context("NuGet requires an explicit GitHub repository and workflow")?;
        let slug = format!("{}/{}", publisher.organization, publisher.repository);
        let output = tokio::process::Command::new("gh")
            .args(["api", &format!("repos/{slug}")])
            .current_dir(self.options.repository)
            .output()
            .await?;
        if !output.status.success() {
            bail!("cannot verify immutable GitHub repository IDs");
        }
        let repository: Value = serde_json::from_slice(&output.stdout)?;
        let mut expected = config["nuget"].clone();
        if !expected.is_object() {
            expected = json!({});
        }
        expected["organization"] = json!(publisher.organization);
        expected["repository"] = json!(publisher.repository);
        expected["workflow"] = json!(publisher.workflow);
        expected["environment"] = json!(publisher.environment);
        expected["owner_id"] = json!(repository["owner"]["id"]
            .as_u64()
            .context("missing owner ID")?
            .to_string());
        expected["repository_id"] = json!(repository["id"]
            .as_u64()
            .context("missing repository ID")?
            .to_string());
        if expected["package_ids"].is_null() {
            expected["package_ids"] = json!([self.plan.package.name]);
        }
        crate::nuget_policy::validate(&expected)?;
        if !expected["package_ids"]
            .as_array()
            .context("package IDs")?
            .iter()
            .any(|id| {
                id.as_str()
                    .is_some_and(|id| id.eq_ignore_ascii_case(&self.plan.package.name))
            })
        {
            bail!("selected package is outside configured NuGet policy IDs");
        }
        self.domains = crate::sign_in_import::nuget_sign_in_domains(
            config["nuget"]["sign_in_route"].as_str().unwrap_or("work"),
        )
        .to_vec();
        let mut snapshot = self.inspect_nuget(&expected).await?;
        let account = expected["username"].as_str().context("NuGet profile")?;
        if !snapshot["username"]
            .as_str()
            .is_some_and(|v| v.eq_ignore_ascii_case(account))
        {
            snapshot = self.reuse_nuget_sign_in(&expected).await?;
        }
        let deadline = tokio::time::Instant::now() + self.options.wait_timeout;
        while snapshot["ready"] != true
            || !snapshot["username"]
                .as_str()
                .is_some_and(|v| v.eq_ignore_ascii_case(account))
        {
            if snapshot["username"].as_str().is_some() {
                bail!("signed-in NuGet profile differs from the configured identity; policy was not changed");
            }
            if self.options.yes {
                bail!("NuGet sign-in requires interaction; local session reuse did not verify the configured identity");
            }
            prompt("Sign in with the configured NuGet profile; complete password, 2FA or platform passkey authentication, then press Enter...")?;
            if tokio::time::Instant::now() >= deadline {
                bail!("NuGet authentication remains unverified");
            }
            snapshot = self.inspect_nuget(&expected).await?;
        }
        let policies = snapshot["policies"]
            .as_array()
            .context("NuGet policies unavailable")?;
        let mut policy_changed = false;
        let mut state = crate::nuget_policy::state(policies, &expected, account)?;
        if state["status"] == "missing-or-mismatched" {
            let submitted = self
                .automated()
                .await?
                .evaluate(&script(&expected, true))
                .await?;
            if submitted["submitted"] != true || !submitted["error"].is_null() {
                bail!("NuGet policy was not created: {}", submitted["error"]);
            }
            loop {
                tokio::time::sleep(self.options.poll_interval).await;
                let pending = self
                    .automated()
                    .await?
                    .evaluate(&script(&expected, false))
                    .await?;
                if !pending["error"].is_null() {
                    bail!("NuGet policy creation failed: {}", pending["error"]);
                }
                if pending["pending"] != true {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    bail!("NuGet policy creation remains pending");
                }
            }
            snapshot = self.inspect_nuget(&expected).await?;
            policy_changed = true;
            state = crate::nuget_policy::state(
                snapshot["policies"]
                    .as_array()
                    .context("fresh policy readback unavailable")?,
                &expected,
                snapshot["username"].as_str().unwrap_or_default(),
            )?;
        }
        println!("{}", json!({"nuget_policy":state}));
        if state["verified"] != true {
            bail!(
                "NuGet policy is {}; publication remains unverified",
                state["status"]
            );
        }
        let mut args = vec![
            "variable".into(),
            "ensure".into(),
            "NUGET_USER".into(),
            "--value".into(),
            account.into(),
            "--repo".into(),
            slug,
        ];
        if let Some(environment) = publisher.environment.as_deref() {
            args.extend(["--env".into(), environment.into()]);
        }
        let variable = GhManager.call(&args, None, self.options.repository).await?;
        if variable["verified"] != true
            || variable["effective"].as_array().is_none_or(|values| {
                values.is_empty() || values.iter().any(|v| v["value"] != account)
            })
        {
            bail!("effective NUGET_USER readback did not verify the profile");
        }
        if policy_changed || variable["changed"] == true {
            self.values.insert("recovery_changed".into(), "true".into());
        }
        self.conditions.remove("trust-missing");
        self.values.insert(
            "nuget_evidence".into(),
            json!({"policy":state,"variable":variable}).to_string(),
        );
        println!("NuGet policy and effective NUGET_USER verified; authentication exchange, actual push and public consumer validation remain separate CI gates. Existing fallback secrets are retained.");
        Ok(())
    }
}
