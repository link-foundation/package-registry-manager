import assert from "node:assert/strict";
import { test } from "node:test";
import { defaultScope } from "../src/default-scope.mjs";

test("implicit scope prefers the repository organization, then sole membership, then user (#48)", async () => {
  const answers = new Map([
    ["repo view --json nameWithOwner --jq .nameWithOwner", "team/project"],
    [
      "api repos/team/project",
      { owner: { type: "Organization", login: "team" } },
    ],
    ["api user/orgs --paginate --slurp --jq flatten", [{ login: "only" }]],
    ["api user", { login: "maintainer" }],
  ]);
  const run = async (args) => answers.get(args.join(" "));
  assert.deepEqual(await defaultScope({ run }), { org: "team" });
  answers.delete("repo view --json nameWithOwner --jq .nameWithOwner");
  assert.deepEqual(await defaultScope({ run }), { org: "only" });
  answers.set("api user/orgs --paginate --slurp --jq flatten", [
    { login: "one" },
    { login: "two" },
  ]);
  assert.deepEqual(await defaultScope({ run }), { user: "maintainer" });
  answers.delete("api user");
  assert.deepEqual(await defaultScope({ run }), {});
});

test("explicit repository, ref, and offline inspection keep local scope (#48)", async () => {
  const run = async () => {
    throw new Error("must not contact GitHub");
  };
  for (const options of [
    { repository: "." },
    { ref: "main" },
    { offline: true },
  ]) {
    assert.deepEqual(await defaultScope({ ...options, run }), {});
  }
});
