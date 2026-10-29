import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { configureNuget } from "../src/nuget-setup.mjs";

test("NuGet sign-in prompts and mismatch errors omit the configured profile", async () => {
  const repository = await mkdtemp(path.join(tmpdir(), "prm-nuget-privacy-"));
  const username = "private-profile-18";
  try {
    await writeFile(
      path.join(repository, ".package-registry-manager.json"),
      JSON.stringify({
        nuget: {
          username,
          package_owner: "Org",
          package_ids: ["Org.Library"],
          operations: ["package:pushversion"],
        },
      }),
    );
    for (const actual of [undefined, "different-profile"]) {
      const prompts = [];
      const session = {
        options: {
          repository,
          github: {
            rest: { request: async () => ({ id: 34, owner: { id: 12 } }) },
          },
        },
        plan: {
          package: { name: "Org.Library" },
          trusted_publisher: {
            organization: "acme",
            repository: "library",
            workflow: "ci.yml",
          },
        },
        visiblePage: async () => ({
          goto: async () => {},
          evaluate: async () => ({ ready: Boolean(actual), username: actual }),
        }),
        prompt: async (message) => {
          prompts.push(message);
          throw new Error("sign-in interrupted");
        },
      };
      await assert.rejects(configureNuget(session), (error) => {
        assert.doesNotMatch(error.message, new RegExp(username));
        assert.match(
          error.message,
          actual ? /differs from/ : /sign-in interrupted/,
        );
        return true;
      });
      assert.equal(prompts.length, actual ? 0 : 1);
      assert.ok(prompts.every((message) => !message.includes(username)));
    }
  } finally {
    await rm(repository, { recursive: true, force: true });
  }
});
