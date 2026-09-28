import { spawn } from "node:child_process";

/**
 * The exact argument vector that opens an http(s) URL in the user's default
 * browser: `open` on macOS, `xdg-open` on Linux and other Unix systems, and
 * the URL protocol handler on Windows, which avoids `cmd` quoting rules.
 */
export function userBrowserCommand(url, platform = process.platform) {
  if (!/^https?:\/\/\S+$/i.test(url)) {
    throw new Error(`refusing to open a non-web URL: ${url}`);
  }
  if (platform === "darwin") {
    return { program: "open", args: [url] };
  }
  if (platform === "win32") {
    return { program: "rundll32", args: ["url.dll,FileProtocolHandler", url] };
  }
  return { program: "xdg-open", args: [url] };
}

/**
 * Opens a URL in the user's own default browser, where they are usually
 * already signed in. Nothing is automated. Resolves once the opener started;
 * it is not awaited because `xdg-open` can wait for a new browser to exit.
 */
export function openInUserBrowser(url, platform = process.platform) {
  const command = userBrowserCommand(url, platform);
  return new Promise((resolve, reject) => {
    const child = spawn(command.program, command.args, {
      detached: true,
      stdio: "ignore",
      shell: false,
    });
    child.once("error", (error) =>
      reject(new Error(`${command.program}: ${error.message}`)),
    );
    child.once("spawn", () => {
      child.unref();
      resolve();
    });
  });
}

export function npmPrefillScript(prefill, submit = false) {
  return `(() => {
  const config = ${JSON.stringify(prefill)};
  const shouldSubmit = ${JSON.stringify(submit)};
  const normalized = value => (value || '').replace(/\\s+/g, ' ').trim().toLowerCase();
  const controls = Array.from(document.querySelectorAll('input, select, textarea'));
  const setValue = (control, value) => {
    const prototype = control instanceof HTMLSelectElement
      ? HTMLSelectElement.prototype
      : control instanceof HTMLTextAreaElement
        ? HTMLTextAreaElement.prototype
        : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
    if (setter) setter.call(control, value); else control.value = value;
    control.dispatchEvent(new Event('input', { bubbles: true }));
    control.dispatchEvent(new Event('change', { bubbles: true }));
  };
  const textFor = control => {
    const explicit = control.id
      ? document.querySelector(\`label[for="\${CSS.escape(control.id)}"]\`)?.textContent
      : '';
    return normalized([explicit, control.getAttribute('aria-label'), control.name,
      control.placeholder, control.closest('label')?.textContent].filter(Boolean).join(' '));
  };
  const values = [
    [['project'], config.project],
    [['organization', 'owner'], config.organization],
    [['repository', 'repo'], config.repository],
    [['workflow'], config.workflow],
    [['environment'], config.environment || ''],
  ];
  const filled = [];
  const used = new Set();
  for (const [labels, value] of values) {
    if (!value) continue;
    const control = controls.find(candidate => !used.has(candidate)
      && labels.some(label => textFor(candidate).includes(label)));
    if (control) { setValue(control, value); used.add(control); filled.push(labels[0]); }
  }
  const provider = Array.from(document.querySelectorAll('button, label, [role="radio"]'))
    .find(element => normalized(element.textContent).includes('github'));
  if (provider && filled.length === 0) provider.click();
  let submitted = false;
  if (shouldSubmit && filled.length >= 3) {
    const button = Array.from(document.querySelectorAll('button, input[type="submit"]'))
      .find(candidate => /add|save|configure|submit/.test(normalized(candidate.textContent || candidate.value))
        && !candidate.disabled);
    if (button) { button.click(); submitted = true; }
  }
  return { filled, providerSelected: Boolean(provider), submitted };
})()`;
}
