import assert from "node:assert/strict";
import { test } from "node:test";
import {
  identityConflicts,
  markIdentityConflicts,
  validateIdentities,
} from "../src/identity-conflicts.mjs";

test("identity preflight includes every repo, manifest, alias and version", () => {
  const first = {
    repository: { root: "a", github_owner: "owner", github_repository: "a" },
    packages: [
      {
        registry: "npm",
        name: "same",
        manifest: "js/package.json",
        version: "1.0.0",
        publishable: true,
      },
      {
        registry: "npm",
        name: "same",
        manifest: "vscode/package.json",
        version: "0.1.0",
        publishable: true,
      },
    ],
  };
  const second = {
    repository: { root: "b" },
    packages: [
      {
        registry: "npm",
        name: "same",
        manifest: "package.json",
        version: "2.0.0",
        publishable: true,
        generated_alias: { source_manifest: "package.json" },
      },
    ],
  };
  assert.equal(identityConflicts([first, second])[0].sources.length, 3);
  markIdentityConflicts([first, second]);
  assert.throws(
    () =>
      validateIdentities([
        {
          repository: first.repository,
          registry: "npm",
          package: first.packages[0],
        },
      ]),
    /distinct configured names/,
  );
});

test("registry-specific normalization preserves case-sensitive Go identities", () => {
  const inspection = {
    repository: { root: "." },
    packages: [
      {
        registry: "crates-io",
        name: "Some_Name",
        manifest: "a",
        publishable: true,
      },
      {
        registry: "crates-io",
        name: "some-name",
        manifest: "b",
        publishable: true,
      },
      {
        registry: "go-modules",
        name: "example.com/A",
        manifest: "c",
        publishable: true,
      },
      {
        registry: "go-modules",
        name: "example.com/a",
        manifest: "d",
        publishable: true,
      },
    ],
  };
  assert.equal(identityConflicts([inspection]).length, 1);
});

test("reviewed shared artifact mapping allows only the canonical selected publisher", () => {
  const mapping = {
    registry: "npm",
    name: "same",
    reviewed: true,
    artifact: "shared-library",
    sources: [
      {
        repository: "owner/repo",
        manifest: "js/package.json",
        version: "1.0.0",
      },
      {
        repository: "owner/repo",
        manifest: "reference/package.json",
        version: "1.0.0",
      },
    ],
    publisher: { repository: "owner/repo", manifest: "js/package.json" },
  };
  const inspection = {
    repository: { github_owner: "owner", github_repository: "repo" },
    packages: mapping.sources.map((source) => ({
      registry: "npm",
      name: "same",
      publishable: true,
      manifest: source.manifest,
      version: source.version,
      shared_artifact: mapping,
    })),
  };
  markIdentityConflicts([inspection]);
  const plans = inspection.packages.map((item) => ({
    registry: "npm",
    repository: inspection.repository,
    package: item,
  }));
  assert.doesNotThrow(() => validateIdentities([plans[0]]));
  assert.throws(() => validateIdentities(plans), /distinct configured names/);
  inspection.packages[0].identity_conflicts[0].version = "2.0.0";
  assert.throws(
    () => validateIdentities([plans[0]]),
    /distinct configured names/,
  );
});
