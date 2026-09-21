import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import { test } from "node:test";

const repositoryRoot = new URL("../../", import.meta.url);

async function exists(relativePath) {
  try {
    await access(new URL(relativePath, repositoryRoot));
    return true;
  } catch {
    return false;
  }
}

test("keeps both publishable implementations in language folders", async () => {
  for (const relativePath of [
    "rust/Cargo.toml",
    "rust/Cargo.lock",
    "rust/README.md",
    "rust/src/lib.rs",
    "rust/src/main.rs",
    "rust/tests/unit/mod.rs",
    "js/package.json",
    "js/package-lock.json",
    "js/README.md",
    "js/src/index.mjs",
  ]) {
    assert.equal(await exists(relativePath), true, `missing ${relativePath}`);
  }

  assert.equal(
    await exists("Cargo.toml"),
    false,
    "the Rust package must not remain at the repository root",
  );
});

test("documents and checks the polyglot layout", async () => {
  const [readme, contributing, releaseWorkflow] = await Promise.all([
    readFile(new URL("README.md", repositoryRoot), "utf8"),
    readFile(new URL("CONTRIBUTING.md", repositoryRoot), "utf8"),
    readFile(new URL(".github/workflows/release.yml", repositoryRoot), "utf8"),
  ]);

  assert.match(readme, /`rust\/`/u);
  assert.match(readme, /`js\/`/u);
  assert.doesNotMatch(readme, /Rust package remains at the repository root/u);
  assert.match(
    contributing,
    /cargo (?:build|test).*--manifest-path rust\/Cargo\.toml/u,
  );
  assert.match(releaseWorkflow, /--manifest-path rust\/Cargo\.toml/u);
  assert.match(
    releaseWorkflow,
    /cache-dependency-path: js\/package-lock\.json/u,
  );
  assert.match(releaseWorkflow, /^ {2}javascript-release:$/mu);
  assert.match(releaseWorkflow, /^ {6}id-token: write$/mu);
  assert.match(releaseWorkflow, /node js\/scripts\/publish-to-npm\.mjs/u);
  assert.match(releaseWorkflow, /node js\/scripts\/setup-npm\.mjs/u);
  assert.match(
    releaseWorkflow,
    /node js\/scripts\/sanitize-npm-userconfig\.mjs/u,
  );
});
