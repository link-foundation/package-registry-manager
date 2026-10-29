import { publishingPolicy } from "./publishing-state.mjs";
import { fallbackSteps } from "./workflow-fallback.mjs";
import {
  parseWorkflow,
  jobPublishes,
  registryTokenSecrets,
  executableLines,
} from "./publishers.mjs";

// Reviewed v1.0.5 commit. Keep both implementations on the same immutable action.
export const CRATES_AUTH_ACTION =
  "rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18";
const LOCAL = /^\s*uses:\s*["']?\.\/\.github\/workflows\/([\w.-]+\.ya?ml)/;
const indent = (line) => line.search(/\S/);
const supported = new Set(["crates-io", "npm", "pypi"]);
const keys = (registry) => [
  ...registryTokenSecrets(registry),
  ...(registry === "npm"
    ? ["NODE_AUTH_TOKEN"]
    : registry === "pypi"
      ? ["TWINE_PASSWORD", "TWINE_USERNAME"]
      : []),
];

function endOf(lines, start) {
  const level = indent(lines[start]);
  let end = start + 1;
  while (
    end < lines.length &&
    (indent(lines[end]) > level ||
      indent(lines[end]) < 0 ||
      lines[end].trimStart().startsWith("#"))
  ) {
    end++;
  }
  return end;
}

export function removeCredentials(lines, names) {
  const result = [...lines];
  for (let index = result.length - 1; index >= 0; index--) {
    const key = /^\s*([\w-]+):/.exec(result[index])?.[1];
    if (names.has(key)) {
      result.splice(index, endOf(result, index) - index);
    }
  }
  for (let index = result.length - 1; index >= 0; index--) {
    if (
      /^\s*(?:env|secrets|with):\s*$/.test(result[index]) &&
      result
        .slice(index + 1, endOf(result, index))
        .every((line) => !line.trim() || line.trimStart().startsWith("#"))
    ) {
      result.splice(index, 1);
    }
  }
  return result;
}

function grant(lines, level) {
  const result = [...lines];
  const start = result.findIndex(
    (line) => indent(line) === level && /^\s*permissions:/.test(line),
  );
  if (start < 0) {
    return [
      `${" ".repeat(level)}permissions:`,
      `${" ".repeat(level + 2)}contents: read`,
      `${" ".repeat(level + 2)}id-token: write`,
      ...result,
    ];
  }
  if (/permissions:\s*write-all\s*$/.test(result[start])) {
    return result;
  }
  if (!/permissions:\s*(?:#.*)?$/.test(result[start])) {
    throw new Error(
      "migration requires a mapping permissions block; expand inline permissions first",
    );
  }
  const end = endOf(result, start);
  const token = result.findIndex(
    (line, index) => index > start && index < end && /^\s*id-token:/.test(line),
  );
  const child = result
    .slice(start + 1, end)
    .find((line) => line.trim() && !line.trimStart().startsWith("#"));
  const entry = `${" ".repeat(child ? indent(child) : level + 2)}id-token: write`;
  if (token >= 0) {
    result[token] = entry;
  } else {
    result.splice(start + 1, 0, entry);
  }
  return result;
}

function withEnv(lines, key, value, stepLevel) {
  const result = [...lines];
  const level = stepLevel + 2;
  const start = result.findIndex(
    (line) => indent(line) === level && /^\s*env:\s*$/.test(line),
  );
  const entry = `${" ".repeat(level + 2)}${key}: ${value}`;
  if (start < 0) {
    result.push(`${" ".repeat(level)}env:`, entry);
  } else {
    result.splice(start + 1, 0, entry);
  }
  return result;
}

function grantJob(lines, level, parsed) {
  if (
    !lines.some(
      (line) => indent(line) === level && /^\s*permissions:/.test(line),
    )
  ) {
    const start = parsed.header.findIndex((line) => /^permissions:/.test(line));
    if (start >= 0) {
      if (/permissions:\s*write-all\s*$/.test(parsed.header[start])) {
        return lines;
      }
      const inherited = parsed.header
        .slice(start, endOf(parsed.header, start))
        .map((line) => `${" ".repeat(level)}${line}`);
      return grant([...inherited, ...lines], level);
    }
  }
  return grant(lines, level);
}

async function convertJob(root, parsed, lines, registry) {
  if (
    publishingPolicy([...parsed.header, ...lines], registry) ===
    "oidc-with-token-fallback"
  ) {
    return lines;
  }
  const names = new Set(keys(registry));
  if (lines.some((line) => /^\s*env:\s*[^\s#]/.test(line))) {
    throw new Error(
      "expand inline environment and secret mappings before migration",
    );
  }
  const level = Math.min(
    ...lines
      .filter((line) => line.trim() && !line.trimStart().startsWith("#"))
      .map(indent),
  );
  if (lines.some((line) => LOCAL.test(line))) {
    return grantJob(lines, level, parsed);
  }
  const start = lines.findIndex(
    (line) => indent(line) === level && /^\s*steps:\s*$/.test(line),
  );
  if (start < 0) {
    throw new Error("publishing job has no editable steps block");
  }
  const end = endOf(lines, start);
  const positions = [];
  for (let index = start + 1; index < end; index++) {
    if (/^\s*-\s+(?:name|run|uses|id):/.test(lines[index])) {
      positions.push(index);
    }
  }
  if (!positions.length) {
    throw new Error("publishing job has no editable steps");
  }
  const stepLevel = indent(lines[positions[0]]);
  const steps = positions.map((position, index) =>
    lines.slice(position, positions[index + 1] ?? end),
  );
  const edits = [];
  const auths = new Map();
  let first = -1;
  for (const [index, step] of steps.entries()) {
    const publishes = await jobPublishes(
      root,
      parsed,
      { lines: [`${" ".repeat(level)}steps:`, ...step] },
      registry,
    );
    if (
      registry === "crates-io" &&
      step.some((line) => /rust-lang\/crates-io-auth-action@/.test(line))
    ) {
      continue;
    }
    const credential = step.some((line) =>
      [...names].some((name) => new RegExp(`^\\s*${name}:`).test(line)),
    );
    const consumer =
      credential ||
      executableLines(step).some(
        (line) =>
          [...names].some((name) => line.includes(name)) ||
          /preflight.*(?:crates|cargo)|(?:crates|cargo).*preflight/.test(line),
      );
    let updated = [...step];
    if (publishes || consumer) {
      if (first < 0) {
        first = index;
      }
      if (registry === "crates-io") {
        const guard = step.findIndex(
          (line) => indent(line) === stepLevel + 2 && /^\s*if:/.test(line),
        );
        const condition =
          guard < 0
            ? []
            : step
                .slice(guard, endOf(step, guard))
                .filter((line) => line.trim());
        const identity = JSON.stringify(condition);
        let auth = auths.get(identity);
        if (!auth) {
          const id = auths.size ? `prm_auth_${auths.size + 1}` : "prm_auth";
          auth = { id, condition };
          auths.set(identity, auth);
          edits.push([
            "",
            `${" ".repeat(stepLevel)}- name: Authenticate with crates.io (trusted publishing)`,
            `${" ".repeat(stepLevel + 2)}uses: ${CRATES_AUTH_ACTION} # v1.0.5`,
            `${" ".repeat(stepLevel + 2)}id: ${id}`,
            `${" ".repeat(stepLevel + 2)}continue-on-error: true`,
            ...condition,
            "",
          ]);
        }
        const fallback = `\${{ steps.${
          auth.id
        }.outputs.token || secrets.CARGO_REGISTRY_TOKEN || secrets.CARGO_TOKEN }}`;
        const usedKeys = [...names].filter((name) =>
          [...parsed.header, ...lines.slice(0, start), ...step].some((line) =>
            new RegExp(`\\b(?:env\\.)?${name}\\b`).test(line),
          ),
        );
        updated = removeCredentials(updated, names).map((line) =>
          line.replace(
            /\s+--token(?:=|\s+)(?:["']?\$\{\{\s*secrets\.[\w]+\s*\}\}["']?|["']?\$\{?(?:CARGO_TOKEN|CARGO_REGISTRY_TOKEN)\}?["']?)/g,
            "",
          ),
        );
        for (const key of new Set(["CARGO_REGISTRY_TOKEN", ...usedKeys])) {
          updated = withEnv(updated, key, fallback, stepLevel);
        }
      }
      if (registry === "npm" && publishes) {
        updated = withEnv(
          updated,
          "NPM_CONFIG_PROVENANCE",
          "'true'",
          stepLevel,
        );
      }
      if (
        registry === "pypi" &&
        publishes &&
        !step.some((line) => /pypa\/gh-action-pypi-publish@/.test(line))
      ) {
        const run = step.find((line) => /^\s*-?\s*run:/.test(line));
        const upload =
          /^\s*(-?\s*)run:\s*(?:python(?:3)? -m )?twine upload ([\w./-]+)\/\*\s*$/.exec(
            run ?? "",
          );
        if (
          !upload ||
          upload[2].split("/").includes("..") ||
          step.some((line) => /^\s*working-directory:/.test(line))
        ) {
          throw new Error(
            "PyPI migration requires a simple twine upload DIRECTORY/* without a working-directory, or a pypa publishing action",
          );
        }
        updated = updated
          .filter((line) => !/^\s*shell:/.test(line))
          .map((line) =>
            line === run
              ? line.replace(
                  /run:.*/,
                  "uses: pypa/gh-action-pypi-publish@release/v1",
                )
              : line,
          );
        updated.push(
          `${" ".repeat(stepLevel + 2)}with:`,
          `${" ".repeat(stepLevel + 4)}packages-dir: '${upload[2]}/'`,
        );
      }
    }
    if (publishes && ["npm", "pypi"].includes(registry)) {
      edits.push(
        ...fallbackSteps(
          updated,
          registry,
          names,
          stepLevel,
          index,
          removeCredentials,
        ),
      );
    } else {
      edits.push(updated);
    }
  }
  if (first < 0) {
    throw new Error("cannot locate publishing step for migration");
  }
  const prefix = " ".repeat(stepLevel);
  if (registry === "npm") {
    edits.splice(first, 0, [
      `${prefix}- uses: actions/setup-node@v6`,
      `${prefix}  with:`,
      `${prefix}    node-version: '22'`,
      `${prefix}- run: npm install --global npm@^11`,
    ]);
  }
  return grantJob(
    [
      ...lines.slice(0, start),
      lines[start],
      ...edits.flat(),
      ...lines.slice(end),
    ],
    level,
    parsed,
  );
}

/** Convert existing jobs in every publishing path, preserving commands and caller identity. */
export async function workflowMigrationProposal(root, packages, workflows) {
  const parsed = new Map(
    workflows.map((item) => [item.name, parseWorkflow(item.contents)]),
  );
  const targets = new Map();
  const mark = (name, job, registry) => {
    const jobs = targets.get(name) ?? new Map();
    const registries = jobs.get(job) ?? new Set();
    registries.add(registry);
    jobs.set(job, registries);
    targets.set(name, jobs);
  };
  const visit = async (name, registry, seen = new Set()) => {
    if (seen.has(name)) {
      throw new Error("cyclic reusable workflows cannot be migrated");
    }
    const workflow = parsed.get(name);
    if (!workflow) {
      throw new Error(`missing reusable workflow ${name}`);
    }
    let found = false;
    for (const job of workflow.jobs) {
      const called = job.lines.find((line) => LOCAL.test(line));
      const publishes = called
        ? await visit(LOCAL.exec(called)[1], registry, new Set([...seen, name]))
        : await jobPublishes(root, workflow, job, registry);
      const tokenCheck =
        registry === "crates-io" &&
        executableLines(job.lines).some((line) =>
          /preflight|CARGO_(?:REGISTRY_)?TOKEN/.test(line),
        ) &&
        job.lines.some((line) => /CARGO_(?:REGISTRY_)?TOKEN|crates/.test(line));
      if (publishes || tokenCheck) {
        found = true;
        mark(name, job.name, registry);
      }
    }
    return found;
  };
  for (const registry of new Set(packages.map((item) => item.registry))) {
    if (!supported.has(registry)) {
      throw new Error(`workflow conversion is unavailable for ${registry}`);
    }
    // Convert all release entry points, including manual releases in reusable files.
    for (const workflow of workflows) {
      await visit(workflow.name, registry);
    }
  }
  const files = {};
  for (const workflow of workflows) {
    const jobs = targets.get(workflow.name);
    if (!jobs) {
      continue;
    }
    const lines = workflow.contents.split(/\r?\n/);
    const start = lines.findIndex((line) => /^jobs:\s*$/.test(line));
    const end = endOf(lines, start);
    const jobLevel = Math.min(
      ...lines
        .slice(start + 1, end)
        .filter((line) => line.trim() && !line.trimStart().startsWith("#"))
        .map(indent),
    );
    const positions = [];
    for (let index = start + 1; index < end; index++) {
      if (
        indent(lines[index]) === jobLevel &&
        /^\s*["']?[\w-]+["']?:\s*$/.test(lines[index])
      ) {
        positions.push(index);
      }
    }
    const body = [];
    for (const [index, position] of positions.entries()) {
      const name = /^\s*["']?([\w-]+)["']?:/.exec(lines[position])[1];
      let content = lines.slice(position + 1, positions[index + 1] ?? end);
      for (const registry of jobs.get(name) ?? []) {
        content = await convertJob(
          root,
          parsed.get(workflow.name),
          content,
          registry,
        );
      }
      body.push(lines[position], ...content);
    }
    const names = new Set(
      [...jobs.values()].flatMap((registries) => [...registries].flatMap(keys)),
    );
    // Reusable credentials remain optional fallback inputs; preserve caller mappings/inherit.
    const header = [...lines.slice(0, start)];
    for (let i = 0; i < header.length; i++) {
      const key = /^\s*([\w-]+):\s*$/.exec(header[i])?.[1];
      if (!names.has(key)) {
        continue;
      }
      const end = endOf(header, i);
      for (let j = i + 1; j < end; j++) {
        header[j] = header[j].replace(/required:\s*true/, "required: false");
      }
    }
    const inlineCall = header.findIndex((line) =>
      /^["']?on["']?:\s*(?:workflow_call|\[workflow_call\])\s*$/.test(line),
    );
    if (inlineCall >= 0) {
      header.splice(inlineCall, 1, "on:", "  workflow_call:");
    }
    const call = header.findIndex((line) =>
      /^\s+workflow_call:\s*$/.test(line),
    );
    if (call >= 0) {
      const callEnd = endOf(header, call);
      const callLevel = indent(header[call]);
      let secrets = header.findIndex(
        (line, index) =>
          index > call &&
          index < callEnd &&
          indent(line) === callLevel + 2 &&
          /^\s*secrets:/.test(line),
      );
      if (secrets < 0) {
        secrets = call + 1;
        header.splice(secrets, 0, `${" ".repeat(callLevel + 2)}secrets:`);
      }
      const existing = header.slice(secrets + 1, endOf(header, secrets));
      for (const name of names) {
        if (existing.some((line) => new RegExp(`^\\s*${name}:`).test(line))) {
          continue;
        }
        header.splice(
          secrets + 1,
          0,
          `${" ".repeat(callLevel + 4)}${name}:`,
          `${" ".repeat(callLevel + 6)}required: false`,
        );
      }
    }
    const converted = [
      ...header,
      lines[start],
      ...body,
      ...lines.slice(end),
    ].join("\n");
    files[workflow.name] = `${converted.trimEnd()}\n`;
  }
  if (!Object.keys(files).length) {
    throw new Error("no publishing jobs were found for migration");
  }
  return { workflow: packages[0].workflow, files };
}
