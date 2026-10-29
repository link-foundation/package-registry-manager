import assert from "node:assert/strict";
import { test } from "node:test";
import { credentialPolicy } from "../src/credential-cycle.mjs";
import { auditRegistryTokens, tokenSecretSteps } from "../src/tokens.mjs";

test("NuGet retains every configured fallback independently of OIDC success", () => {
  assert.equal(credentialPolicy("nuget").mode, "oidc-with-token-fallback");
  assert.equal(
    tokenSecretSteps(
      { registry: "nuget", token_secrets: ["CUSTOM_NUGET"] },
      "a/b",
    ).some((s) => s.id === "delete-token-secret"),
    false,
  );
  assert.deepEqual(
    auditRegistryTokens('[{"name":"NUGET_TOKEN"},{"name":"CUSTOM_NUGET"}]', {
      registry: "nuget",
      token_secrets: ["CUSTOM_NUGET"],
    }),
    { inUse: ["NUGET_TOKEN", "CUSTOM_NUGET"], leftover: [] },
  );
});

import {
  publishNugetArtifact,
  nugetPolicyState,
  nugetVerificationEvidence,
} from "../src/nuget-policy.mjs";

const policy = {
  username: "profile",
  package_owner: "Org",
  organization: "github-owner",
  repository: "libraries",
  owner_id: "12",
  repository_id: "34",
  workflow: "ci.yml",
  package_ids: ["Org.Lib", "Org.Tool"],
  operations: ["package:pushversion"],
};
test("NuGet policies compare actual account, owner, exact IDs, operations and activation", () => {
  assert.equal(
    nugetPolicyState([{ ...policy, activation: "active" }], policy, "wrong")
      .status,
    "wrong-account",
  );
  assert.equal(
    nugetPolicyState(
      [{ ...policy, package_owner: "profile", activation: "active" }],
      policy,
      "profile",
    ).verified,
    false,
  );
  assert.equal(
    nugetPolicyState(
      [{ ...policy, activation: "temporarily-active" }],
      policy,
      "profile",
    ).verified,
    false,
  );
  assert.equal(
    nugetPolicyState([{ ...policy, activation: "inactive" }], policy, "profile")
      .status,
    "inactive",
  );
  assert.equal(
    nugetPolicyState(
      [{ ...policy, activation: "active" }, policy],
      policy,
      "profile",
    ).status,
    "duplicate",
  );
  assert.equal(
    nugetPolicyState([{ ...policy, activation: "active" }], policy, "profile")
      .verified,
    true,
  );
  assert.throws(
    () =>
      nugetPolicyState(
        [],
        { ...policy, package_ids: ["MyPackage"] },
        "profile",
      ),
    /non-template/,
  );
});

test("NuGet workflow and environment subjects are exact and one publish operation is selected", () => {
  for (const change of [
    { workflow: "CI.yml" },
    { environment: "production" },
  ]) {
    assert.equal(
      nugetPolicyState(
        [{ ...policy, ...change, activation: "active" }],
        policy,
        "profile",
      ).verified,
      false,
    );
  }
  assert.throws(
    () =>
      nugetPolicyState(
        [],
        { ...policy, operations: ["package:push", "package:pushversion"] },
        "profile",
      ),
    /one explicit/,
  );
});

test("OIDC success ignores invalid fallback; missing username uses provided custom fallback", async () => {
  let logins = 0;
  const adapter = {
    username: "profile",
    fallback: "invalid",
    login: async () => {
      logins++;
      return { status: "success", key: "temporary" };
    },
    fingerprint: async () => "sha",
    push: async (_, key) => ({
      status: key === "temporary" ? "accepted" : "authentication-failed",
    }),
  };
  assert.equal(
    (await publishNugetArtifact("same.nupkg", adapter)).selected_method,
    "trusted-publishing",
  );
  const result = await publishNugetArtifact("same.nupkg", {
    ...adapter,
    username: undefined,
    fallback: "custom",
    push: async () => ({ status: "accepted" }),
  });
  assert.equal(logins, 1);
  assert.equal(result.selected_method, "token-fallback");
  assert.equal(result.trust_repaired, false);
  assert.equal(result.attempts[0].status, "unavailable");
});

test("NuGet retries only auth rejection with the unchanged artifact after exact absence", async () => {
  const pushed = [];
  const adapter = {
    username: "profile",
    fallback: "fallback",
    login: async () => ({ status: "success", key: "temporary" }),
    fingerprint: async () => "sha",
    exactVersionExists: async () => false,
    push: async (artifact, key) => {
      pushed.push([artifact, key]);
      return {
        status: key === "temporary" ? "authentication-failed" : "accepted",
      };
    },
  };
  assert.equal(
    (await publishNugetArtifact("v1.nupkg", adapter)).selected_method,
    "token-fallback",
  );
  assert.deepEqual(pushed, [
    ["v1.nupkg", "temporary"],
    ["v1.nupkg", "fallback"],
  ]);
  for (const status of [
    "transport-failed",
    "build-failed",
    "verification-unsupported",
  ]) {
    let count = 0;
    const result = await publishNugetArtifact("v1.nupkg", {
      ...adapter,
      push: async () => {
        count++;
        return { status };
      },
    });
    assert.equal(count, 1);
    assert.equal(result.trust_repaired, false);
  }
  const result = await publishNugetArtifact("v1.nupkg", {
    ...adapter,
    exactVersionExists: async () => true,
  });
  assert.equal(result.publication, "already-present");
  assert.equal(result.trust_repaired, false);
});

test("PackageVerify HTTP 400 and derived keys preserve uncertainty with redaction", () => {
  const evidence = nugetVerificationEvidence(
    400,
    "requires package:verify temporary-secret",
    { secrets: ["temporary-secret"] },
  );
  assert.match(evidence.reason, /push-only/);
  assert.doesNotMatch(evidence.diagnostic, /temporary-secret/);
  assert.equal(evidence.authorization, "unknown");
  assert.match(
    nugetVerificationEvidence(200, "ok", { derivedKey: true }).reason,
    /original policy package scope is unproven/,
  );
});
