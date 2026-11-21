import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { gradleDistribution } from "../src/gradle-distribution.mjs";

test("Gradle discovers custom desktop, Android and JS IR Maven publications", async () => {
  const manifest = "packages/ui-kotlin/build.gradle.kts";
  const manifestPath = fileURLToPath(
    new URL(
      `../../tests/fixtures/gradle-publication/ui-kotlin/${manifest}`,
      import.meta.url,
    ),
  );
  const contents = await readFile(manifestPath, "utf8");
  const item = await gradleDistribution(contents, manifest, manifestPath);

  assert.equal(item.name, "foundation.link.ui:ui-kotlin");
  assert.deepEqual(
    item.variants.map((variant) => variant.coordinates),
    [
      "foundation.link.ui:ui-kotlin-desktop",
      "foundation.link.ui:ui-kotlin-android",
      "foundation.link.ui:ui-kotlin-js",
    ],
  );
  assert.equal(item.variants[1].requires, "Android SDK");
});
