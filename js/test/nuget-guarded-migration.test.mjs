import assert from "node:assert/strict";
import { test } from "node:test";
import { convertNugetStep } from "../src/nuget-migration.mjs";

const pkg = [
  { registry: "nuget", name: "A.Lib", manifest: "csharp/A.Lib/A.Lib.csproj" },
];
const step = (body) => [
  "      - name: Publish NuGet package",
  "        id: publish",
  "        env:",
  "          NUGET_API_KEY: ${{ secrets.NUGET_API_KEY }}",
  "          NUGET_TOKEN: ${{ secrets.NUGET_TOKEN }}",
  "        run: |",
  ...body.map((line) => `          ${line}`),
];
const guarded = [
  "set -euo pipefail",
  'if [ "${{ steps.version-check.outputs.should_publish }}" != "true" ]; then',
  '  echo "published=skipped" >> "$GITHUB_OUTPUT"',
  "  exit 0",
  "fi",
  'API_KEY="${NUGET_API_KEY:-$NUGET_TOKEN}"',
  "# Report presence, never the value.",
  'if [ -z "$API_KEY" ]; then',
  '  echo "published=failed" >> "$GITHUB_OUTPUT"',
  "  exit 1",
  "fi",
  'dotnet nuget push "artifacts/*.nupkg" \\',
  '  --api-key "$API_KEY" \\',
  "  --source https://api.nuget.org/v3/index.json \\",
  "  --skip-duplicate",
  'echo "published=true" >> "$GITHUB_OUTPUT"',
];

test("guarded multiline NuGet push becomes OIDC-first with the token fallback and guard kept", () => {
  const text = convertNugetStep(step(guarded), pkg, 3).join("\n");
  assert.match(text, /NuGet\/login@/);
  assert.match(
    text,
    /PRM_NUGET_FALLBACK: \$\{\{ secrets.NUGET_API_KEY \|\| secrets.NUGET_TOKEN/,
  );
  assert.match(text, /PRM_ARTIFACT_GLOB: 'artifacts\/\*\.nupkg'/);
  assert.match(
    text,
    /if: \(steps.version-check.outputs.should_publish == 'true'\)/,
  );
  assert.match(text, /id: publish\n/);
  assert.match(text, /published=skipped/);
  assert.doesNotMatch(text, /dotnet (pack|test|build)/);
});

test("unreviewed commands in the push script return the exact prerequisite edit", () => {
  for (const extra of [
    "dotnet pack x.csproj",
    "dotnet test",
    "curl https://evil",
  ]) {
    const body = [...guarded];
    body.splice(5, 0, extra);
    assert.throws(
      () => convertNugetStep(step(body), pkg, 0),
      (error) =>
        error.code === "nuget-prerequisite-edit" &&
        /Split the NuGet publish step/.test(error.message),
    );
  }
  const other = guarded.map((line) =>
    line.replace(
      "https://api.nuget.org/v3/index.json",
      "https://example.test/v3/index.json",
    ),
  );
  assert.throws(
    () => convertNugetStep(step(other), pkg, 0),
    /nuget.org destination/,
  );
});
