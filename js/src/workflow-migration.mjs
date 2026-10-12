import {
  parseWorkflow,
  jobPublishes,
  registryTokenSecrets,
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

function removeCredentials(lines, names) {
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
  const names = new Set(keys(registry));
  if (lines.some((line) => /^\s*(?:env|secrets):\s*[^\s#]/.test(line))) {
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
    return grantJob(removeCredentials(lines, names), level, parsed);
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
  const guards = [];
  let first = -1;
  for (const [index, step] of steps.entries()) {
    const publishes = await jobPublishes(
      root,
      parsed,
      { lines: [`${" ".repeat(level)}steps:`, ...step] },
      registry,
    );
    const credential = step.some((line) =>
      [...names].some((name) => new RegExp(`^\\s*${name}:`).test(line)),
    );
    let updated = removeCredentials(step, names);
    if (publishes || credential) {
      if (first < 0) {
        first = index;
      }
      if (registry === "crates-io") {
        const guard = step.findIndex(
          (line) => indent(line) === stepLevel + 2 && /^\s*if:/.test(line),
        );
        guards.push(guard < 0 ? [] : step.slice(guard, endOf(step, guard)));
        updated = withEnv(
          updated,
          "CARGO_REGISTRY_TOKEN",
          "${{ steps.prm_auth.outputs.token }}",
          stepLevel,
        );
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
        step.some((line) => /pypa\/gh-action-pypi-publish@/.test(line))
      ) {
        updated = removeCredentials(
          updated,
          new Set([...names, "password", "user"]),
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
    edits.push(updated);
  }
  if (first < 0) {
    throw new Error("cannot locate publishing step for migration");
  }
  const prefix = " ".repeat(stepLevel);
  if (registry === "crates-io") {
    if (
      guards.some(
        (guard) => JSON.stringify(guard) !== JSON.stringify(guards[0]),
      )
    ) {
      throw new Error(
        "different publishing conditions require separate authentication steps; review this job before migration",
      );
    }
    if (
      lines.some((line) =>
        /\bid:\s*prm_auth\b|rust-lang\/crates-io-auth-action@/.test(line),
      )
    ) {
      throw new Error(
        "existing crates.io auth step needs review before converting token wiring",
      );
    }
    edits.splice(first, 0, [
      `${prefix}- name: Authenticate with crates.io (trusted publishing)`,
      `${prefix}  uses: ${CRATES_AUTH_ACTION}`,
      `${prefix}  id: prm_auth`,
      ...guards[0],
    ]);
  } else if (registry === "npm") {
    edits.splice(first, 0, [
      `${prefix}- uses: actions/setup-node@v6`,
      `${prefix}  with:`,
      `${prefix}    node-version: '22'`,
      `${prefix}- run: npm install --global npm@^11`,
    ]);
  }
  return grantJob(
    [
      ...removeCredentials(lines.slice(0, start), names),
      lines[start],
      ...edits.flat(),
      ...removeCredentials(lines.slice(end), names),
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
      if (publishes) {
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
    for (const job of parsed
      .get(workflow.name)
      .jobs.filter((job) => !jobs.has(job.name))) {
      if (
        job.lines.some((line) =>
          [...names].some((name) =>
            new RegExp(`\\b(?:secrets|env)\\.${name}\\b`).test(line),
          ),
        )
      ) {
        throw new Error(
          "token is also read by a non-publishing job; preserve that consumer before migration",
        );
      }
    }
    // Remove workflow_call declarations after converting every affected publishing job.
    const header = removeCredentials(lines.slice(0, start), names);
    const converted = [
      ...header,
      lines[start],
      ...body,
      ...lines.slice(end),
    ].join("\n");
    if (
      [...names].some((name) =>
        new RegExp(`\\bsecrets\\.${name}\\b`).test(converted),
      ) ||
      /\bsecrets\s*\[|\bsecrets:\s*inherit\b/.test(converted)
    ) {
      throw new Error(
        "remaining legacy or inherited secret references require review before migration",
      );
    }
    files[workflow.name] = `${converted.trimEnd()}\n`;
  }
  if (!Object.keys(files).length) {
    throw new Error("no publishing jobs were found for migration");
  }
  return { workflow: packages[0].workflow, files };
}
