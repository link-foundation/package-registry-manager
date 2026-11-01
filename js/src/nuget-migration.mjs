import { nugetWorkflowSteps } from "./nuget-workflow.mjs";

export const NUGET_PREREQUISITE_EDIT =
  "Split the NuGet publish step so one step runs only `dotnet nuget push <artifact glob> --api-key ... --source https://api.nuget.org/v3/index.json` (optionally behind a `should_publish` guard, `${A:-$B}` key selection, a key-presence check and `published=` outputs); pack, tests and any other command must stay in earlier steps.";

function prerequisite(reason) {
  const error = new Error(
    `${reason}. Prerequisite edit: ${NUGET_PREREQUISITE_EDIT}`,
  );
  error.code = "nuget-prerequisite-edit";
  error.prerequisite = NUGET_PREREQUISITE_EDIT;
  return error;
}

/**
 * Recognize the guarded multiline push used by links-notation style workflows.
 * Every line must be the guard, key selection, presence check, notices,
 * `published=` outputs or one nuget.org push; anything else is not migrated.
 */
export function guardedNugetPush(step) {
  const start = step.findIndex((line) =>
    /^\s*-?\s*run:\s*\|[-+]?\s*$/.test(line),
  );
  if (start < 0) {
    return null;
  }
  const level = step[start].search(/\S/);
  const body = [];
  for (const line of step.slice(start + 1)) {
    if (line.trim() && line.search(/\S/) <= level) {
      break;
    }
    body.push(line.trim());
  }
  const joined = body.join("\n").replace(/\\\n\s*/g, " ");
  const lines = joined
    .split("\n")
    .filter((line) => line && !line.startsWith("#"));
  let guard = null;
  let push = null;
  let block = null;
  for (const line of lines) {
    const opened =
      /^if \[ "\$\{\{\s*([\w.-]+)\s*\}\}" != "true" \]; then$/.exec(line);
    if (opened && !block && !guard) {
      guard = opened[1];
      block = "guard";
      continue;
    }
    if (/^if \[ -z "\$\w+" \]; then$/.test(line) && !block) {
      block = "missing";
      continue;
    }
    if (line === "fi" && block) {
      block = null;
      continue;
    }
    if (block === "guard" && /^exit 0$/.test(line)) {
      continue;
    }
    if (block === "missing" && /^exit 1$/.test(line)) {
      continue;
    }
    if (
      /^set -[euxo pipefail]+$/.test(line) ||
      /^\w+="\$\{\w+:-\$\w+\}"$/.test(line) ||
      /^echo "[^"`$]*(?:\$\{\{[^}]*\}\}[^"`$]*)*"(?: >> "\$GITHUB_OUTPUT")?$/.test(
        line,
      )
    ) {
      continue;
    }
    const command =
      /^dotnet nuget push (?:"([^"]+)"|'([^']+)'|(\S+))(.*)$/.exec(line);
    if (command && !push && !block) {
      push = command;
      continue;
    }
    throw prerequisite(
      `NuGet publish script contains an unreviewed command: ${line}`,
    );
  }
  if (!push || block) {
    throw prerequisite(
      "NuGet multiline step has no single recognizable artifact push",
    );
  }
  return {
    guard,
    glob: push[1] ?? push[2] ?? push[3],
    options: push[4],
    start,
    level,
  };
}

/** Preserve validation, build, artifact and step conditions when replacing a direct push. */
export function convertNugetStep(step, packages, index) {
  if (packages.length !== 1) {
    throw new Error(
      "NuGet migration requires a package-specific publishing step; ambiguous multi-package globs need explicit review",
    );
  }
  const guarded = guardedNugetPush(step);
  if (guarded) {
    return convertGuarded(step, packages, index, guarded);
  }
  const run = step.find((line) => /^\s*-?\s*run:/.test(line));
  const push =
    /^\s*-?\s*run:\s*dotnet\s+nuget\s+push\s+(?:"([^"\n]+)"|'([^'\n]+)'|([^\s]+))\s*(.*)$/.exec(
      run ?? "",
    );
  if (!push || /[;\n`]|\$\(/.test(push.slice(1).join(""))) {
    throw prerequisite(
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

/** Guard -> OIDC login -> controller push of the same artifacts -> original outputs. */
function convertGuarded(step, packages, index, guarded) {
  if (
    /--source\s+(?!["']?https:\/\/api\.nuget\.org\/v3\/index\.json)/.test(
      guarded.options,
    )
  ) {
    throw prerequisite(
      "NuGet policy migration requires the explicit nuget.org destination",
    );
  }
  if (!/^[\w./*-]+$/.test(guarded.glob)) {
    throw prerequisite("NuGet artifact glob must be a literal job-local path");
  }
  const prefix = " ".repeat(step[0].search(/\S/));
  const header = step.slice(0, guarded.start);
  const name =
    /-\s*name:\s*(.+)$/.exec(step[0])?.[1] ?? "Publish NuGet package";
  const id = header
    .map((line) => /^\s*id:\s*(\S+)/.exec(line)?.[1])
    .find(Boolean);
  const condition = header
    .map((line) => /^\s*if:\s*(.+)$/.exec(line)?.[1])
    .find(Boolean)
    ?.replace(/^\$\{\{\s*|\s*\}\}$/g, "");
  const secrets = [...step.join("\n").matchAll(/secrets\.(\w+)/g)].map(
    (m) => m[1],
  );
  const gates = [condition, guarded.guard && `${guarded.guard} == 'true'`]
    .filter(Boolean)
    .map((part) => `(${part})`)
    .join(" && ");
  const publishId = `prm_nuget_publish_${index}`;
  const lines = nugetWorkflowSteps(
    {
      ...packages[0],
      token_secrets: [...secrets, ...(packages[0].token_secrets ?? [])],
    },
    { build: false, artifactGlob: guarded.glob },
  )
    .map((line) => prefix + line.slice(6))
    .map((line) =>
      line.replaceAll("prm_nuget_login", `prm_nuget_login_${index}`),
    );
  const loginIf = lines.findIndex((line) => /if: vars.NUGET_USER/.test(line));
  if (gates) {
    lines[loginIf] += ` && ${gates}`;
  }
  const publish = lines.findIndex((line) =>
    /- name: Publish validated NuGet/.test(line),
  );
  lines.splice(
    publish + 1,
    0,
    `${prefix}  id: ${publishId}`,
    ...(gates ? [`${prefix}  if: ${gates}`] : []),
  );
  const record = [
    `${prefix}- name: ${name}`,
    ...(id ? [`${prefix}  id: ${id}`] : []),
    `${prefix}  if: \${{ !cancelled()${condition ? ` && (${condition})` : ""} }}`,
    `${prefix}  env:`,
    `${prefix}    PRM_SHOULD_PUBLISH: ${guarded.guard ? `\${{ ${guarded.guard} }}` : "'true'"}`,
    `${prefix}    PRM_PUBLISH_OUTCOME: \${{ steps.${publishId}.outcome }}`,
    `${prefix}  run: |`,
    `${prefix}    if [ "$PRM_SHOULD_PUBLISH" != "true" ]; then`,
    `${prefix}      echo "published=skipped" >> "$GITHUB_OUTPUT"`,
    `${prefix}      exit 0`,
    `${prefix}    fi`,
    `${prefix}    if [ "$PRM_PUBLISH_OUTCOME" = "success" ]; then`,
    `${prefix}      echo "published=true" >> "$GITHUB_OUTPUT"`,
    `${prefix}    else`,
    `${prefix}      echo "published=failed" >> "$GITHUB_OUTPUT"`,
    `${prefix}      exit 1`,
    `${prefix}    fi`,
  ];
  return [...lines, ...record];
}
