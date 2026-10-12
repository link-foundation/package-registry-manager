//! Managed Python builds and publisher-form polling.
use super::{command_of, is_yes, prompt, Session};
use crate::model::{CommandSpec, Registry, SetupStep};
use anyhow::{bail, Result};

#[allow(clippy::future_not_send)]
impl Session<'_> {
    pub(super) async fn build_python(&mut self, step: &SetupStep) -> Result<()> {
        let temporary = tempfile::Builder::new()
            .prefix("prm-python-build-")
            .tempdir()?;
        let python = temporary
            .path()
            .join(if cfg!(windows) {
                "Scripts/python.exe"
            } else {
                "bin/python"
            })
            .to_string_lossy()
            .into_owned();
        let commands = [
            CommandSpec {
                program: command_of(step)?.program.clone(),
                args: vec![
                    "-m".into(),
                    "venv".into(),
                    temporary.path().to_string_lossy().into_owned(),
                ],
            },
            CommandSpec {
                program: python.clone(),
                args: [
                    "-m",
                    "pip",
                    "install",
                    "--disable-pip-version-check",
                    "build",
                ]
                .map(str::to_owned)
                .to_vec(),
            },
            CommandSpec {
                program: python,
                args: vec!["-m".into(), "build".into()],
            },
        ];
        for command in commands {
            let mut build = step.clone();
            build.command = Some(command);
            let result = self.run_process(&build, true).await?;
            if result.code != 0 {
                bail!("Python build preparation or build failed (status {}); ensure the selected Python supports venv and pip", result.code);
            }
        }
        Ok(())
    }

    pub(super) async fn prefill(&mut self) -> Result<()> {
        let (Some(browser), Some(config)) = (&self.browser, &self.plan.oidc_publisher) else {
            return Ok(());
        };
        let pending = self.plan.registry == Registry::PyPi;
        let interval = self.options.poll_interval;
        let timeout = self.options.wait_timeout;
        crate::publisher_form::wait_for_form(browser, config, pending, interval, timeout).await?;
        if pending
            || self.options.yes
            || is_yes(&prompt(
                "Submit this trusted-publisher configuration? [y/N] ",
            )?)
        {
            crate::publisher_form::submit_form(browser, config, pending, interval, timeout).await?;
            if pending {
                self.conditions.remove("trust-missing");
            }
        } else {
            bail!("trusted-publisher submission declined; release not started");
        }
        Ok(())
    }
}
