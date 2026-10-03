import assert from "node:assert/strict";
import { test } from "node:test";

import {
  APPROVAL_ATTEMPTS,
  approvalDeadline,
  expiredApproval,
  oidcReleaseNote,
  withFreshLinks,
} from "../src/approvals.mjs";
import { authUrlScanner } from "../src/auth-urls.mjs";
import {
  browserName,
  defaultBrowserId,
  defaultBrowserQuery,
  detectDefaultBrowser,
  openWithCommand,
} from "../src/default-browser.mjs";

test("prints when a sign-in or approval link expires", () => {
  const now = new Date(2026, 9, 3, 18, 58);
  assert.equal(
    approvalDeadline("login", now),
    "Sign in within about 5 minutes (until 19:03).",
  );
  assert.equal(
    approvalDeadline("approve", now),
    "Approve within about 5 minutes (until 19:03).",
  );
});

test("recognizes the ways an npm link expires", () => {
  assert.equal(
    expiredApproval({ code: null, legacyLogin: true }),
    "npm fell back to its legacy username prompt",
  );
  assert.equal(
    expiredApproval({ code: 1, approvalExpired: true }),
    "npm's approval session ended",
  );
  assert.equal(expiredApproval({ code: 0, approvalExpired: true }), undefined);
  assert.equal(expiredApproval({ code: 1 }), undefined);
});

test("requests fresh links a bounded number of times", async () => {
  const messages = [];
  const results = [{ code: null, legacyLogin: true }, { code: 0 }];
  let calls = 0;
  const result = await withFreshLinks(async () => results[calls++], {
    log: (message) => messages.push(message),
  });
  assert.deepEqual(result, { code: 0 });
  assert.equal(calls, 2);
  assert.match(messages[0], /requesting a fresh one \(attempt 2 of 3\)/);

  calls = 0;
  await assert.rejects(
    withFreshLinks(
      async () => {
        calls += 1;
        return { code: 1, approvalExpired: true };
      },
      { log: () => {} },
    ),
    /the browser link expired 3 times \(npm's approval session ended\)/,
  );
  assert.equal(calls, APPROVAL_ATTEMPTS);
  assert.equal(
    oidcReleaseNote("release.yml"),
    "Future releases publish from release.yml through trusted publishing; no login is needed.",
  );
});

test("tells sign-in links from approval links", () => {
  const found = [];
  const scan = authUrlScanner((url, kind) => found.push([url, kind]));
  scan("Login at:\nhttps://www.npmjs.com/login?next=/login/cli/1\n");
  scan("Authenticate your account at:\nhttps://www.npmjs.com/auth/cli/2\n");
  assert.deepEqual(found, [
    ["https://www.npmjs.com/login?next=/login/cli/1", "login"],
    ["https://www.npmjs.com/auth/cli/2", "approve"],
  ]);
});

test("names the default browser on every platform", async () => {
  const mac = `(
    {
        LSHandlerContentType = "public.html";
        LSHandlerRoleAll = "com.google.chrome";
    },
    {
        LSHandlerPreferredVersions = { LSHandlerRoleAll = "-"; };
        LSHandlerRoleAll = "com.brave.browser";
        LSHandlerURLScheme = https;
    }
)`;
  assert.equal(defaultBrowserId(mac, "darwin"), "com.brave.browser");
  assert.equal(defaultBrowserId("(\n)", "darwin"), "com.apple.safari");
  assert.equal(
    defaultBrowserId(
      "\r\nHKEY_CURRENT_USER\\...\\UserChoice\r\n    ProgId    REG_SZ    MSEdgeHTM\r\n",
      "win32",
    ),
    "MSEdgeHTM",
  );
  assert.equal(
    defaultBrowserId("firefox.desktop\n", "linux"),
    "firefox.desktop",
  );
  assert.equal(defaultBrowserId("", "linux"), undefined);
  assert.equal(browserName("com.brave.browser"), "Brave");
  assert.equal(browserName("MSEdgeHTM"), "Microsoft Edge");
  assert.equal(browserName("google-chrome.desktop"), "Google Chrome");
  assert.equal(browserName("unknown.desktop"), undefined);
  assert.deepEqual(defaultBrowserQuery("linux"), {
    program: "xdg-settings",
    args: ["get", "default-web-browser"],
  });
  assert.equal(defaultBrowserQuery("darwin").program, "defaults");
  assert.equal(defaultBrowserQuery("win32").program, "reg");

  const answer = (code, stdout) => async () => ({ code, stdout });
  assert.equal(
    await detectDefaultBrowser({
      platform: "darwin",
      run: answer(1, ""),
    }),
    "Safari",
  );
  assert.equal(
    await detectDefaultBrowser({ platform: "linux", run: answer(1, "") }),
    undefined,
  );
  assert.equal(
    await detectDefaultBrowser({
      platform: "linux",
      run: async () => {
        throw new Error("xdg-settings: not found");
      },
    }),
    undefined,
  );
});

test("opens a link in a chosen application without a shell", () => {
  const url = "https://www.npmjs.com/auth/cli/2";
  assert.deepEqual(openWithCommand(url, "Firefox", "darwin"), {
    program: "open",
    args: ["-a", "Firefox", url],
  });
  assert.deepEqual(openWithCommand(url, "firefox", "linux"), {
    program: "firefox",
    args: [url],
  });
  assert.throws(
    () => openWithCommand("file:///etc/passwd", "firefox", "linux"),
    /non-web URL/,
  );
});
