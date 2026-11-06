//! Native browser bridge for the shared Packagist engine.
use super::{prompt, Session, SetupStep, Value};
use anyhow::{bail, Context, Result};
use serde_json::json;
use tokio::io::{AsyncWriteExt, BufReader};

#[allow(clippy::future_not_send)] // Browser Commander's CDP transport is intentionally !Sync.
impl Session<'_> {
    pub(super) async fn run_packagist(&mut self, step: &SetupStep) -> Result<Value> {
        if let Some(cached) = self
            .packagist_results
            .iter()
            .find(|entry| entry["step"] == step.id)
        {
            return Ok(cached.clone());
        }
        let mut engine = crate::packagist::start(&json!({
            "plan": self.plan,
            "options": { "repository": self.options.repository, "yes": self.options.yes, "noBrowser": self.options.no_browser, "waitTimeoutMs": self.options.wait_timeout.as_millis() }
        }), self.options.repository).await?;
        let child = &mut engine.child;
        let mut stdout = BufReader::new(
            child
                .stdout
                .take()
                .context("Packagist stdout unavailable")?,
        )
        .lines();
        let result = loop {
            let message = crate::packagist::message(&mut stdout).await?;
            if message.get("results").is_some() || message.get("error").is_some() {
                break message;
            }
            let argument = message["argument"].as_str().unwrap_or_default();
            let response: Result<Value> = match message["operation"].as_str() {
                Some("prompt") => prompt(argument).map(Value::String),
                Some("goto") => self
                    .visible_automated()
                    .await?
                    .goto(argument)
                    .await
                    .map(|()| Value::Null),
                Some("evaluate") => self.visible_automated().await?.evaluate(argument).await,
                _ => bail!("unknown Packagist browser protocol operation"),
            };
            let reply = match response {
                Ok(value) => json!({"value":value}),
                Err(_) => json!({"error":true}),
            };
            child
                .stdin
                .as_mut()
                .context("Packagist stdin unavailable")?
                .write_all(format!("{reply}\n").as_bytes())
                .await?;
        };
        drop(child.stdin.take());
        let status = child.wait().await?;
        if !status.success() || result.get("error").is_some() {
            bail!("Packagist engine failed; sensitive output withheld");
        }
        self.packagist_results = result["results"]
            .as_array()
            .context("Packagist results missing")?
            .clone();
        self.packagist_results
            .iter()
            .find(|entry| entry["step"] == step.id)
            .cloned()
            .context("Packagist step did not run")
    }
}
use tokio::io::AsyncBufReadExt;
