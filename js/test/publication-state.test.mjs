/* global Response */
import assert from "node:assert/strict";
import { test } from "node:test";
import { probePackage } from "../src/registry-state.mjs";

const pkg = (registry, name, version = "1.1.0") => ({
  registry,
  name,
  version,
});
const fetchWith = (response) => async (url) => {
  const value = response(url);
  return new Response(
    typeof value.body === "string" ? value.body : JSON.stringify(value.body),
    { status: value.status ?? 200 },
  );
};

test("npm exact presence survives a missing latest document and unpublish root metadata", async () => {
  const state = await probePackage(pkg("npm", "si-units"), {
    fetch: fetchWith((url) =>
      url.endsWith("/1.1.0")
        ? { body: { name: "si-units", version: "1.1.0" } }
        : { status: 404, body: {} },
    ),
  });
  assert.equal(state.exists, true);
  assert.equal(state.manifest_version_exists, true);
  assert.equal(state.latest_version, undefined);
});

test("failed or invalid exact metadata remains unknown, never absent", async () => {
  for (const result of [
    { status: 403, body: {} },
    { status: 429, body: {} },
    { status: 500, body: {} },
    { body: {} },
  ]) {
    const state = await probePackage(pkg("npm", "package"), {
      fetch: fetchWith((url) =>
        url.endsWith("/latest") ? { body: { version: "1.0.0" } } : result,
      ),
    });
    assert.equal(state.exists, true);
    assert.equal(state.manifest_version_exists, undefined);
  }
});

test("absent older source is stale and exact public URL is retained", async () => {
  const state = await probePackage(pkg("npm", "package", "1.0.0"), {
    fetch: fetchWith((url) =>
      url.endsWith("/latest")
        ? { body: { version: "2.0.0" } }
        : { status: 404, body: {} },
    ),
  });
  assert.equal(state.version_state, "stale-manifest-version");
  assert.match(state.version_evidence_url, /\/1.0.0$/);
});

test("Maven checks exact coordinates, Packagist versions and Go pseudo-versions", async () => {
  const maven = await probePackage(
    pkg("maven-central", "org.example:library"),
    {
      fetch: fetchWith((url) =>
        url.endsWith(".xml")
          ? {
              body: "<metadata><versioning><release>1.0.0</release></versioning></metadata>",
            }
          : { status: 404, body: "missing" },
      ),
    },
  );
  assert.equal(maven.exists, true);
  assert.equal(maven.manifest_version_exists, false);
  assert.equal(maven.version_state, "manifest-version-absent");
  const packagist = await probePackage(pkg("packagist", "org/library"), {
    fetch: fetchWith(() => ({
      body: {
        packages: {
          "org/library": [{ version: "v1.1.0" }, { version: "v1.0.0" }],
        },
        minified: "composer/2.0",
      },
    })),
  });
  assert.equal(packagist.manifest_version_exists, true);
  const urls = [];
  const go = await probePackage(pkg("go-modules", "example.com/Module", null), {
    fetch: fetchWith((url) => {
      urls.push(url);
      return {
        body: url.endsWith("/list")
          ? ""
          : { Version: "v0.0.0-20261029123456-123456abcdef" },
      };
    }),
  });
  assert.equal(go.exists, true);
  assert.equal(go.tagged_release_exists, false);
  assert.equal(go.version_state, "no-tagged-release");
  assert.ok(urls.every((url) => url.includes("!module")));
  assert.match(
    (await probePackage(pkg("conan", "library"))).registry_state_reason,
    /unsupported/,
  );
});

test("Packagist invalid responses stay unknown and older versions are stale", async () => {
  for (const body of [{}, { packages: { "org/library": [{}] } }]) {
    const result = await probePackage(pkg("packagist", "org/library"), {
      fetch: fetchWith(() => ({ body })),
    });
    assert.equal(result.exists, undefined);
    assert.match(result.registry_state_reason, /invalid/);
  }
  const stale = await probePackage(pkg("packagist", "org/library", "0.1.0"), {
    fetch: fetchWith(() => ({
      body: { packages: { "org/library": [{ version: "v2.0.0" }] } },
    })),
  });
  assert.equal(stale.version_state, "stale-manifest-version");
});

test("NuGet exact evidence uses normalized versions and rejects malformed indexes", async () => {
  const state = await probePackage(
    pkg("nuget", "Org.Lib", "01.2.3.0+build.9"),
    {
      fetch: fetchWith(() => ({ body: { versions: ["1.2.3"] } })),
    },
  );
  assert.equal(state.manifest_version_exists, true);
  for (const body of [
    {},
    { versions: [null] },
    ...["invalid", "1.2.3-", "1.2.3-?", "1.2.3-a..b", "1.2.3+"].map(
      (version) => ({ versions: [version] }),
    ),
  ]) {
    const state = await probePackage(pkg("nuget", "Org.Lib"), {
      fetch: fetchWith(() => ({ body })),
    });
    assert.equal(state.exists, undefined);
  }
});

test("malformed Go latest versions remain unknown rather than proving publication", async () => {
  for (const Version of [
    null,
    42,
    {},
    "",
    "invalid",
    "v1.2.3-?",
    "V1.2.3",
    "v01.2.3",
    "v1.02.3",
    "v1.2.03",
    "v1.2.3-01",
    "v1.2.3+build",
    "v1.2.3-alpha..1",
  ]) {
    const state = await probePackage(
      pkg("go-modules", "example.com/module", null),
      {
        fetch: fetchWith((url) => ({
          body: url.endsWith("/list") ? "" : { Version },
        })),
      },
    );
    assert.equal(state.exists, undefined, JSON.stringify(Version));
    assert.equal(state.version_state, undefined);
    assert.match(state.registry_state_reason, /invalid/);
  }
});

test("Go proxy accepts canonical versions and keeps malformed tagged lists unknown", async () => {
  for (const [Version, list, tagged] of [
    ["v2.3.4+incompatible", "v2.3.4+incompatible\n", true],
    ["v1.2.3-RC.1", "v1.2.3-RC.1\n", true],
    ["v1.2.3", "not-a-version\n", undefined],
    ["v1.2.3", "V1.2.3\n", undefined],
  ]) {
    const state = await probePackage(
      pkg("go-modules", "example.com/module", null),
      {
        fetch: fetchWith((url) => ({
          body: url.endsWith("/list") ? list : { Version },
        })),
      },
    );
    assert.equal(state.exists, true);
    assert.equal(state.latest_version, Version);
    assert.equal(state.tagged_release_exists, tagged);
    if (tagged === undefined) {
      assert.match(state.registry_state_reason, /invalid/);
    }
  }
});
