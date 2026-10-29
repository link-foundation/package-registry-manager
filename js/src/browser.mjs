import { openInUserBrowser as openWithSystemOpener } from "browser-commander";
import { spawn } from "command-stream";

/**
 * How long an opener may run before the tool stops waiting for it: `xdg-open`
 * can wait for a newly started browser to exit.
 */
export const OPENER_GRACE_MS = 2_000;

/**
 * The platform whose opener Browser Commander runs: `open` on macOS,
 * `explorer.exe` on Windows, and `xdg-open` on Linux and other Unix systems.
 */
export function openerPlatform(platform = process.platform) {
  return platform === "darwin" || platform === "win32" ? platform : "linux";
}

/**
 * Opens an http(s) URL in the user's own default browser, where they are
 * usually already signed in. Nothing is automated. Browser Commander builds
 * the opener's exact argument vector; `runner` starts it.
 */
export async function openInUserBrowser(
  url,
  { platform = process.platform, runner = detachedRunner() } = {},
) {
  if (!/^https?:\/\/\S+$/i.test(url)) {
    throw new Error(`refusing to open a non-web URL: ${url}`);
  }
  return openWithSystemOpener(url, {
    platform: openerPlatform(platform),
    runner,
  });
}

/** Whether an opener handed the URL over: `explorer.exe` exits with 1 after it did. */
export function openerSucceeded(file, code) {
  return code === 0 || (file === "explorer.exe" && code === 1);
}

/**
 * Starts an opener through command-stream without a shell and settles once it
 * exits, or after `graceMs` while it keeps running in the background.
 */
export function detachedRunner(graceMs = OPENER_GRACE_MS, spawnChild = spawn) {
  return (file, args) =>
    new Promise((resolve, reject) => {
      const child = spawnChild(file, args, {
        detached: process.platform !== "win32",
        stdio: "ignore",
        shell: false,
      });
      const timer = setTimeout(() => {
        child.unref();
        resolve({ code: null });
      }, graceMs);
      child.once("error", (error) => {
        clearTimeout(timer);
        reject(new Error(`Could not start ${file}: ${error.message}`));
      });
      child.once("exit", (code, signal) => {
        clearTimeout(timer);
        if (openerSucceeded(file, code)) {
          resolve({ code });
        } else {
          reject(new Error(`${file} exited with code ${code ?? signal}`));
        }
      });
    });
}

export function npmPrefillScript(prefill, submit = false) {
  return `(() => {
  const config = ${JSON.stringify(prefill)};
  const shouldSubmit = ${JSON.stringify(submit)};
  const normalized = value => (value || '').replace(/\\s+/g, ' ').trim().toLowerCase();
  const provider = Array.from(document.querySelectorAll('button, label, [role="radio"]'))
    .find(element => normalized(element.textContent) === 'github');
  if (provider) provider.click();
  const githubForm = document.querySelector('#pending-github-publisher-form, #github-publisher-form');
  const controls = Array.from((githubForm || document).querySelectorAll('input, select, textarea'))
    .filter(control => !control.disabled && control.type !== 'hidden'
      && (githubForm || control.getClientRects().length > 0));
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
      ? (control.closest('form') || document).querySelector(\`label[for="\${CSS.escape(control.id)}"]\`)?.textContent
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
    if (!value && labels[0] !== 'environment') continue;
    const control = controls.find(candidate => !used.has(candidate)
      && labels.some(label => textFor(candidate).includes(label)));
    if (control) { setValue(control, value); used.add(control); filled.push(labels[0]); }
  }
  let submitted = false;
  const required = ['organization', 'repository', 'workflow', ...(githubForm?.id.startsWith('pending-') ? ['project'] : [])];
  const form = githubForm || controls.find(control => /repository|repo/.test(textFor(control)))?.closest('form');
  if (shouldSubmit && required.every(field => filled.includes(field))) {
    const button = Array.from((form || document).querySelectorAll('button, input[type="submit"]'))
      .find(candidate => /add|save|configure|submit/.test(normalized(candidate.textContent || candidate.value))
        && !candidate.disabled && !candidate.classList.contains('button--disabled'));
    if (button) { button.click(); submitted = true; }
  }
  return { filled, providerSelected: Boolean(provider), submitted };
})()`;
}
