import { readFileSync } from "node:fs";

/** Build once; NuGet/login creates a temporary key used only inside this job. */
export function nugetWorkflowSteps(item, options = {}) {
  if (
    !/^[a-z0-9_.-]+$/i.test(item.name) ||
    /^(MyPackage|PackageName|example)$/i.test(item.name)
  ) {
    throw new Error(
      "resolve exact NuGet package identity before proposing publication",
    );
  }
  const secrets = [
    ...new Set([...(item.token_secrets ?? []), "NUGET_API_KEY", "NUGET_TOKEN"]),
  ];
  if (secrets.some((name) => !/^[A-Z_][A-Z0-9_]*$/i.test(name))) {
    throw new Error("invalid NuGet fallback secret name");
  }
  const project = `'${item.manifest.split("/").at(-1).replaceAll("'", "''")}'`;
  const controller = readFileSync(
    new URL("./nuget-publish.py", import.meta.url),
    "utf8",
  ).trimEnd();
  const result = [
    "      - uses: actions/setup-dotnet@v6",
    "        with:",
    "          dotnet-version: '10.0.x'",
    `      - run: dotnet test ${project} --configuration Release`,
    `      - run: dotnet pack ${project} --configuration Release --no-build --output .prm-nuget`,
    "      - uses: NuGet/login@8d196754b4036150537f80ac539e15c2f1028841 # v1",
    "        id: prm_nuget_login",
    "        if: vars.NUGET_USER != ''",
    "        continue-on-error: true",
    "        with:",
    "          user: ${{ vars.NUGET_USER }}",
    "      - name: Publish validated NuGet artifact and verify public consumer",
    "        shell: bash",
    "        env:",
    `          PRM_PACKAGE_ID: '${item.name}'`,
    ...(options.artifactGlob
      ? [
          `          PRM_ARTIFACT_GLOB: '${options.artifactGlob.replaceAll("'", "''")}'`,
        ]
      : []),
    "          PRM_NUGET_USER: ${{ vars.NUGET_USER }}",
    "          PRM_NUGET_OIDC: ${{ steps.prm_nuget_login.outputs.NUGET_API_KEY }}",
    `          PRM_NUGET_FALLBACK: \${{ ${secrets.map((name) => `secrets.${name}`).join(" || ")} }}`,
    "        run: |",
    "          python3 - <<'PRM_NUGET_PY'",
    ...controller.split("\n").map((line) => `          ${line}`),
    "          PRM_NUGET_PY",
  ];
  return options.build === false ? result.slice(5) : result;
}
