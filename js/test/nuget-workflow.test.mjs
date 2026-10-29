import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { workflowProposal } from "../src/workflow-proposal.mjs";
import { convertNugetStep } from "../src/nuget-migration.mjs";

test("NuGet migration preserves artifact environment and the original publication guard", () => {
  const lines = convertNugetStep(
    [
      "      - name: Upload",
      "        if: github.ref == 'refs/heads/main'",
      "        env:",
      "          ARTIFACTS: ./packages",
      "          CUSTOM_KEY: ${{ secrets.CUSTOM_KEY }}",
      '        run: dotnet nuget push "$ARTIFACTS/*.nupkg" --api-key $CUSTOM_KEY --source https://api.nuget.org/v3/index.json',
    ],
    [{ name: "Org.Lib", manifest: "Lib.csproj", registry: "nuget" }],
    0,
  ).join("\n");
  assert.match(lines, /ARTIFACTS: .\/packages/);
  assert.match(lines, /secrets.CUSTOM_KEY/);
  assert.match(lines, /if: github.ref == 'refs\/heads\/main'/);
  assert.match(lines, /PRM_ARTIFACT_GLOB: '\$ARTIFACTS\/\*\.nupkg'/);
});

test("NuGet jobs build once and use official OIDC with a custom existing fallback", () => {
  const item = {
    registry: "nuget",
    name: "Org.Library",
    manifest: "src/Library.csproj",
    version: "1.2.3",
    publishable: true,
    token_secrets: ["CUSTOM_NUGET"],
  };
  const { contents } = workflowProposal(
    { repository: {}, packages: [item] },
    [item],
    [],
    "ci.yml",
  );
  assert.match(
    contents,
    /NuGet\/login@8d196754b4036150537f80ac539e15c2f1028841/,
  );
  assert.match(contents, /user: \$\{\{ vars.NUGET_USER \}\}/);
  assert.match(contents, /secrets.CUSTOM_NUGET/);
  assert.match(contents, /id-token: write/);
  assert.equal((contents.match(/dotnet pack /g) ?? []).length, 1);
  assert.match(contents, /exact_version_exists/);
  assert.match(contents, /sha256/);
  assert.doesNotMatch(contents, /dotnet nuget trust|--skip-duplicate/);
});

test("NuGet publication controller is identical in both ports", () => {
  assert.equal(
    readFileSync(new URL("../src/nuget-publish.py", import.meta.url), "utf8"),
    readFileSync(
      new URL("../../rust/src/nuget-publish.py", import.meta.url),
      "utf8",
    ),
  );
});
