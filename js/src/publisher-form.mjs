import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { isSamePage } from "./automation.mjs";
import { npmPrefillScript } from "./browser.mjs";

const run = promisify(execFile);

/**
 * Submit a sign-in form only after the browser has prefilled both the username
 * and password. The script never reads or returns credential values, and the
 * sessionStorage marker prevents repeated automatic submissions after a failed
 * sign-in or a 2FA page reload.
 */
export function signInScript() {
  return `(() => {
    const form = Array.from(document.forms).find(candidate =>
      candidate.querySelector('input[name="username"], input[autocomplete="username"], input[type="email"]') &&
      candidate.querySelector('input[type="password"]')
    );
    if (!form) return { present: false, prefilled: false, submitted: false };
    const username = form.querySelector('input[name="username"], input[autocomplete="username"], input[type="email"]');
    const password = form.querySelector('input[type="password"]');
    const prefilled = Boolean(username?.value && password?.value && !username.disabled && !password.disabled);
    if (!prefilled) return { present: true, prefilled: false, submitted: false };
    for (const field of [username, password]) {
      field.dispatchEvent(new Event('input', { bubbles: true }));
      field.dispatchEvent(new Event('change', { bubbles: true }));
    }
    let submittedBefore = false;
    try { submittedBefore = sessionStorage.getItem('prm-sign-in-submitted') === '1'; } catch {}
    if (submittedBefore) {
      const errorText = Array.from(document.querySelectorAll('.form-errors, .form-group__error, .error-message, .notification-bar--danger')).map(node => node.textContent.trim()).filter(Boolean).join('; ');
      return { present: true, prefilled: true, submitted: false, alreadySubmitted: true, failed: /invalid (?:or expired )?(?:two[- ]factor )?login|expired two[- ]factor login/i.test(errorText) };
    }
    const button = Array.from(form.querySelectorAll('button, input[type="submit"]'))
      .find(candidate => /log in|sign in|continue|submit/i.test((candidate.textContent || candidate.value || '').trim()));
    if (!button) return { present: true, prefilled: true, submitted: false, error: 'no submit button' };
    if (button.disabled) return { present: true, prefilled: true, submitted: false, pendingButton: true };
    try { sessionStorage.setItem('prm-sign-in-submitted', '1'); } catch {}
    button.click();
    return { present: true, prefilled: true, submitted: true };
  })()`;
}

/**
 * Check the optional "Remember this device" box on a 2FA page and report
 * whether the login-confirmation page is open. It checks every page in the
 * active browser because PyPI may open the confirmation link in a new tab.
 */
export function loginDeviceScript() {
  return `(() => {
    const text = document.body?.innerText ?? '';
    const confirmed = /your login has been confirmed and this device is now recognized/i.test(text);
    const authSuccessful = /authentication successful|you can close this tab/i.test(text);
    const boxes = Array.from(document.querySelectorAll('input[type="checkbox"]')).filter((box) => {
      const label = box.closest('label') ?? (box.id ? document.querySelector('label[for="' + box.id.replaceAll('"', '\\\\"') + '"]') : null);
      return /^remember[_-]?device$/i.test(box.name || '')
        || /remember this device(?: for \\d+ days?)?/i.test((label?.innerText || box.getAttribute('aria-label') || '').trim());
    });
    let remembered = 0;
    for (const box of boxes) {
      if (box.disabled || box.checked) continue;
      box.click();
      box.dispatchEvent(new Event('input', { bubbles: true }));
      box.dispatchEvent(new Event('change', { bubbles: true }));
      if (box.checked) remembered += 1;
    }
    return {
      present: boxes.length > 0,
      checked: boxes.length > 0 && boxes.every((box) => box.checked),
      remembered,
      confirmed,
      authSuccessful,
    };
  })()`;
}

/** Whether a password field is visible and still empty, without reading its value. */
export function passwordPromptScript() {
  return `(() => {
    const field = Array.from(document.querySelectorAll('input[type="password"]')).find((candidate) => !candidate.disabled && candidate.offsetParent !== null);
    const form = field?.form ?? null;
    const button = form ? Array.from(form.querySelectorAll('button, input[type="submit"]')).find((candidate) => /confirm password/i.test((candidate.textContent || candidate.value || '').trim())) : null;
    const errors = Array.from(document.querySelectorAll('.form-errors, .form-group__error, .error-message, .notification-bar--danger')).map((node) => node.textContent.trim()).filter(Boolean).join('; ');
    return {
      present: Boolean(field),
      filled: Boolean(field?.value),
      confirmation: Boolean(button),
      alreadySubmitted: (() => { try { return sessionStorage.getItem('prm-password-confirmed') === '1'; } catch { return false; } })(),
      failed: /invalid password|incorrect password/i.test(errors),
      error: errors,
    };
  })()`;
}

/** The PyPI "Confirm password" form, submitted only after autofill supplies it. */
export function passwordConfirmationScript() {
  return `(() => {
    const form = Array.from(document.forms).find((candidate) => {
      const button = candidate.querySelector('button, input[type="submit"]');
      return button && /confirm password/i.test((button.textContent || button.value || '').trim());
    });
    if (!form) return { present: false, filled: false, submitted: false };
    const password = form.querySelector('input[type="password"]');
    const filled = Boolean(password?.value && !password.disabled);
    if (!filled) return { present: true, filled: false, submitted: false };
    const button = Array.from(form.querySelectorAll('button, input[type="submit"]')).find((candidate) => /confirm password/i.test((candidate.textContent || candidate.value || '').trim()));
    if (!button || button.disabled) return { present: true, filled: true, submitted: false };
    let submittedBefore = false;
    try { submittedBefore = sessionStorage.getItem('prm-password-confirmed') === '1'; } catch {}
    if (submittedBefore) return { present: true, filled: true, submitted: false, alreadySubmitted: true };
    try { sessionStorage.setItem('prm-password-confirmed', '1'); } catch {}
    button.click();
    return { present: true, filled: true, submitted: true };
  })()`;
}

/** Map a browser channel to the macOS application that owns the autofill popup. */
export function browserApplication(channel = "chrome") {
  const value = String(channel).toLowerCase();
  if (value.includes("edge")) {
    return "Microsoft Edge";
  }
  if (value.includes("brave")) {
    return "Brave Browser";
  }
  if (value.includes("chromium")) {
    return "Chromium";
  }
  if (value.includes("firefox")) {
    return "Firefox";
  }
  return "Google Chrome";
}

/**
 * Accept Chrome's saved-password suggestion with a real OS key press. Playwright
 * key events go to the page, but the autofill popup belongs to the browser UI,
 * so it needs System Events. The credential value is never read or logged.
 */
export async function pressPasswordAutofillKeys({
  app = "Google Chrome",
  pid,
  platform = process.platform,
} = {}) {
  if (platform !== "darwin") {
    return false;
  }
  const activate = Number.isInteger(pid)
    ? `tell application "System Events" to set frontmost of first process whose unix id is ${pid} to true`
    : `tell application ${JSON.stringify(app)} to activate`;
  try {
    await run("osascript", [
      "-e",
      activate,
      "-e",
      'tell application "System Events" to key code 125',
      "-e",
      'tell application "System Events" to key code 36',
    ]);
    return true;
  } catch {
    return false;
  }
}

/**
 * Focus the empty password field, open the browser's saved-password suggestion,
 * and select it with the OS keyboard. Only `field.value` presence is inspected.
 */
export async function acceptPasswordAutofill(
  page,
  {
    click,
    pressKeys,
    app = "Google Chrome",
    pid,
    log = () => {},
    state = {},
    timeoutMs = 5_000,
    pollIntervalMs = 250,
  } = {},
) {
  if (typeof page?.evaluate !== "function") {
    return false;
  }
  const before = await page.evaluate(passwordPromptScript()).catch(() => null);
  if (!before?.present || before.filled) {
    return Boolean(before?.filled);
  }
  if (state.autofillAttempted) {
    return false;
  }
  state.autofillAttempted = true;
  const clickField =
    click ??
    (typeof page.click === "function"
      ? () => page.click('input[type="password"]')
      : null);
  if (clickField) {
    await clickField().catch(() => {});
  }
  await new Promise((resolve) => setTimeout(resolve, 750));
  const press = pressKeys ?? (() => pressPasswordAutofillKeys({ app, pid }));
  if (!(await press())) {
    return false;
  }
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const stateAfter = await page
      .evaluate(passwordPromptScript())
      .catch(() => null);
    if (stateAfter?.filled) {
      log("  Selected the browser's saved password.");
      return true;
    }
    if (Date.now() >= deadline) {
      return false;
    }
    await new Promise((resolve) => setTimeout(resolve, pollIntervalMs));
  }
}

/** Fill PyPI's confirm-password page from the browser and submit it. */
export async function submitPasswordConfirmation(page, options = {}) {
  if (typeof page?.evaluate !== "function") {
    return undefined;
  }
  const { log = () => {} } = options;
  let state = await page
    .evaluate(passwordConfirmationScript())
    .catch(() => null);
  if (!state?.present) {
    return state;
  }
  if (state.failed) {
    throw new Error(
      "PyPI rejected the saved password; update it in the automated browser profile and retry",
    );
  }
  if (!state.filled) {
    const filled = await acceptPasswordAutofill(page, options);
    if (!filled) {
      if (!options.state?.waitingLogged) {
        if (options.state) {
          options.state.waitingLogged = true;
        }
        log(
          "  Confirm-password page is open; waiting for the browser's saved password.",
        );
      }
      return state;
    }
  }
  state = await page.evaluate(passwordConfirmationScript()).catch(() => null);
  if (state?.submitted) {
    log("  Confirmed the saved password for this sensitive action.");
  }
  return state;
}

/** Every page in the active browser, including tabs opened by the site. */
export function browserPages(page) {
  if (typeof page?.pages === "function") {
    try {
      const pages = page.pages();
      if (Array.isArray(pages) && pages.length > 0) {
        return pages;
      }
    } catch {
      // Fall back to the current page.
    }
  }
  try {
    const pages = page?.context?.()?.pages?.();
    if (Array.isArray(pages) && pages.length > 0) {
      return pages;
    }
  } catch {
    // The extension relay has no context, only the active tab.
  }
  return page ? [page] : [];
}

/**
 * Remember the current device on every 2FA page and notice the confirmation
 * page, even when it was opened in another tab. Logging is per state object so
 * the polling caller does not repeat the same message.
 */
export async function prepareLoginDevice(
  page,
  { log = () => {}, state = {} } = {},
) {
  for (const candidate of browserPages(page)) {
    if (typeof candidate?.evaluate !== "function") {
      continue;
    }
    const working = isSamePage(candidate, page);
    const blank =
      typeof candidate.url === "function" && candidate.url() === "about:blank";
    if (!working && blank) {
      await candidate.close?.().catch(() => {});
      continue;
    }
    const result = await candidate
      .evaluate(loginDeviceScript())
      .catch(() => null);
    if (!result) {
      continue;
    }
    if (result.remembered > 0 && !state.remembered) {
      state.remembered = true;
      log('  Checked "Remember this device for 30 days".');
    }
    if (result.confirmed && !state.confirmed) {
      state.confirmed = true;
      log("  This device is now confirmed for the PyPI login.");
    }
    if (!working && (result.authSuccessful || result.confirmed)) {
      await candidate.close?.().catch(() => {});
    }
  }
  return state;
}

/**
 * Submit a prefilled sign-in form, waiting for browser autofill and for the
 * site to enable its submit button after the autofilled values are announced.
 * Only page state is inspected; credentials are never read or returned.
 */
export async function submitPrefilledSignIn(
  page,
  {
    timeoutMs = 15_000,
    pollIntervalMs = 250,
    formGraceMs = 3_000,
    autofillDelayMs = 3_000,
    twoFactorGraceMs = 2_000,
    pressKeys,
    click,
    app,
    pid,
    state = {},
    log = () => {},
  } = {},
) {
  if (typeof page?.evaluate !== "function") {
    return undefined;
  }
  const deadline = Date.now() + timeoutMs;
  const formDeadline = Date.now() + formGraceMs;
  let sawForm = false;
  let formSeenAt = 0;
  let announcedPrefill = false;
  let attemptedAutofill = false;
  let retried = false;
  for (;;) {
    const signIn =
      (await page.evaluate(signInScript()).catch(() => ({}))) ?? {};
    if (signIn.present && !formSeenAt) {
      formSeenAt = Date.now();
    }
    sawForm ||= Boolean(signIn.present);
    await prepareLoginDevice(page, { log, state });
    if (signIn.submitted) {
      log(
        "  Submitted the prefilled sign-in form; complete passkey or 2FA if prompted.",
      );
      const twoFactorDeadline = Date.now() + twoFactorGraceMs;
      for (;;) {
        if (state.remembered || state.confirmed) {
          return signIn;
        }
        if (Date.now() >= twoFactorDeadline) {
          return signIn;
        }
        await new Promise((resolve) =>
          setTimeout(resolve, Math.min(pollIntervalMs, 250)),
        );
        await prepareLoginDevice(page, { log, state });
      }
    }
    if (signIn.present && !signIn.prefilled && !announcedPrefill) {
      announcedPrefill = true;
      log(
        "  The sign-in form is open; waiting for the browser to prefill the username and password.",
      );
    }
    if (
      signIn.present &&
      !signIn.prefilled &&
      !attemptedAutofill &&
      formSeenAt &&
      Date.now() - formSeenAt >= autofillDelayMs
    ) {
      attemptedAutofill = true;
      await acceptPasswordAutofill(page, {
        click,
        pressKeys,
        app,
        pid,
        state,
        log,
        pollIntervalMs,
      });
    }
    if (signIn.failed && signIn.alreadySubmitted && !retried) {
      retried = true;
      await page
        .evaluate(() => {
          try {
            globalThis.sessionStorage.removeItem("prm-sign-in-submitted");
          } catch {
            // The marker only prevents a duplicate retry.
          }
        })
        .catch(() => {});
      log("  Sign-in failed; retrying once with the prefilled credentials.");
      continue;
    }
    if (Date.now() >= deadline || (!sawForm && Date.now() >= formDeadline)) {
      return signIn;
    }
    await new Promise((resolve) => setTimeout(resolve, pollIntervalMs));
  }
}

/** Verify the server-rendered publisher row, and surface form validation errors. */
export function publisherConfirmationScript(config, pending = true) {
  return `(() => {
    const config = ${JSON.stringify(config)};
    const pending = ${JSON.stringify(pending)};
    const errors = Array.from(document.querySelectorAll('#pending-github-publisher-form .form-errors, #pending-github-publisher-form .form-group__error, #github-publisher-form .form-errors, #github-publisher-form .form-group__error, .error-message, .notification-bar--danger')).map(node => node.textContent.trim()).filter(Boolean);
    const rows = Array.from(document.querySelectorAll('tr, .publisher, .card'));
    const confirmed = rows.some(row => {
      const repository = Array.from(row.querySelectorAll('a[href]')).some(link => {
        try { const url = new URL(link.href); return url.hostname === 'github.com' && (url.pathname === '/' + config.organization + '/' + config.repository || url.pathname.startsWith('/' + config.organization + '/' + config.repository + '/')); } catch { return false; }
      });
      return repository && row.querySelector('td')?.textContent.trim() === (pending ? config.project : 'GitHub')
        && [config.workflow, config.environment].filter(Boolean).every(value => row.innerText.split(/\\s+/).includes(value));
    });
    return { confirmed, error: errors.join('; ') };
  })()`;
}

/** Wait through sign-in, fill and verify a trusted-publisher form. */
export async function fillPublisherForm(
  session,
  pending = session.plan.mode !== "attach",
) {
  const page = session.automation;
  const script = npmPrefillScript(session.plan.trusted_publisher, false);
  const deadline = Date.now() + (session.options.waitTimeoutMs ?? 10 * 60_000);
  console.log(
    "  Waiting for the trusted-publisher form; a prefilled sign-in form is submitted automatically.",
  );
  let announcedPrefill = false;
  let announcedSubmit = false;
  let announcedRetry = false;
  let announcedConfirmLogin = false;
  let signInRetries = 0;
  const loginDevice = {};
  const passwordConfirmation = {};
  for (;;) {
    await prepareLoginDevice(page, { log: console.log, state: loginDevice });
    if (session.plan.registry === "pypi") {
      await submitPasswordConfirmation(page, {
        log: console.log,
        state: passwordConfirmation,
        app: browserApplication(session.options.browserOptions?.channel),
        pid: session.automation.pid?.(),
      });
      const currentUrl = typeof page.url === "function" ? page.url() : "";
      if (
        currentUrl.includes("/account/confirm-login/") &&
        !announcedConfirmLogin
      ) {
        announcedConfirmLogin = true;
        console.log(
          "  PyPI needs email confirmation for this device; open the link from noreply@pypi.org in the automated Chrome window.",
        );
      }
      const signIn = await page.evaluate(signInScript()).catch(() => ({}));
      if (signIn.present && !signIn.prefilled && !announcedPrefill) {
        announcedPrefill = true;
        console.log(
          "  The sign-in form is open; waiting for the browser to prefill the username and password.",
        );
      }
      if (signIn.submitted && !announcedSubmit) {
        announcedSubmit = true;
        console.log(
          "  Submitted the prefilled sign-in form; complete passkey or 2FA if prompted.",
        );
      }
      if (signIn.alreadySubmitted && !announcedRetry) {
        announcedRetry = true;
        console.log(
          "  The sign-in form was submitted once already; if it failed, correct it in the browser.",
        );
      }
      if (signIn.failed && signIn.alreadySubmitted && signInRetries < 1) {
        signInRetries += 1;
        await page
          .evaluate(() => {
            try {
              globalThis.sessionStorage.removeItem("prm-sign-in-submitted");
            } catch {
              // The marker only prevents a duplicate retry.
            }
          })
          .catch(() => {});
        console.log(
          "  Sign-in failed; retrying once with the prefilled credentials.",
        );
        continue;
      }
      if (signIn.error && !announcedRetry) {
        announcedRetry = true;
        console.log(`  Could not submit the sign-in form: ${signIn.error}.`);
      }
    }
    const result = await page.evaluate(script).catch(() => ({}));
    const required = [
      "organization",
      "repository",
      "workflow",
      ...(session.plan.registry === "pypi" && pending ? ["project"] : []),
    ];
    if (required.every((field) => result.filled?.includes(field))) {
      break;
    }
    if (Date.now() >= deadline) {
      throw new Error(
        "timed out waiting for sign-in and the trusted-publisher form; nothing submitted",
      );
    }
    await new Promise((resolve) =>
      setTimeout(resolve, session.options.pollIntervalMs ?? 500),
    );
  }
  const submit =
    session.plan.registry === "pypi" ||
    session.options.yes ||
    /^(?:y|yes)$/i.test(
      await session.prompt(
        "Submit this trusted-publisher configuration? [y/N] ",
      ),
    );
  if (!submit) {
    throw new Error(
      "trusted-publisher submission declined; release not started",
    );
  }
  if (submit) {
    const submission = await page.evaluate(
      npmPrefillScript(session.plan.trusted_publisher, true),
    );
    if (!submission.submitted) {
      throw new Error(
        "the form was not submitted; review the visible browser and submit manually",
      );
    }
    if (session.plan.registry === "pypi") {
      for (;;) {
        const state = await page
          .evaluate(
            publisherConfirmationScript(
              session.plan.trusted_publisher,
              pending,
            ),
          )
          .catch(() => ({}));
        if (state.error) {
          throw new Error(`PyPI refused the pending publisher: ${state.error}`);
        }
        if (state.confirmed) {
          session.conditions.delete("trust-missing");
          break;
        }
        if (Date.now() >= deadline) {
          throw new Error(
            "timed out verifying the PyPI pending publisher; release not started",
          );
        }
        await new Promise((resolve) =>
          setTimeout(resolve, session.options.pollIntervalMs ?? 500),
        );
      }
    }
  }
}

/** npm's security-key page: opt in to the 5-minute cooldown and press the key button. */
export function npmChallengeScript(mode = "") {
  const clickBox = mode === "optin" || mode === "both";
  const clickButton = mode === "button" || mode === "both";
  return `(() => {
    const box = document.querySelector('#cooldownOptin_didOptForCooldown');
    const button = Array.from(document.querySelectorAll('button')).find((candidate) =>
      /^(?:use|authenticate with|continue with)\\s+security key$/i.test(candidate.innerText.trim())
    );
    const text = document.body?.innerText ?? '';
    const challenge = /two-factor authentication|security key|use touch id/i.test(text);
    let opted = Boolean(box?.checked);
    let clicked = false;
    if (${JSON.stringify(clickBox)} && box && !box.checked) {
      box.click();
      box.dispatchEvent(new Event('input', { bubbles: true }));
      box.dispatchEvent(new Event('change', { bubbles: true }));
      opted = true;
    }
    if (${JSON.stringify(clickButton)} && button && !button.disabled) {
      button.click();
      clicked = true;
    }
    return { present: Boolean(box || button), challenge, opted, button: Boolean(button), clicked };
  })()`;
}

/**
 * Approve npm's `npm publish`/`npm trust` security-key page in the automated
 * browser: check the 5-minute cooldown box and click "Use security key". Touch
 * ID or the physical key still has to be confirmed by the person.
 */
export async function approveNpmChallenge(
  page,
  { timeoutMs = 20_000, pollIntervalMs = 500, log = () => {} } = {},
) {
  if (typeof page?.evaluate !== "function") {
    return undefined;
  }
  const deadline = Date.now() + timeoutMs;
  let announced = false;
  for (;;) {
    const state = await page.evaluate(npmChallengeScript()).catch(() => null);
    if (state?.challenge && !announced) {
      announced = true;
      log(
        '  npm is asking for a security key; opting in to the 5-minute cooldown and clicking "Use security key".',
      );
    }
    if (state?.present && state?.challenge) {
      if (!state.opted) {
        await page.evaluate(npmChallengeScript("optin")).catch(() => null);
        await new Promise((resolve) => setTimeout(resolve, 500));
      }
      const clicked = await page
        .evaluate(npmChallengeScript("button"))
        .catch(() => null);
      if (clicked?.clicked) {
        log(
          '  Clicked "Use security key"; approve with Touch ID or the security key in the browser.',
        );
        return clicked;
      }
    }
    if (Date.now() >= deadline) {
      return state;
    }
    await new Promise((resolve) => setTimeout(resolve, pollIntervalMs));
  }
}
