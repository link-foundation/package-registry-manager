import assert from "node:assert/strict";
import test from "node:test";

import {
  acceptPasswordAutofill,
  approveNpmChallenge,
  browserApplication,
  npmChallengeScript,
  prepareLoginDevice,
  submitPasswordConfirmation,
  submitPrefilledSignIn,
} from "../src/publisher-form.mjs";

test("waits for autofill, then submits once the site enables its button", async () => {
  const states = [
    { present: true, prefilled: true, submitted: false, pendingButton: true },
    { present: true, prefilled: true, submitted: true },
  ];
  const page = {
    evaluate: async (script) =>
      script.includes("your login has been confirmed")
        ? { present: false, checked: false, remembered: 0, confirmed: false }
        : states.shift(),
  };
  const messages = [];
  const result = await submitPrefilledSignIn(page, {
    timeoutMs: 1000,
    pollIntervalMs: 1,
    twoFactorGraceMs: 1,
    log: (message) => messages.push(message),
  });
  assert.equal(result.submitted, true);
  assert.match(messages.join("\n"), /Submitted the prefilled sign-in form/);
});

test("does not submit until the browser prefills credentials", async () => {
  const states = [
    { present: true, prefilled: false, submitted: false },
    { present: true, prefilled: true, submitted: false, pendingButton: true },
    { present: true, prefilled: true, submitted: true },
  ];
  const page = {
    evaluate: async (script) =>
      script.includes("your login has been confirmed")
        ? { present: false, checked: false, remembered: 0, confirmed: false }
        : states.shift(),
  };
  const messages = [];
  const result = await submitPrefilledSignIn(page, {
    timeoutMs: 1000,
    pollIntervalMs: 1,
    twoFactorGraceMs: 1,
    log: (message) => messages.push(message),
  });
  assert.equal(result.submitted, true);
  assert.match(messages.join("\n"), /waiting for the browser to prefill/);
});

test("returns quickly when no sign-in form appears", async () => {
  const page = {
    evaluate: async () => ({
      present: false,
      prefilled: false,
      submitted: false,
    }),
  };
  const started = Date.now();
  const result = await submitPrefilledSignIn(page, {
    timeoutMs: 1000,
    formGraceMs: 1,
    pollIntervalMs: 1,
    log: () => {},
  });
  assert.equal(result.submitted, false);
  assert.ok(Date.now() - started < 500);
});

test("retries a failed prefilled sign-in once", async () => {
  const states = [
    {
      present: true,
      prefilled: true,
      submitted: false,
      alreadySubmitted: true,
      failed: true,
    },
    { present: true, prefilled: true, submitted: true },
  ];
  let cleared = false;
  const page = {
    evaluate: async (script) => {
      if (typeof script === "function") {
        cleared = true;
        return undefined;
      }
      if (script.includes("your login has been confirmed")) {
        return {
          present: false,
          checked: false,
          remembered: 0,
          confirmed: false,
        };
      }
      return states.shift();
    },
  };
  const messages = [];
  const result = await submitPrefilledSignIn(page, {
    timeoutMs: 1000,
    pollIntervalMs: 1,
    twoFactorGraceMs: 1,
    log: (message) => messages.push(message),
  });
  assert.equal(result.submitted, true);
  assert.equal(cleared, true);
  assert.match(messages.join("\n"), /retrying once/);
});

test("checks remember-device when 2FA appears after sign-in is submitted", async () => {
  let submitted = false;
  let signInCalls = 0;
  let deviceChecksAfterSubmit = 0;
  const page = {
    evaluate: async (script) => {
      if (script.includes("your login has been confirmed")) {
        if (submitted) {
          deviceChecksAfterSubmit += 1;
          return {
            present: true,
            checked: false,
            remembered: 1,
            confirmed: false,
          };
        }
        return {
          present: false,
          checked: false,
          remembered: 0,
          confirmed: false,
        };
      }
      signInCalls += 1;
      if (signInCalls === 1) {
        return {
          present: true,
          prefilled: true,
          submitted: false,
          pendingButton: true,
        };
      }
      submitted = true;
      return { present: true, prefilled: true, submitted: true };
    },
    pages: () => [],
  };
  const messages = [];
  const result = await submitPrefilledSignIn(page, {
    timeoutMs: 1000,
    pollIntervalMs: 1,
    twoFactorGraceMs: 100,
    log: (message) => messages.push(message),
  });
  assert.equal(result.submitted, true);
  assert.ok(deviceChecksAfterSubmit > 0);
  assert.match(messages.join("\n"), /Remember this device/);
});

test("checks remember-device boxes and finds confirmation in another tab", async () => {
  const states = {
    twoFactor: {
      present: true,
      checked: false,
      remembered: 1,
      confirmed: false,
    },
    confirmed: {
      present: false,
      checked: false,
      remembered: 0,
      confirmed: true,
    },
  };
  const twoFactor = { evaluate: async () => states.twoFactor };
  const confirmed = { evaluate: async () => states.confirmed };
  const messages = [];
  const state = {};
  await prepareLoginDevice(
    { pages: () => [twoFactor, confirmed] },
    { state, log: (message) => messages.push(message) },
  );
  assert.equal(state.remembered, true);
  assert.equal(state.confirmed, true);
  assert.match(messages.join("\n"), /Remember this device/);
  assert.match(messages.join("\n"), /now confirmed/);
});

test("does not repeat login-device messages while polling", async () => {
  const page = {
    evaluate: async () => ({
      present: true,
      checked: true,
      remembered: 0,
      confirmed: true,
    }),
  };
  const messages = [];
  const state = {};
  await prepareLoginDevice(page, {
    state,
    log: (message) => messages.push(message),
  });
  await prepareLoginDevice(page, {
    state,
    log: (message) => messages.push(message),
  });
  assert.equal(messages.length, 1);
});

test("accepts the browser's saved password without reading it", async () => {
  let state = { present: true, filled: false };
  let clicked = 0;
  let pressed = 0;
  const page = {
    evaluate: async () => state,
    click: async () => {
      clicked += 1;
    },
  };
  const filled = await acceptPasswordAutofill(page, {
    pressKeys: async () => {
      pressed += 1;
      state = { present: true, filled: true };
      return true;
    },
    timeoutMs: 1000,
    pollIntervalMs: 1,
  });
  assert.equal(filled, true);
  assert.equal(clicked, 1);
  assert.equal(pressed, 1);
});

test("does not press keys when the password field is already filled", async () => {
  const page = { evaluate: async () => ({ present: true, filled: true }) };
  let pressed = 0;
  const filled = await acceptPasswordAutofill(page, {
    pressKeys: async () => {
      pressed += 1;
      return true;
    },
  });
  assert.equal(filled, true);
  assert.equal(pressed, 0);
});

test("confirms the saved password on PyPI's sensitive-action page", async () => {
  let prompt = { present: true, filled: false };
  const confirmation = [
    { present: true, filled: false, submitted: false },
    { present: true, filled: true, submitted: true },
  ];
  let pressed = 0;
  const page = {
    evaluate: async (script) =>
      script.includes("candidate.offsetParent !== null")
        ? prompt
        : confirmation.shift(),
    click: async () => {},
  };
  const messages = [];
  const result = await submitPasswordConfirmation(page, {
    state: {},
    log: (message) => messages.push(message),
    pressKeys: async () => {
      pressed += 1;
      prompt = { present: true, filled: true };
      return true;
    },
    timeoutMs: 1000,
    pollIntervalMs: 1,
  });
  assert.equal(pressed, 1);
  assert.equal(result.submitted, true);
  assert.match(messages.join("\n"), /Confirmed the saved password/);
});

test("maps browser channels to their macOS application", () => {
  assert.equal(browserApplication("chrome"), "Google Chrome");
  assert.equal(browserApplication("msedge"), "Microsoft Edge");
  assert.equal(browserApplication("brave"), "Brave Browser");
  assert.equal(browserApplication("chromium"), "Chromium");
});

test("closes extra authentication-success and blank tabs", async () => {
  const closed = [];
  const makePage = (result, url = "https://example.test/") => ({
    url: () => url,
    evaluate: async () => result,
    close: async () => closed.push(url),
  });
  const working = makePage({ present: false, authSuccessful: false });
  const success = makePage({ present: false, authSuccessful: true });
  const confirmed = makePage({ present: false, confirmed: true });
  const blank = makePage({ present: false }, "about:blank");
  const state = {};
  await prepareLoginDevice(
    { pages: () => [working, success, confirmed, blank] },
    { state, log: () => {} },
  );
  assert.equal(state.confirmed, true);
  assert.deepEqual(closed, [
    "https://example.test/",
    "https://example.test/",
    "about:blank",
  ]);
});

test("keeps the working page even when it shows authentication success", async () => {
  const closed = [];
  const working = {
    url: () => "https://www.npmjs.com/auth/cli/example",
    evaluate: async () => ({ present: false, authSuccessful: true }),
    close: async () => closed.push("working"),
  };
  working.pages = () => [working];
  await prepareLoginDevice(working, { state: {}, log: () => {} });
  assert.deepEqual(closed, []);
});

test("does not close a working-page wrapper showing authentication success", async () => {
  const closed = [];
  const actual = {
    url: () => "https://www.npmjs.com/auth/cli/example",
    evaluate: async () => ({ present: false, authSuccessful: true }),
    close: async () => closed.push("actual"),
  };
  actual.pages = () => [actual];
  const wrapper = {
    url: () => "https://www.npmjs.com/auth/cli/example",
    pages: () => [actual],
  };
  await prepareLoginDevice(wrapper, { state: {}, log: () => {} });
  assert.deepEqual(closed, []);
});

test("automatically opts in and clicks npm's security key button", async () => {
  let calls = 0;
  const page = {
    evaluate: async () => {
      calls += 1;
      if (calls === 1) {
        return {
          present: true,
          challenge: true,
          opted: false,
          button: true,
          clicked: false,
        };
      }
      if (calls === 2) {
        return {
          present: true,
          challenge: true,
          opted: true,
          button: true,
          clicked: false,
        };
      }
      return {
        present: true,
        challenge: true,
        opted: true,
        button: true,
        clicked: true,
      };
    },
  };
  const messages = [];
  const result = await approveNpmChallenge(page, {
    timeoutMs: 100,
    pollIntervalMs: 1,
    log: (message) => messages.push(message),
  });
  assert.equal(result.clicked, true);
  assert.equal(calls, 3);
  assert.ok(
    messages.some((message) => /Clicked "Use security key"/.test(message)),
  );
});

test("npm challenge script names the cooldown checkbox and key button", () => {
  const script = npmChallengeScript("both");
  assert.match(script, /cooldownOptin_didOptForCooldown/);
  assert.match(script, /security key/i);
  assert.match(script, /button\.click\(\)/);
});
