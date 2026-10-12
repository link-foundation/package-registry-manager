//! Wait for sign-in and verify trusted-publisher form submission without stdin.
use crate::browser::npm_prefill_script;
use crate::model::TrustedPublisherPrefill;
use anyhow::{bail, Result};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Page access for deterministic tests and the live automated browser.
#[allow(async_fn_in_trait)]
pub trait PublisherPage {
    /// Evaluate a script without logging page secrets.
    async fn evaluate(&self, script: &str) -> Result<Value>;
}
impl PublisherPage for crate::automation::Automation {
    async fn evaluate(&self, script: &str) -> Result<Value> {
        Self::evaluate(self, script).await
    }
}

/// Script that checks the saved publisher row and server-side validation errors.
pub fn confirmation_script(config: &TrustedPublisherPrefill) -> Result<String> {
    let config = serde_json::to_string(config)?;
    Ok(format!(
        r"(() => {{
      const config = {config};
      const errors = Array.from(document.querySelectorAll('#pending-github-publisher-form .form-errors, #pending-github-publisher-form .form-group__error, .error-message, .notification-bar--danger')).map(node => node.textContent.trim()).filter(Boolean);
      const rows = Array.from(document.querySelectorAll('tr, .publisher, .card'));
      const confirmed = rows.some(row => {{
      const repository = Array.from(row.querySelectorAll('a[href]')).some(link => {{
        try {{ const url = new URL(link.href); return url.hostname === 'github.com' && (url.pathname === '/' + config.organization + '/' + config.repository || url.pathname.startsWith('/' + config.organization + '/' + config.repository + '/')); }} catch {{ return false; }}
      }});
      return repository && row.querySelector('td')?.textContent.trim() === config.project
        && [config.workflow, config.environment].filter(Boolean).every(value => row.innerText.split(/\s+/).includes(value));
    }});
      return {{ confirmed, error: errors.join('; ') }};
    }})()"
    ))
}

/// Wait for all required controls, including the project for a pending publisher.
#[allow(clippy::future_not_send)]
pub async fn wait_for_form(
    page: &impl PublisherPage,
    config: &TrustedPublisherPrefill,
    pending: bool,
    interval: Duration,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let script = npm_prefill_script(config, false)?;
    println!("  Finish sign-in and 2FA in the browser; waiting for the trusted-publisher form.");
    loop {
        if let Ok(result) = page.evaluate(&script).await {
            let fields = if pending {
                vec!["project", "organization", "repository", "workflow"]
            } else {
                vec!["organization", "repository", "workflow"]
            };
            if fields.iter().all(|field| {
                result["filled"]
                    .as_array()
                    .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(field)))
            }) {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for sign-in and the trusted-publisher form; nothing submitted"
            );
        }
        tokio::time::sleep(interval).await;
    }
}

/// Submit the ready form and wait for `PyPI`'s saved publisher row.
#[allow(clippy::future_not_send)]
pub async fn submit_form(
    page: &impl PublisherPage,
    config: &TrustedPublisherPrefill,
    pending: bool,
    interval: Duration,
    timeout: Duration,
) -> Result<()> {
    if page.evaluate(&npm_prefill_script(config, true)?).await?["submitted"] != true {
        bail!("the form was not submitted; review the visible browser");
    }
    if !pending {
        return Ok(());
    }
    let deadline = Instant::now() + timeout;
    let script = confirmation_script(config)?;
    loop {
        if let Ok(state) = page.evaluate(&script).await {
            if let Some(error) = state["error"].as_str().filter(|error| !error.is_empty()) {
                bail!("PyPI refused the pending publisher: {error}");
            }
            if state["confirmed"] == true {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            bail!("timed out verifying the PyPI pending publisher; release not started");
        }
        tokio::time::sleep(interval).await;
    }
}
