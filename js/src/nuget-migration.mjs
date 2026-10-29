import { nugetWorkflowSteps } from "./nuget-workflow.mjs";

/** Preserve validation, build, artifact and step conditions when replacing a direct push. */
export function convertNugetStep(step, packages, index) {
  if (packages.length !== 1) {
    throw new Error(
      "NuGet migration requires a package-specific publishing step; ambiguous multi-package globs need explicit review",
    );
  }
  const run = step.find((line) => /^\s*-?\s*run:/.test(line));
  const push =
    /^\s*-?\s*run:\s*dotnet\s+nuget\s+push\s+(?:"([^"\n]+)"|'([^'\n]+)'|([^\s]+))\s*(.*)$/.exec(
      run ?? "",
    );
  if (!push || /[;\n`]|\$\(/.test(push.slice(1).join(""))) {
    throw new Error(
      "NuGet script or multiline push requires a reviewed artifact-only publishing command; validation/build commands cannot be retried",
    );
  }
  const glob = push[1] ?? push[2] ?? push[3];
  if (!/^[\w./*${}-]+$/.test(glob) || glob.includes("${{")) {
    throw new Error(
      "NuGet artifact glob must resolve from job-local environment variables",
    );
  }
  if (
    /--source\s+(?!["']?https:\/\/api\.nuget\.org\/v3\/index\.json(?:["']?\s|$))/.test(
      push[4],
    )
  ) {
    throw new Error(
      "NuGet policy migration requires the explicit nuget.org destination",
    );
  }
  const custom = [...step.join("\n").matchAll(/secrets\.([\w]+)/g)].map(
    (match) => match[1],
  );
  const output = nugetWorkflowSteps(
    {
      ...packages[0],
      token_secrets: [...custom, ...(packages[0].token_secrets ?? [])],
    },
    { build: false, artifactGlob: glob },
  );
  const prefix = " ".repeat(step[0].search(/\S/));
  const lines = output
    .map((line) => prefix + line.slice(6))
    .map((line) =>
      line.replaceAll("prm_nuget_login", `prm_nuget_login_${index}`),
    );
  const condition = step.find((line) => /^\s*if:/.test(line));
  const directory = step.find((line) => /^\s*working-directory:/.test(line));
  const publish = lines.findIndex((line) =>
    /- name: Publish validated NuGet/.test(line),
  );
  if (condition) {
    const expression = condition
      .trim()
      .slice(3)
      .trim()
      .replace(/^\$\{\{\s*|\s*\}\}$/g, "");
    const loginIf = lines.findIndex((line) => /if: vars.NUGET_USER/.test(line));
    lines[loginIf] += ` && (${expression})`;
    lines.splice(publish + 1, 0, `${prefix}  if: ${expression}`);
  }
  if (directory) {
    lines.splice(
      lines.findIndex((line) => /- name: Publish validated NuGet/.test(line)) +
        1,
      0,
      `${prefix}  ${directory.trim()}`,
    );
  }
  const envIndex = step.findIndex((line) => /^\s*env:/.test(line));
  if (envIndex >= 0) {
    if (!/^\s*env:\s*$/.test(step[envIndex])) {
      throw new Error(
        "NuGet migration requires a resolved step environment mapping",
      );
    }
    const level = step[envIndex].search(/\S/);
    const entries = [];
    for (const line of step.slice(envIndex + 1)) {
      if (line.trim() && line.search(/\S/) <= level) {
        break;
      }
      if (/^\s*PRM_\w+:/.test(line)) {
        throw new Error(
          "NuGet migration cannot overwrite a publication controller environment",
        );
      }
      entries.push(line);
    }
    const env = lines.findIndex((line) => /^\s*env:\s*$/.test(line));
    lines.splice(env + 1, 0, ...entries);
  }
  return lines;
}
