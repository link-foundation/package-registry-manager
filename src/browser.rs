use anyhow::Result;

use crate::model::TrustedPublisherPrefill;

/// Build a self-contained script that fills npm's trusted-publisher form.
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
    [['organization', 'owner'], config.organization],
    [['repository', 'repo'], config.repository],
    [['workflow'], config.workflow],
    [['environment'], config.environment || ''],
  ];
  const filled = [];
  for (const [labels, value] of values) {{
    if (!value) continue;
    const control = controls.find(candidate => labels.some(label => textFor(candidate).includes(label)));
    if (control) {{ setValue(control, value); filled.push(labels[0]); }}
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
