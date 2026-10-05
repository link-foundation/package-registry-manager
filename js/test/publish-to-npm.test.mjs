import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  VISIBILITY_DELAYS,
  classifyRegistryLookup,
  registryLookupArgs,
  totalWaitSeconds,
  waitForVisibility,
} from "../scripts/publish-to-npm.mjs";

test("registry lookup distinguishes published, missing, and unknown states", () => {
  assert.equal(
    classifyRegistryLookup(
      { status: 0, stdout: '"1.2.3"\n', stderr: "" },
      "1.2.3",
    ),
    "published",
  );
  assert.equal(
    classifyRegistryLookup(
      { status: 1, stdout: "", stderr: "npm error code E404" },
      "1.2.3",
    ),
    "missing",
  );
  assert.equal(
    classifyRegistryLookup(
      { status: 1, stdout: "", stderr: "network timeout" },
      "1.2.3",
    ),
    "unknown",
  );
});

const lookupResult = (state, version = "1.2.3") =>
  state === "published"
    ? { status: 0, stdout: `"${version}"\n`, stderr: "" }
    : { status: 1, stdout: "", stderr: "npm error code E404" };

/** A stand-in for npm that answers "missing" for the first `misses` lookups. */
function stubNpm(misses) {
  const calls = [];
  const run = (args) => {
    calls.push(args);
    return lookupResult(calls.length > misses ? "published" : "missing");
  };
  return { calls, run };
}

test("every registry lookup bypasses npm's cached packument (#28)", () => {
  assert.deepEqual(registryLookupArgs("demo@1.2.3"), [
    "view",
    "demo@1.2.3",
    "version",
    "--json",
    "--prefer-online",
  ]);
});

test("waitForVisibility keeps polling until a lagging registry shows the version", async () => {
  const npm = stubNpm(3);
  const waits = [];
  const visible = await waitForVisibility("demo@1.2.3", "1.2.3", {
    run: npm.run,
    wait: async (seconds) => waits.push(seconds),
    log: () => {},
  });

  assert.equal(visible, true);
  assert.equal(npm.calls.length, 4);
  assert.deepEqual(waits, VISIBILITY_DELAYS.slice(0, 3));
  for (const args of npm.calls) {
    assert.ok(args.includes("--prefer-online"), args.join(" "));
  }
});

test("waitForVisibility gives up without throwing once the window runs out", async () => {
  const npm = stubNpm(Infinity);
  const waits = [];
  const visible = await waitForVisibility("demo@1.2.3", "1.2.3", {
    run: npm.run,
    wait: async (seconds) => waits.push(seconds),
    log: () => {},
  });

  assert.equal(visible, false);
  assert.equal(npm.calls.length, VISIBILITY_DELAYS.length + 1);
  assert.deepEqual(waits, VISIBILITY_DELAYS);
});

test("the visibility window covers several minutes of registry lag (#28)", () => {
  // The 0.22.0 release became visible 132 seconds after publishing, 12 seconds
  // after the old 120-second window closed.
  assert.ok(totalWaitSeconds() >= 600, `${totalWaitSeconds()} seconds`);
  assert.ok(VISIBILITY_DELAYS.every((delay) => delay <= 60));
});

test("publish-to-npm warns instead of failing when npm publish succeeded", async () => {
  const source = await readFile(
    new URL("../scripts/publish-to-npm.mjs", import.meta.url),
    "utf8",
  );
  assert.match(source, /::warning title=npm registry lag::/u);
  assert.doesNotMatch(
    source,
    /was not visible from the npm registry after 120/u,
  );
  assert.match(
    source,
    /isDirectExecution\(import\.meta\.url, process\.argv\[1\]\)/u,
  );
});

test(
  "publish-to-npm publishes and confirms the version through uncached lookups",
  { skip: process.platform === "win32" && "fake tools are POSIX scripts" },
  async () => {
    const dir = await mkdtemp(path.join(tmpdir(), "publish-to-npm-"));
    const manifest = JSON.parse(
      await readFile(new URL("../package.json", import.meta.url), "utf8"),
    );
    // A stand-in npm: `view` answers E404 until `publish` has run.
    await writeFile(
      path.join(dir, "npm"),
      [
        "#!/bin/sh",
        `echo "$*" >> "${dir}/calls"`,
        'case "$1" in',
        `  publish) touch "${dir}/published" ;;`,
        `  view) if [ -f "${dir}/published" ]; then echo '"${manifest.version}"'; else echo "npm error code E404" >&2; exit 1; fi ;;`,
        "esac",
      ].join("\n"),
    );
    await chmod(path.join(dir, "npm"), 0o755);
    const outputs = path.join(dir, "outputs");
    const script = fileURLToPath(
      new URL("../scripts/publish-to-npm.mjs", import.meta.url),
    );

    const result = spawnSync(process.execPath, [script], {
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${dir}${path.delimiter}${process.env.PATH}`,
        GITHUB_OUTPUT: outputs,
      },
    });

    assert.equal(result.status, 0, result.stderr);
    const calls = (await readFile(path.join(dir, "calls"), "utf8"))
      .trim()
      .split("\n");
    const spec = `${manifest.name}@${manifest.version}`;
    assert.deepEqual(calls, [
      `view ${spec} version --json --prefer-online`,
      "publish --access public --provenance",
      `view ${spec} version --json --prefer-online`,
    ]);
    assert.equal(
      await readFile(outputs, "utf8"),
      `published=true\npublished_version=${manifest.version}\n`,
    );
  },
);
