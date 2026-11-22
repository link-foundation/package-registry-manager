use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;
use tokio::time::{sleep, Instant};

use super::APPROVAL_WINDOW;
use crate::automation::Automation;

const COOLDOWN: &str = "#cooldownOptin_didOptForCooldown";
const KEY: &str = "[data-prm-npm-security-key]";

const STATE: &str = r#"(() => {
  if (location.protocol === 'chrome-error:') return { failure: 'the approval page could not load' };
  if (location.protocol !== 'https:' || !/(^|\.)npmjs\.com$/i.test(location.hostname)) return {};
  const text = document.body?.innerText ?? '';
  if (/something went wrong/i.test(text)) return { failure: 'the approval page reported Something went wrong' };
  if (/\bexpired\b|\binvalid (?:approval|link|session)\b/i.test(text)) {
    return { failure: 'the approval page reported an expired or invalid link' };
  }
  const box = document.querySelector('#cooldownOptin_didOptForCooldown');
  const button = Array.from(document.querySelectorAll('button')).find((candidate) =>
    /^(?:use|authenticate with|continue with)\s+security key$/i.test(candidate.innerText.trim()) &&
    !candidate.matches(':disabled') && candidate.getClientRects().length
  );
  for (const marked of document.querySelectorAll('[data-prm-npm-security-key]')) {
    if (marked !== button) marked.removeAttribute('data-prm-npm-security-key');
  }
  button?.setAttribute('data-prm-npm-security-key', '');
  return { cooldown: box ? box.checked : null, key: Boolean(button) };
})()"#;

#[derive(Default, Deserialize)]
struct State {
    cooldown: Option<bool>,
    #[serde(default)]
    key: bool,
    failure: Option<String>,
}

trait ApprovalPage {
    async fn state(&self) -> State;
    async fn click(&self, selector: &str) -> Result<()>;
}

impl ApprovalPage for Automation {
    async fn state(&self) -> State {
        self.evaluate(STATE)
            .await
            .ok()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    async fn click(&self, selector: &str) -> Result<()> {
        self.click(selector).await
    }
}

pub(crate) async fn watch(page: &Automation) -> String {
    watch_page(page, APPROVAL_WINDOW, Duration::from_millis(500)).await
}

async fn watch_page(page: &impl ApprovalPage, timeout: Duration, interval: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut requested_key = false;
    loop {
        let state = page.state().await;
        if let Some(failure) = state.failure {
            return failure;
        }
        if !requested_key && state.cooldown == Some(false) {
            let _ = page.click(COOLDOWN).await;
        } else if !requested_key && state.key {
            // Navigation can interrupt the driver's acknowledgement after a click.
            requested_key = true;
            if page.click(KEY).await.is_ok() {
                println!(
                    "  Clicked \"Use security key\"; approve with Touch ID or your security key."
                );
            }
        }
        if Instant::now() >= deadline {
            return "the browser approval deadline elapsed".to_owned();
        }
        sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[derive(Default)]
    struct Page {
        box_clicks: Cell<usize>,
        after_key: Cell<usize>,
        clicks: RefCell<Vec<String>>,
    }

    impl ApprovalPage for Page {
        async fn state(&self) -> State {
            let mut state = State {
                cooldown: Some(self.box_clicks.get() >= 2),
                key: true,
                failure: None,
            };
            if self.clicks.borrow().iter().any(|selector| selector == KEY) {
                self.after_key.set(self.after_key.get() + 1);
                if self.after_key.get() == 3 {
                    state.failure = Some("Something went wrong".to_owned());
                }
            }
            state
        }

        async fn click(&self, selector: &str) -> Result<()> {
            if selector == COOLDOWN {
                self.box_clicks.set(self.box_clicks.get() + 1);
            } else {
                assert_eq!(self.box_clicks.get(), 2);
            }
            self.clicks.borrow_mut().push(selector.to_owned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn npm_approval_clicks_once_after_readback_and_keeps_watching() {
        let page = Page::default();
        let failure = watch_page(&page, Duration::from_secs(2), Duration::from_millis(1)).await;
        assert_eq!(failure, "Something went wrong");
        assert_eq!(*page.clicks.borrow(), [COOLDOWN, COOLDOWN, KEY]);
    }
}
