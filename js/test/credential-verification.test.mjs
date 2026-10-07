import assert from "node:assert/strict";
import { test } from "node:test";
import { verifyCredentialWorkflow } from "../src/credential-verification.mjs";

test("credential verification watches only the nonce-correlated dispatch (#38)", async () => {
  const calls = [];
  const session = {
    plan: { repository: { github_owner: "acme", github_repository: "tool" } },
    options: { verificationNonce: "nonce-unique" },
    capture: async (step) => {
      const args = step.command.args;
      calls.push(args);
      let data = "";
      if (args[0] === "repo") {
        data = JSON.stringify({ defaultBranchRef: { name: "main" } });
      }
      if (args[1] === "list") {
        data = JSON.stringify([
          { databaseId: 2, displayTitle: "unrelated latest run" },
          {
            databaseId: 1,
            displayTitle: "prm credential verification nonce-unique",
          },
        ]);
      }
      if (args[0] === "run" && args[1] === "view") {
        assert.equal(args[2], "1");
        data = JSON.stringify({ status: "completed", conclusion: "success" });
      }
      return { code: 0, stdout: data };
    },
  };
  await verifyCredentialWorkflow(session, "verify.yml");
  const dispatch = calls.find((args) => args[0] === "workflow");
  assert.ok(dispatch.includes("prm_nonce=nonce-unique"));
  assert.ok(!dispatch.includes("--json"));
});

test("a failed correlated login cannot authorize revocation (#38)", async () => {
  const session = {
    plan: { repository: { github_owner: "acme", github_repository: "tool" } },
    options: { verificationNonce: "unique" },
    capture: async (step) => ({
      code: 0,
      stdout: JSON.stringify(
        step.command.args[0] === "repo"
          ? { defaultBranchRef: { name: "main" } }
          : step.command.args[1] === "list"
            ? [{ databaseId: 1, displayTitle: "unique" }]
            : { status: "completed", conclusion: "failure" },
      ),
    }),
  };
  await assert.rejects(
    verifyCredentialWorkflow(session, "verify.yml"),
    /old token remains active/,
  );
});
