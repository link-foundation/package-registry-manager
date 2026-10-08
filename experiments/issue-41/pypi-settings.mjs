// Exercise the PyPI settings scripts in a browser using a Warehouse-shaped
// fixture; every request is intercepted, so no registry account is needed.
import assert from "node:assert/strict";
import { chromium } from "../../js/node_modules/playwright/index.mjs";
import {
  PYPI_PUBLISHERS_SCRIPT,
  pypiRemovalScript,
} from "../../js/src/repository-repair.mjs";
import { publisherIdentities, publisherMatches } from "../../js/src/repository-identity.mjs";

const url = "https://pypi.org/manage/project/disk-space-saviour/settings/publishing/";
const expected = { repository: "link-foundation/disk-space-saviour", workflow: "release.yml", environment: null };
const prefill = { ...expected, organization: "link-foundation", repository: "disk-space-saviour" };
const publishers = [
  { id: "old-id", repository: "konard/disk-space-saviour", workflow: "release.yml", environment: null },
  { id: "new-id", ...expected },
  { id: "other-id", ...expected, workflow: "other.yml", environment: "release" },
];
const html = () => `<table class="table--publisher-list"><tbody>${publishers.map(publisher => `<tr>
  <td>GitHub</td><td><small>
    <b>Repository:</b> <a href="https://github.com/${publisher.repository}">${publisher.repository}</a><br>
    <b>Workflow:</b> ${publisher.workflow}<br>
    <b>Environment name:</b> ${publisher.environment ?? "<i>(Any)</i>"}
  </small></td><td><a href="#remove-publisher-${publisher.id}">Remove</a></td></tr>`).join("")}</tbody></table>
  ${publishers.map(publisher => `<form method="post" action="${url}"><input name="csrf_token" type="hidden" value="fixture-csrf"><input name="publisher_id" type="hidden" value="${publisher.id}"></form>`).join("")}`;

const browser = await chromium.launch({ executablePath: process.env.PRM_CHROME_PATH ?? "/usr/bin/google-chrome", headless: true });
try {
  const page = await browser.newPage();
  let removed = false;
  await page.route("**/*", async route => {
    assert.equal(route.request().url(), url);
    if (route.request().method() === "POST") {
      const body = route.request().postData();
      assert.match(body, /name="csrf_token"\r\n\r\nfixture-csrf/);
      assert.match(body, /name="publisher_id"\r\n\r\nold-id/);
      publishers.splice(0, 1);
      removed = true;
    }
    await route.fulfill({ status: 200, contentType: "text/html", body: html() });
  });
  await page.goto(url);
  const before = publisherIdentities(await page.evaluate(PYPI_PUBLISHERS_SCRIPT));
  assert.equal(before.length, 3);
  assert.deepEqual(before.map(publisher => publisherMatches(publisher, prefill)), [false, true, false]);
  assert.equal((await page.evaluate(pypiRemovalScript("old-id"))).status, 200);
  assert.equal(removed, true);
  await page.reload();
  const after = publisherIdentities(await page.evaluate(PYPI_PUBLISHERS_SCRIPT));
  assert.deepEqual(after.map(publisher => publisher.id), ["new-id", "other-id"]);
  await assert.rejects(page.evaluate(pypiRemovalScript("absent-id")), /removal form unavailable/);
  console.log("PASS: exact publisher matching, existing CSRF form, old-only removal, replacement retained");
} finally {
  await browser.close();
}
