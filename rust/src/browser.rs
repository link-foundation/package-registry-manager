use std::process::Stdio;

use anyhow::{bail, Context, Result};
use regex::Regex;

use crate::model::{CommandSpec, TrustedPublisherPrefill};

/// The exact argument vector that opens an http(s) URL in the default browser.
///
/// `os` is a value of [`std::env::consts::OS`]: `open` runs on macOS, the URL
/// protocol handler on Windows, which avoids `cmd` quoting rules, and
/// `xdg-open` elsewhere.
pub fn user_browser_command(url: &str, os: &str) -> Result<CommandSpec> {
    let web = Regex::new(r"(?i)^https?://\S+$").expect("static pattern must compile");
    if !web.is_match(url) {
        bail!("refusing to open a non-web URL: {url}");
    }
    let (program, mut args) = match os {
        "macos" => ("open", Vec::new()),
        "windows" => ("rundll32", vec!["url.dll,FileProtocolHandler".to_owned()]),
        _ => ("xdg-open", Vec::new()),
    };
    args.push(url.to_owned());
    Ok(CommandSpec {
        program: program.to_owned(),
        args,
    })
}

/// Open a URL in the user's own default browser, where they are usually
/// already signed in. Nothing is automated.
///
/// Returns once the opener started; it is not awaited because `xdg-open` can
/// wait for a newly started browser to exit.
pub fn open_in_user_browser(url: &str) -> Result<()> {
    let command = user_browser_command(url, std::env::consts::OS)?;
    let mut opener = std::process::Command::new(&command.program);
    opener
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Keep the browser running when the terminal interrupts the tool.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut opener, 0);
    let mut child = opener
        .spawn()
        .with_context(|| format!("{}: cannot start", command.program))?;
    // Reap the opener in the background so it does not linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
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
