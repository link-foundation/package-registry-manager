//! Browser routing shared by login, approval, and publisher steps.
use super::{render, resolve_program, BrowserMode, Session};
use crate::browser::open_in_user_browser;
use crate::default_browser::{detect_default_browser, open_with_command};
use anyhow::{bail, Result};
use command_stream::StreamingRunner;
use std::time::Duration;

#[allow(clippy::future_not_send)]
impl Session<'_> {
    /// Open a URL in the default browser, or in the automation profile when
    /// `automate` is set (form filling) or `--browser automated` was chosen.
    pub(super) async fn open(&mut self, url: &str, automate: bool) -> Result<()> {
        if self.options.no_browser {
            println!("Open {url}");
            return Ok(());
        }
        if !automate && self.options.browser == BrowserMode::Default {
            let app = self.options.open_with;
            let label = self.default_browser_label();
            println!("Opening {url} in {}", app.unwrap_or(&label));
            let opened = match app {
                Some(app) => self.open_with(url, app).await,
                None => open_in_user_browser(url).await,
            };
            if let Err(error) = opened {
                eprintln!(
                    "warning: could not open {} ({error:#}); open the URL yourself",
                    app.unwrap_or("your default browser")
                );
            }
            return Ok(());
        }
        self.visible_automated().await?.goto(url).await
    }

    async fn open_with(&self, url: &str, app: &str) -> Result<()> {
        let command = open_with_command(url, app, std::env::consts::OS)?;
        if self.options.verbose {
            eprintln!("+ {}", render(&command));
        }
        // Like the default browser's opener, a slow application keeps running.
        let opener = tokio::spawn(async move {
            StreamingRunner::from_argv(resolve_program(&command.program), &command.args)
                .collect()
                .await
        });
        match tokio::time::timeout(Duration::from_secs(5), opener).await {
            Err(_) => Ok(()),
            Ok(joined) => match joined?? {
                result if result.code == 0 => Ok(()),
                result => bail!("{app} exited with code {}", result.code),
            },
        }
    }

    /// Names the default browser once per run, so maintainers know where to
    /// look. Runs before npm starts, while no link is waiting to be opened.
    pub(super) async fn detect_default_browser(&mut self) {
        let options = self.options;
        if options.no_browser
            || options.open_with.is_some()
            || options.browser != BrowserMode::Default
        {
            return;
        }
        if !self.browser_detected {
            self.browser_name = detect_default_browser(options.verbose).await;
            self.browser_detected = true;
        }
    }

    fn default_browser_label(&self) -> String {
        self.browser_name.map_or_else(
            || "your default browser".to_owned(),
            |name| format!("{name}, your default browser"),
        )
    }
}
