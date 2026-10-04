//! The crates.io API steps of a setup session (#26) and the automated
//! browser they drive, with the scoped sign-in import.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::{is_yes, prompt, Session};
use crate::automation::Automation;
use crate::browser_options::{BrowserOptions, ImportSource};
use crate::crates_api::{
    attach_with_session, crate_version, first_publish, first_publish_question, read_session,
    sign_in, CratesHost, CRATES_IO_URL, SIGN_IN_TIMEOUT,
};
use crate::default_browser::detect_default_browser;
use crate::model::{Registry, SetupStep};
use crate::sign_in_import::{
    choose_sign_in_source, default_sources, default_unreadable, find_sign_in_sources,
    sign_in_domains, source_id, InstalledBrowsers, IMPORT_CHOICES,
};
use crate::tokens::TokenState;

/// The setup session as the crates.io API flow sees it, for one step.
struct PublishHost<'s, 'a> {
    session: &'s mut Session<'a>,
    step: &'s SetupStep,
}

#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
impl CratesHost for PublishHost<'_, '_> {
    fn verbose(&self) -> bool {
        self.session.options.verbose
    }

    async fn evaluate(&mut self, script: &str) -> Result<Value> {
        self.session.automated().await?.evaluate(script).await
    }

    async fn goto(&mut self, url: &str) -> Result<()> {
        self.session.automated().await?.goto(url).await
    }

    async fn publish(&mut self, env: &BTreeMap<String, String>) -> Result<i32> {
        let step = self.step;
        let result = self.session.run_process_with(step, true, env).await?;
        Ok(result.code)
    }

    async fn wait_for_registry(&mut self) -> Result<()> {
        self.session.wait(self.step).await
    }

    async fn token_call(
        &mut self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let Some(base) = self.session.options.endpoints.base(Registry::CratesIo) else {
            return (
                0,
                json!({ "errors": [{ "detail": "no crates.io API URL" }] }),
            );
        };
        let url = format!("{base}{path}");
        self.session
            .client
            .token_call(method, &url, token, body)
            .await
    }

    async fn token_state(&mut self, token: &str) -> Option<TokenState> {
        let base = self.session.options.endpoints.base(Registry::CratesIo)?;
        self.session.client.crates_token_state(&base, token).await
    }
}

#[allow(clippy::future_not_send)] // Browser Commander's native CDP adapter is intentionally !Sync.
impl Session<'_> {
    /// Runs a crates.io API step: `crates-sign-in`, `first-publish`,
    /// `attach-trusted-publisher`, or the cleanup step `crates-sign-out`.
    pub(super) async fn api_step(&mut self, step: &SetupStep) -> Result<()> {
        if self.options.no_browser {
            bail!(
                "the crates.io API steps drive the automated browser; drop --no-browser, or pass --manual for the manual checklist"
            );
        }
        let plan = self.plan;
        let crate_name = plan.package.name.as_str();
        let domains = sign_in_domains(plan.registry);
        match step.id.as_str() {
            "crates-sign-in" => {
                let mut host = PublishHost {
                    session: self,
                    step,
                };
                host.goto(CRATES_IO_URL).await?;
                if !read_session(&mut host).await?.signed_in {
                    // Whether imported or signed in by hand, the sign-in is this run's.
                    self.conditions.insert("crates-signed-in");
                    self.offer_sign_in_import(domains).await?;
                }
                let interval = self.options.poll_interval;
                let mut host = PublishHost {
                    session: self,
                    step,
                };
                sign_in(&mut host, interval, SIGN_IN_TIMEOUT).await?;
            }
            "first-publish" => {
                let version = crate_version(&self.cwd(step));
                let publisher = plan.trusted_publisher.as_ref();
                if step.confirm
                    && !self.options.yes
                    && !is_yes(&prompt(&first_publish_question(
                        crate_name,
                        version.as_deref(),
                        publisher,
                    ))?)
                {
                    bail!("the first publish was declined; no token was created and nothing was published");
                }
                let mut host = PublishHost {
                    session: self,
                    step,
                };
                if first_publish(&mut host, crate_name, publisher, chrono::Utc::now()).await? {
                    self.conditions.remove("trust-missing");
                }
            }
            "attach-trusted-publisher" => {
                let Some(publisher) = &plan.trusted_publisher else {
                    println!(
                        "  No GitHub repository or release workflow is known, so the trusted publisher is configured in the form."
                    );
                    self.conditions.insert("trust-api-failed");
                    return Ok(());
                };
                let mut host = PublishHost {
                    session: self,
                    step,
                };
                if attach_with_session(&mut host, crate_name, publisher).await? {
                    self.conditions.remove("trust-missing");
                } else {
                    self.conditions.insert("trust-api-failed");
                }
            }
            "crates-sign-out" => {
                let names = domains.join(" / ");
                if self.options.keep_session {
                    println!(
                        "Keeping the {names} sign-in in the automated profile (--keep-session); the next run reuses it. No token is kept."
                    );
                } else if self.automated().await?.clear_cookies(domains).await? {
                    println!("  Removed the {names} sign-in cookies from the automated profile.");
                }
            }
            other => bail!("no crates.io API step {other}"),
        }
        Ok(())
    }

    /// The automated browser, launched once. `--browser-import default|auto`
    /// is resolved to an installed browser first, except for crates.io, whose
    /// sign-in step offers the import once it knows a sign-in is missing.
    pub(super) async fn automated(&mut self) -> Result<&mut Automation> {
        if self.browser.is_none() {
            let browser = self.launch_browser().await?;
            self.connect(&browser).await?;
        }
        self.browser
            .as_mut()
            .ok_or_else(|| anyhow!("the automated browser is not connected"))
    }

    async fn connect(&mut self, browser: &BrowserOptions) -> Result<()> {
        if let Some(previous) = self.browser.take() {
            previous.close().await;
        }
        self.browser = Some(
            Automation::connect(
                browser,
                self.options.browser_profile,
                self.options.verbose,
                sign_in_domains(self.plan.registry),
            )
            .await?,
        );
        Ok(())
    }

    async fn launch_browser(&self) -> Result<BrowserOptions> {
        let browser = self.options.browser_options;
        let Some(choice) = browser
            .import
            .as_ref()
            .map(|source| source.browser.as_str())
            .filter(|choice| IMPORT_CHOICES.contains(choice))
        else {
            return Ok(browser.clone());
        };
        let mut resolved = browser.clone();
        resolved.import = None;
        if self.plan.registry == Registry::CratesIo {
            return Ok(resolved);
        }
        let domains = sign_in_domains(self.plan.registry);
        let preferred = self.default_source().await;
        resolved.import = if choice == "default" {
            Some(ImportSource {
                browser: preferred
                    .ok_or_else(|| anyhow!(default_unreadable()))?
                    .to_owned(),
                profile: None,
            })
        } else {
            find_sign_in_sources(&InstalledBrowsers, domains, preferred, self.options.verbose)
                .into_iter()
                .next()
                .map(|source| ImportSource {
                    browser: source.browser,
                    profile: source.profile,
                })
        };
        if resolved.import.is_none() {
            let names = if domains.is_empty() {
                "this registry".to_owned()
            } else {
                domains.join(", ")
            };
            eprintln!(
                "warning: no installed browser holds a sign-in for {names}; the automated profile starts without one"
            );
        }
        Ok(resolved)
    }

    async fn default_source(&self) -> Option<&'static str> {
        detect_default_browser(self.options.verbose)
            .await
            .and_then(source_id)
    }

    /// Offers, once, to import the sign-in for `domains` from an installed
    /// browser into the automated profile, then relaunches it with the
    /// import. Only a dedicated profile is offered the import, and only
    /// without a named `--browser-import`, which already migrated at launch.
    /// Returns whether the sign-in was imported.
    async fn offer_sign_in_import(&mut self, domains: &[&str]) -> Result<bool> {
        let browser = self.options.browser_options;
        let choice = browser
            .import
            .as_ref()
            .map(|source| source.browser.as_str());
        if browser.attach.is_some()
            || choice.is_some_and(|choice| !IMPORT_CHOICES.contains(&choice))
        {
            return Ok(false);
        }
        let preferred = self.default_source().await;
        let sources = if choice == Some("default") {
            default_sources(preferred, domains)
        } else {
            find_sign_in_sources(&InstalledBrowsers, domains, preferred, self.options.verbose)
        };
        let Some(source) = choose_sign_in_source(sources, domains, prompt, choice.is_some())?
        else {
            return Ok(false);
        };
        let mut imported = browser.clone();
        imported.import = Some(ImportSource {
            browser: source.browser,
            profile: source.profile,
        });
        self.connect(&imported).await?;
        println!(
            "  Imported the {} sign-in from {}.",
            domains.join(" / "),
            source.label
        );
        Ok(true)
    }
}
