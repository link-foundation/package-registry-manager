// Run with geckodriver on PATH and PRM_FIREFOX_EXECUTABLE pointing at Firefox:
// xvfb-run -a node experiments/issue-32/firefox-smoke.mjs
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import os from "node:os";
import path from "node:path";

import { connectAutomation } from "../../js/src/automation.mjs";
import { npmPrefillScript } from "../../js/src/browser.mjs";
import { parseBrowserOptions } from "../../js/src/browser-options.mjs";
import { pageFetchScript } from "../../js/src/crates-api.mjs";

const html = `<label>Project<input name="project"></label>
<label>Organization<input name="organization"></label>
<label>Repository<input name="repository"></label>
<label>Workflow<input name="workflow"></label>`;
const server = createServer((request, response) => {
  if (request.url === "/api/v1/me") {
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ user: { login: "fixture" }, cookie: request.headers.cookie }));
  } else {
    response.setHeader("content-type", "text/html");
    response.end(html);
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const url = `http://127.0.0.1:${server.address().port}`;
const profile = await mkdtemp(path.join(os.tmpdir(), "prm-firefox-smoke-"));
let page;
try {
  page = await connectAutomation({
    browser: parseBrowserOptions({ channel: "firefox", executable: process.env.PRM_FIREFOX_EXECUTABLE,
      importFrom: "chrome", importScope: "domains" }),
    profile, domains: ["127.0.0.1"], verbose: true,
  }, { readCookies: async () => [{ name: "fixture", value: "signed-in", domain: "127.0.0.1", path: "/", httpOnly: true, secure: false, sameSite: "Lax", expires: -1 }] });
  await page.goto(url);
  const prefill = { project: "demo", organization: "owner", repository: "repo", workflow: "release.yml" };
  const npm = await page.evaluate(npmPrefillScript(prefill));
  assert.ok(npm.filled.length >= 3, JSON.stringify(npm));
  const pypi = await page.evaluate(npmPrefillScript(prefill));
  assert.ok(pypi.filled.length >= 3, JSON.stringify(pypi));
  const me = await page.evaluate(pageFetchScript("GET", "/api/v1/me"));
  assert.equal(me.status, 200);
  assert.equal(me.body.user.login, "fixture");
  assert.match(me.body.cookie, /fixture=signed-in/);
  await page.clearCookies(["127.0.0.1"]);
  const signedOut = await page.evaluate(pageFetchScript("GET", "/api/v1/me"));
  assert.equal(signedOut.body.cookie, undefined);
  console.log("Firefox npm/PyPI form filling, async crates.io page fetch, imported sign-in and cookie cleanup passed.");
} finally {
  await page?.close();
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
  await rm(profile, { recursive: true, force: true });
}
