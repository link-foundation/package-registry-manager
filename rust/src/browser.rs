use std::time::Duration;

use anyhow::{bail, Result};
use browser_commander::browser::open_in_user_browser::{
    build_open_command, open_in_user_browser as open_with_system_opener,
};
use browser_commander::utilities::subprocess::{run_command, CommandError, RunCommandOptions};
use regex::Regex;

use crate::model::TrustedPublisherPrefill;

/// How long an opener may run before the tool stops waiting for it:
/// `xdg-open` can wait for a newly started browser to exit.
pub const OPENER_GRACE: Duration = Duration::from_secs(2);

/// The platform whose opener Browser Commander runs, from a value of
/// [`std::env::consts::OS`]: `open` on macOS, `explorer.exe` on Windows, and
/// `xdg-open` on Linux and other Unix systems.
#[must_use]
pub fn opener_platform(os: &str) -> &'static str {
    match os {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
}

/// Whether a failed opener still handed the URL over: `explorer.exe` exits
/// with 1 after it did.
#[must_use]
pub fn opener_succeeded(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<CommandError>(),
        Some(CommandError::Exited { file, code: 1, .. }) if file == "explorer.exe"
    )
}

/// Open an http(s) URL in the user's own default browser.
///
/// The user is usually already signed in there, and nothing is automated.
/// Browser Commander builds the opener's exact argument vector and runs it
/// through command-stream.
///
/// Returns once the opener exits, or after [`OPENER_GRACE`] while it keeps
/// running in the background.
///
/// # Errors
/// Fails for a non-web URL, or when the opener cannot start or fails.
pub async fn open_in_user_browser(url: &str) -> Result<()> {
    let web = Regex::new(r"(?i)^https?://\S+$").expect("static pattern must compile");
    if !web.is_match(url) {
        bail!("refusing to open a non-web URL: {url}");
    }
    let url = url.to_owned();
    let platform = opener_platform(std::env::consts::OS);
    // A detached task keeps a slow opener running while the setup continues.
    let opener = tokio::spawn(async move {
        if platform == std::env::consts::OS {
            open_with_system_opener(&url).await.map(drop)
        } else {
            let command = build_open_command(&url, platform)?;
            run_command(&command[0], &command[1..], RunCommandOptions::default()).await?;
            Ok(())
        }
    });
    match tokio::time::timeout(OPENER_GRACE, opener).await {
        Err(_) => Ok(()),
        Ok(joined) => match joined? {
            Err(error) if !opener_succeeded(&error) => Err(error),
            _ => Ok(()),
        },
    }
}

/// Build a self-contained script that fills a trusted-publisher form (npm,
/// crates.io, or a `PyPI` pending publisher).
///
/// Values are serialized as JSON before interpolation, so repository metadata
/// cannot escape into executable JavaScript.
pub fn npm_prefill_script(prefill: &TrustedPublisherPrefill, submit: bool) -> Result<String> {
    let config = serde_json::to_string(prefill)?;
    let submit = serde_json::to_string(&submit)?;
    Ok(format!(
        r#"(() => {{
  const config = {config};
  const shouldSubmit = {submit};
  const normalized = value => (value || '').replace(/\s+/g, ' ').trim().toLowerCase();
  const controls = Array.from(document.querySelectorAll('input, select, textarea'));
  const setValue = (control, value) => {{
    const prototype = control instanceof HTMLSelectElement
      ? HTMLSelectElement.prototype
      : control instanceof HTMLTextAreaElement
        ? HTMLTextAreaElement.prototype
        : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
    if (setter) setter.call(control, value); else control.value = value;
    control.dispatchEvent(new Event('input', {{ bubbles: true }}));
    control.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }};
  const textFor = control => {{
    const explicit = control.id
      ? document.querySelector(`label[for="${{CSS.escape(control.id)}}"]`)?.textContent
      : '';
    return normalized([explicit, control.getAttribute('aria-label'), control.name,
      control.placeholder, control.closest('label')?.textContent].filter(Boolean).join(' '));
  }};
  const values = [
    [['project'], config.project],
    [['organization', 'owner'], config.organization],
    [['repository', 'repo'], config.repository],
    [['workflow'], config.workflow],
    [['environment'], config.environment || ''],
  ];
  const filled = [];
  const used = new Set();
  for (const [labels, value] of values) {{
    if (!value) continue;
    const control = controls.find(candidate => !used.has(candidate)
      && labels.some(label => textFor(candidate).includes(label)));
    if (control) {{ setValue(control, value); used.add(control); filled.push(labels[0]); }}
  }}
  const provider = Array.from(document.querySelectorAll('button, label, [role="radio"]'))
    .find(element => normalized(element.textContent).includes('github'));
  if (provider && filled.length === 0) provider.click();
  let submitted = false;
  if (shouldSubmit && filled.length >= 3) {{
    const button = Array.from(document.querySelectorAll('button, input[type="submit"]'))
      .find(candidate => /add|save|configure|submit/.test(normalized(candidate.textContent || candidate.value))
        && !candidate.disabled);
    if (button) {{ button.click(); submitted = true; }}
  }}
  return {{ filled, providerSelected: Boolean(provider), submitted }};
}})()"#
    ))
}
