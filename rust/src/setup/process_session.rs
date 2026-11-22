//! Interactive setup commands and npm approval retries.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use tokio::sync::{mpsc, oneshot, watch};

use super::Session;
use crate::approvals::{approval_deadline, browser, next_approval, LinkKind, TWO_FACTOR_HINT};
use crate::auth_urls::{
    node_options_with_shim, run_interactive_controlled, write_tty_shim, ApprovalControl,
    CommandOutput,
};
use crate::model::{CommandSpec, SetupStep};

#[allow(clippy::future_not_send)]
impl Session<'_> {
    pub(super) async fn run_process(
        &mut self,
        step: &SetupStep,
        mirror: bool,
    ) -> Result<CommandOutput> {
        self.run_process_with(step, mirror, &BTreeMap::new()).await
    }

    pub(super) async fn run_process_with(
        &mut self,
        step: &SetupStep,
        mirror: bool,
        extra: &BTreeMap<String, String>,
    ) -> Result<CommandOutput> {
        let (command, cwd) = self.prepare(step)?;
        let npm = matches!(command.program.as_str(), "npm" | "npx");
        let mut env = extra.clone();
        if npm {
            self.detect_default_browser().await;
            if self.shim.is_none() {
                self.shim = Some(write_tty_shim()?);
            }
            if let Some((_, shim)) = &self.shim {
                let existing = env
                    .get("NODE_OPTIONS")
                    .cloned()
                    .or_else(|| std::env::var("NODE_OPTIONS").ok());
                env.insert(
                    "NODE_OPTIONS".to_owned(),
                    node_options_with_shim(existing.as_deref(), &shim.to_string_lossy()),
                );
            }
        }
        let trust_create = npm && step.id == "attach-trusted-publisher";
        for attempt in 1.. {
            let (result, linked, failure) = self
                .run_once(&command, &cwd, &env, mirror, npm, trust_create)
                .await?;
            if !npm {
                return Ok(result);
            }
            match next_approval(&result, linked, failure.as_deref(), attempt)? {
                Some(message) => println!("{message}"),
                None => return Ok(result),
            }
        }
        unreachable!()
    }

    async fn run_once(
        &mut self,
        command: &CommandSpec,
        cwd: &Path,
        env: &BTreeMap<String, String>,
        mirror: bool,
        npm: bool,
        trust_create: bool,
    ) -> Result<(CommandOutput, bool, Option<String>)> {
        let (sender, mut urls) = mpsc::unbounded_channel();
        let (stop, cancel) = oneshot::channel();
        let (finished, mut done) = watch::channel(false);
        let automate = npm && self.options.open_with.is_none();
        let monitor = automate && !self.options.no_browser;
        let command = run_interactive_controlled(
            command,
            cwd,
            env,
            mirror,
            Some(sender),
            npm,
            npm,
            npm.then_some(ApprovalControl {
                stop: Some(cancel),
                finished,
                trust_create,
            }),
        );
        let follow_links = async {
            let mut linked = false;
            let mut next = urls.recv().await;
            while let Some((url, kind)) = next {
                linked = true;
                println!("{}", approval_deadline(kind, chrono::Local::now().time()));
                if kind == LinkKind::Approve {
                    println!("{TWO_FACTOR_HINT}");
                }
                if !monitor || !*done.borrow() {
                    if let Err(error) = self.open(&url, automate).await {
                        eprintln!("warning: could not open {url}: {error:#}");
                    } else if let Some(page) = self.browser.as_ref().filter(|_| monitor) {
                        tokio::select! {
                            biased;
                            _ = done.wait_for(|finished| *finished) => {}
                            link = urls.recv() => {
                                next = link;
                                continue;
                            }
                            failure = browser::watch(page) => {
                                let _ = stop.send(());
                                return (linked, Some(failure));
                            }
                        }
                    }
                }
                next = urls.recv().await;
            }
            (linked, None)
        };
        let (result, (linked, failure)) = tokio::join!(command, follow_links);
        Ok((result?, linked, failure))
    }
}
