import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";

import { runInteractive } from "../src/auth-urls.mjs";
import {
  detachedRunner,
  openerSucceeded,
  openInUserBrowser,
} from "../src/browser.mjs";
import {
  defaultBrowserProfile,
  ensureProfileIgnored,
  legacyBrowserProfile,
  protectLegacyProfile,
} from "../src/profile.mjs";

let temporary;

before(async () => {
  temporary = await mkdtemp(path.join(os.tmpdir(), "prm-profile-"));
});

after(async () => {
  await rm(temporary, { recursive: true, force: true });
});

const git = (cwd, ...args) =>
  execFileSync("git", args, { cwd, encoding: "utf8" });

const isIgnored = (cwd, file) => {
  try {
    git(cwd, "check-ignore", "--quiet", "--", file);
    return true;
  } catch {
    return false;
  }
};

async function repository(name) {
  const root = path.join(temporary, name);
  await mkdir(root, { recursive: true });
  git(root, "init", "--quiet");
  return root;
}

test("defaults to a per-user profile outside any repository", () => {
  assert.equal(
    defaultBrowserProfile({ platform: "darwin", env: {}, home: "/Users/me" }),
    "/Users/me/Library/Application Support/package-registry-manager/browser-profile",
  );
  assert.equal(
    defaultBrowserProfile({ platform: "linux", env: {}, home: "/home/me" }),
    "/home/me/.local/state/package-registry-manager/browser-profile",
  );
  assert.equal(
    defaultBrowserProfile({
      platform: "linux",
      env: { XDG_STATE_HOME: "/state" },
      home: "/home/me",
    }),
    "/state/package-registry-manager/browser-profile",
  );
  assert.equal(
    defaultBrowserProfile({
      platform: "linux",
      env: { XDG_STATE_HOME: "relative" },
      home: "/home/me",
    }),
    "/home/me/.local/state/package-registry-manager/browser-profile",
  );
  assert.equal(
    defaultBrowserProfile({
      platform: "win32",
      env: { LOCALAPPDATA: "C:\\Users\\me\\AppData\\Local" },
      home: "C:\\Users\\me",
    }),
    "C:\\Users\\me\\AppData\\Local\\package-registry-manager\\browser-profile",
  );

  const repo = path.resolve(temporary, "any-repository");
  const relative = path.relative(repo, defaultBrowserProfile());
  assert.ok(
    relative.startsWith("..") || path.isAbsolute(relative),
    `${defaultBrowserProfile()} is outside ${repo}`,
  );
});

test("ignores a profile inside the repository before first use", async () => {
  const root = await repository("in-repository");
  const profile = legacyBrowserProfile(root);
  const cookies = ".package-registry-manager/browser-profile/Default/Cookies";
  assert.equal(isIgnored(root, cookies), false, "reproduces issue #8");

  await ensureProfileIgnored(profile);

  assert.equal(
    await readFile(path.join(root, ".package-registry-manager/.gitignore"), {
      encoding: "utf8",
    }),
    "*\n",
  );
  assert.equal(isIgnored(root, cookies), true);
  await mkdir(path.join(profile, "Default"), { recursive: true });
  await writeFile(path.join(root, cookies), "session");
  git(root, "add", "-A");
  assert.equal(git(root, "diff", "--cached", "--name-only"), "");
});

test("ignores an explicit profile anywhere in a work tree", async () => {
  const root = await repository("explicit");
  const profile = path.join(root, "work", "profile");
  await ensureProfileIgnored(profile);
  assert.equal(
    await readFile(path.join(profile, ".gitignore"), "utf8"),
    "*\n",
    "the ignore file is local to the profile, not to its parent",
  );
  assert.equal(isIgnored(root, "work/profile/Default/Cookies"), true);
  assert.equal(isIgnored(root, "work/other"), false);
});

test("refuses a profile whose files Git already tracks", async () => {
  const root = await repository("tracked");
  const profile = legacyBrowserProfile(root);
  await mkdir(profile, { recursive: true });
  await writeFile(path.join(profile, "Local State"), "{}");
  git(root, "add", "-A");
  await assert.rejects(
    ensureProfileIgnored(profile),
    /inside the Git work tree .* Git does not ignore it/,
  );
});

test("leaves a profile outside any work tree alone", async () => {
  const profile = path.join(temporary, "outside", "profile");
  await ensureProfileIgnored(profile);
  await assert.rejects(readFile(path.join(profile, ".gitignore")));
});

test("protects and reports a profile left in the repository", async () => {
  const root = await repository("legacy");
  await mkdir(legacyBrowserProfile(root), { recursive: true });
  const warnings = [];
  const original = console.error;
  console.error = (message) => warnings.push(message);
  try {
    await protectLegacyProfile(root, path.join(temporary, "new-profile"));
  } finally {
    console.error = original;
  }
  assert.equal(
    isIgnored(root, ".package-registry-manager/browser-profile/x"),
    true,
  );
  assert.match(warnings.join("\n"), /is no longer used .* delete it/);
});

test("opens URLs in the default browser without a shell", async () => {
  const url = "https://www.npmjs.com/login?next=/login/cli/1&a=b";
  const opener = async (platform) => {
    const calls = [];
    await openInUserBrowser(url, {
      platform,
      runner: async (file, args) => calls.push([file, ...args]),
    });
    return calls;
  };
  assert.deepEqual(await opener("darwin"), [["open", url]]);
  assert.deepEqual(await opener("linux"), [["xdg-open", url]]);
  assert.deepEqual(await opener("freebsd"), [["xdg-open", url]]);
  assert.deepEqual(await opener("win32"), [["explorer.exe", url]]);
});

test("refuses to open non-web URLs", async () => {
  const runner = () => assert.fail("the opener must not run");
  for (const hostile of [
    "--help",
    "file:///etc/passwd",
    "https://a b",
    "https://",
    "javascript:alert(1)",
  ]) {
    await assert.rejects(
      openInUserBrowser(hostile, { platform: "linux", runner }),
      /non-web URL/,
      hostile,
    );
  }
});

test("treats explorer exit code one as handed over", () => {
  assert.equal(openerSucceeded("explorer.exe", 1), true);
  assert.equal(openerSucceeded("explorer.exe", 2), false);
  assert.equal(openerSucceeded("xdg-open", 1), false);
  assert.equal(openerSucceeded("xdg-open", 0), true);
});

test("waits for an opener only for a grace period", async () => {
  const node = (script) => ["-e", script];
  const runner = detachedRunner(300);
  assert.deepEqual(await runner(process.execPath, node("")), { code: 0 });
  await assert.rejects(
    runner(process.execPath, node("process.exit(3)")),
    /exited with code 3/,
  );
  await assert.rejects(
    runner(path.join(temporary, "missing-opener"), []),
    /Could not start/,
  );
  const started = Date.now();
  assert.deepEqual(
    await runner(process.execPath, node("setTimeout(() => {}, 1500)")),
    { code: null },
  );
  assert.ok(Date.now() - started < 1_400, "a running opener is not awaited");
});

test("stops npm at its legacy username prompt", async () => {
  const urls = [];
  const started = Date.now();
  const result = await runInteractive(
    {
      program: process.execPath,
      args: [
        "-e",
        'process.stdout.write("Login at:\\nhttps://www.npmjs.com/login?next=/login/cli/1\\nUsername: "); setTimeout(() => {}, 60000);',
      ],
    },
    {
      cwd: temporary,
      mirror: false,
      stopOnLegacyLogin: true,
      onUrl: (url) => urls.push(url),
    },
  );
  assert.equal(result.legacyLogin, true);
  assert.deepEqual(urls, ["https://www.npmjs.com/login?next=/login/cli/1"]);
  assert.ok(Date.now() - started < 20_000, "the prompt is not awaited");
});
