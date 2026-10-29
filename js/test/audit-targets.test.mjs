/* global Response */
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import { probeRegistryState } from "../src/registry-state.mjs";
import { packageFindings } from "../src/organization-scan.mjs";

async function repository(t, files) {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-audit-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const [name, contents] of Object.entries(files)) {
    await mkdir(path.dirname(path.join(root, name)), { recursive: true });
    await writeFile(path.join(root, name), contents);
  }
  return root;
}

test("generated aliases retain distinct names, version, generator and workflow", async (t) => {
  const root = await repository(t, {
    "package.json": JSON.stringify({
      name: "original",
      version: "1.2.3",
      publishAliases: ["alias"],
      scripts: { "prepare:npm": "node scripts/prepare-npm-packages.mjs" },
    }),
    "scripts/prepare-npm-packages.mjs": "// generator",
    ".github/workflows/release.yml":
      "on: push\njobs:\n  publish:\n    permissions:\n      id-token: write\n    steps:\n      - run: npm publish\n",
  });
  const inspection = await inspectRepository(root);
  assert.deepEqual(
    inspection.packages.map((p) => p.name),
    ["alias", "original"],
  );
  const alias = inspection.packages[0];
  assert.equal(alias.version, "1.2.3");
  assert.equal(alias.generated_alias.source_manifest, "package.json");
  assert.equal(
    alias.generated_alias.generator,
    "scripts/prepare-npm-packages.mjs",
  );
  assert.equal(alias.workflow, "release.yml");
  assert.equal(
    buildPlans(inspection).filter((p) => p.package.name === "alias").length,
    1,
  );
});

test("build-only Gradle, unpublished examples and development snapshots are skipped", async (t) => {
  const root = await repository(t, {
    "android/app/build.gradle":
      "plugins { id 'com.android.application' }\nandroid { defaultConfig { applicationId 'org.example.app' } }",
    "android/build.gradle":
      "buildscript { dependencies { classpath 'android' } }",
    "examples/BasicUsage.csproj":
      "<Project><PropertyGroup><OutputType>Exe</OutputType></PropertyGroup></Project>",
    "dev/log/issues/1/package.json": '{"name":"template","version":"1.0.0"}',
    "lib/build.gradle.kts":
      'plugins { `java-library`; `maven-publish` }\ngroup = "org.example"\nversion = "1.0.0"\narchivesBaseName = "library"\npublishing { publications { create<MavenPublication>("maven") { from(components["java"]) } } }',
  });
  const inspection = await inspectRepository(root, { includeSkipped: true });
  assert.deepEqual(
    inspection.packages.filter((p) => p.publishable).map((p) => p.name),
    ["org.example:library"],
  );
  for (const manifest of [
    "android/app/build.gradle",
    "android/build.gradle",
    "examples/BasicUsage.csproj",
    "dev/log/issues/1/package.json",
  ]) {
    assert.ok(
      inspection.skipped.some((p) => p.manifest === manifest),
      manifest,
    );
  }
});

test("Cargo member versions inherit the nearest workspace package version", async (t) => {
  const root = await repository(t, {
    "rust/Cargo.toml":
      '[workspace]\nmembers = ["member"]\n[workspace.package]\nversion = "0.5.0"\n',
    "rust/member/Cargo.toml":
      '[package]\nname = "member"\nversion.workspace = true\n',
  });
  assert.equal((await inspectRepository(root)).packages[0].version, "0.5.0");
});

test("an existing npm name with an absent manifest version has an explicit finding", async () => {
  const source = {
    repository: { root: ".", github_owner: "acme", github_repository: "repo" },
    packages: [
      {
        registry: "npm",
        name: "lazy",
        version: "1.1.0",
        manifest: "package.json",
        publishable: true,
      },
    ],
  };
  const probed = await probeRegistryState(source, {
    runIdentity: async () => undefined,
    fetch: async (url) =>
      new Response(
        JSON.stringify(
          url.endsWith("/latest") ? { name: "lazy", version: "1.0.4" } : {},
        ),
        { status: url.endsWith("/1.1.0") ? 404 : 200 },
      ),
  });
  assert.equal(probed.packages[0].latest_version, "1.0.4");
  assert.equal(probed.packages[0].manifest_version_exists, false);
  assert.ok(
    packageFindings(probed).some((f) => f.type === "manifest-version-absent"),
  );
});

test("C++ distribution metadata yields three unverified channels with a shared version", async (t) => {
  const root = await repository(t, {
    "CMakeLists.txt":
      "project(my_package VERSION 0.2.1 LANGUAGES CXX)\ninstall(TARGETS my_package)",
    "conanfile.py":
      'from conan import ConanFile\nclass Package(ConanFile):\n    name = "my-package"\n    def set_version(self): pass\n',
    "vcpkg.json": '{"name":"my-package","version":"0.2.1"}',
  });
  const result = await inspectRepository(root);
  assert.deepEqual(
    result.packages.map((p) => p.registry),
    ["cmake", "conan", "vcpkg"],
  );
  assert.ok(
    result.packages.every(
      (p) => p.version === "0.2.1" && p.exists_on_registry === undefined,
    ),
  );
  assert.ok(
    buildPlans(result).every((p) =>
      p.steps.some((s) => s.id === "validate-package"),
    ),
  );
});
