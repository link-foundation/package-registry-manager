import assert from "node:assert/strict";
import { test } from "node:test";
import { probePackage } from "../src/registry-state.mjs";

test("account scans query NuGet, RubyGems and JSR without guessing trusted-publisher settings (#43)", async () => {
  const packages = [
    [
      "nuget",
      "Team.Library",
      "https://api.nuget.org/v3-flatcontainer/team.library/index.json",
    ],
    [
      "rubygems",
      "team-tool",
      "https://rubygems.org/api/v1/gems/team-tool.json",
    ],
    ["jsr", "@team/tool", "https://jsr.io/@team/tool/meta.json"],
  ];
  for (const [registry, name, url] of packages) {
    const state = await probePackage(
      { registry, name },
      {
        fetch: async (actual) => {
          assert.equal(actual, url);
          return { ok: true, status: 200, json: async () => ({}) };
        },
      },
    );
    assert.equal(state.exists, true);
    assert.equal(state.trusted, undefined);
    assert.equal(
      (
        await probePackage(
          { registry, name },
          { fetch: async () => ({ status: 404 }) },
        )
      ).exists,
      false,
    );
  }
});
