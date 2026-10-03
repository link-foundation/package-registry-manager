import { readFile, realpath, stat } from "node:fs/promises";
import path from "node:path";

/** Registries whose trusted publisher is bound to a workflow file. */
export const TRUSTED_REGISTRIES = Object.freeze(["npm", "crates-io", "pypi"]);

const JOB_PATTERNS = new Map([
  [
    "npm",
    /\b(?:npm|pnpm)\s+(?:-r\s+|--recursive\s+)?(?:stage\s+)?publish\b|\byarn\s+npm\s+publish\b|\bchangeset\s+publish\b|changesets\/action\/publish@|JS-DevTools\/npm-publish/,
  ],
  [
    "crates-io",
    /\bcargo\s+(?:\+\S+\s+)?(?:workspaces\s+|ws\s+)?publish\b|katyo\/publish-crates/,
  ],
  [
    "pypi",
    /pypa\/gh-action-pypi-publish|\btwine\s+upload\b|\b(?:uv|poetry|hatch|flit|pdm)\s+publish\b/,
  ],
]);

// Scripts usually pass the subcommand as a separate argument, as in
// `spawnSync("npm", ["publish"])` or `Command::new("cargo")` followed by
// `.arg("publish")` within the next few lines.
const SCRIPT_WINDOW = 3;
const SCRIPT_PATTERNS = new Map([
  ["npm", [/\b(?:npm|pnpm|yarn)\b/, /["'`]publish["'`]/]],
  ["crates-io", [/\bcargo\b/, /["'`]publish["'`]/]],
  [
    "pypi",
    [/\b(?:twine|uv|poetry|hatch|flit|pdm)\b/, /["'`](?:upload|publish)["'`]/],
  ],
]);

const TOKEN_SECRETS = new Map([
  ["npm", ["NPM_TOKEN", "NPM_AUTH_TOKEN"]],
  [
    "crates-io",
    ["CARGO_TOKEN", "CARGO_REGISTRY_TOKEN", "CRATES_IO_TOKEN", "CRATES_TOKEN"],
  ],
  ["pypi", ["PYPI_TOKEN", "PYPI_API_TOKEN", "PYPI_PASSWORD", "TWINE_PASSWORD"]],
]);

const TRUSTED_PUBLISHING_HINTS = new Map([
  ["npm", "npm trusted publishing (`id-token: write` and npm 11.5.1 or newer)"],
  [
    "crates-io",
    "crates.io trusted publishing (`rust-lang/crates-io-auth-action` with `id-token: write`)",
  ],
  [
    "pypi",
    "PyPI trusted publishing (`pypa/gh-action-pypi-publish` with `id-token: write`)",
  ],
]);

const SCRIPT_REFERENCE =
  /^(?:\.{1,2}\/)?[\w@.-]+(?:\/[\w@.-]+)*\.(?:mjs|cjs|js|mts|cts|ts|rs|sh|bash|py)$/;
const RUN_SCRIPT =
  /\bnpm\s+run(?:-script)?\s+([\w:.-]+)|\b(?:pnpm|yarn|bun)\s+(?:run\s+)?([\w:.-]+)/g;
// `changesets/action` runs its `publish` (v1) or `publish-script` (v2) input.
const CHANGESETS_PUBLISH = /^\s*publish(?:-script)?\s*:\s*(.+?)\s*$/;
const LOCAL_WORKFLOW =
  /^\s*uses\s*:\s*["']?\.\/\.github\/workflows\/([\w.-]+\.ya?ml)/;
const SCRIPT_COMMENT = /^\s*(?:#|\/\/|\/?\*)/;
const COMMAND_PREFIX =
  /^\s*(?:(?:npx|bunx|exec|sudo|env|time|command|pnpm\s+exec|yarn|\w+=\S*)\s+)*$/;
const MAX_SCRIPT_BYTES = 1024 * 1024;

/**
 * Splits a GitHub Actions workflow into its jobs by indentation, since no YAML
 * parser is bundled. Full-line comments are dropped.
 */
export function parseWorkflow(contents) {
  const lines = contents.split(/\r?\n/).filter((line) => !/^\s*#/.test(line));
  const start = lines.findIndex((line) => /^jobs\s*:\s*$/.test(line));
  if (start === -1) {
    return { header: lines, jobs: [] };
  }
  let end = lines.findIndex((line, index) => index > start && /^\S/.test(line));
  end = end === -1 ? lines.length : end;
  const body = lines.slice(start + 1, end);
  const jobIndent = indentOf(body.find((line) => line.trim()) ?? "");
  const jobs = [];
  for (const line of body) {
    const header =
      line.trim() && indentOf(line) === jobIndent
        ? /^\s*["']?([\w-]+)["']?\s*:\s*$/.exec(line)
        : null;
    if (header) {
      jobs.push({ name: header[1], lines: [] });
    } else if (jobs.length > 0) {
      jobs.at(-1).lines.push(line);
    }
  }
  return {
    header: [...lines.slice(0, start), ...lines.slice(end)],
    jobs,
  };
}

/**
 * Finds the workflow jobs that publish to a registry with an OIDC token, so
 * the trusted publisher names the workflow CI actually publishes from.
 */
export async function detectPublisher(root, workflows, registry) {
  const parsedWorkflows = new Map(
    workflows.map((workflow) => [
      workflow.name,
      parseWorkflow(workflow.contents),
    ]),
  );
  const found = [];
  for (const workflow of workflows) {
    const parsed = parsedWorkflows.get(workflow.name);
    // Registries check the caller's identity, so a workflow that only runs
    // when another workflow calls it can never be the trusted publisher.
    if (onlyCalled(parsed.header)) {
      continue;
    }
    const workflowGrant = grantsIdToken(
      childBlock(parsed.header, 0, "permissions"),
    );
    for (const job of parsed.jobs) {
      const called = LOCAL_WORKFLOW.exec(
        job.lines.find((line) => LOCAL_WORKFLOW.test(line)) ?? "",
      )?.[1];
      const callee = called ? parsedWorkflows.get(called) : null;
      const publishes = callee
        ? await anyJobPublishes(root, callee, registry)
        : await jobPublishes(root, parsed, job, registry);
      if (!publishes) {
        continue;
      }
      const indent = childIndent(job.lines);
      found.push({
        workflow: workflow.name,
        job: job.name,
        idToken:
          grantsIdToken(childBlock(job.lines, indent, "permissions")) ??
          workflowGrant ??
          false,
        environment: environmentName(
          childBlock(job.lines, indent, "environment"),
        ),
      });
    }
  }
  const trusted = found.filter((item) => item.idToken);
  const files = [...new Set(trusted.map((item) => item.workflow))];
  if (files.length > 1) {
    return {
      workflow: null,
      jobs: [],
      environment: null,
      candidates: files,
      warnings: [
        `several workflows publish to ${registry} with id-token: write (${files.join(", ")}); pass --workflow <file> to choose the trusted publisher`,
      ],
    };
  }
  if (files.length === 1) {
    return described(files[0], trusted);
  }
  const untrusted = [...new Set(found.map((item) => item.workflow))];
  if (untrusted.length === 1) {
    const result = described(untrusted[0], found);
    result.warnings.unshift(
      `${untrusted[0]} publishes to ${registry} from ${jobList(found)} without \`id-token: write\`; add it so trusted publishing can mint a token`,
    );
    return result;
  }
  return {
    workflow: null,
    jobs: [],
    environment: null,
    candidates: untrusted,
    warnings:
      untrusted.length > 1
        ? [
            `several workflows publish to ${registry} (${untrusted.join(", ")}) and none grants id-token: write; pass --workflow <file> to choose the trusted publisher`,
          ]
        : [],
  };
}

function described(workflow, found) {
  const jobs = found.filter((item) => item.workflow === workflow);
  const environments = [...new Set(jobs.map((item) => item.environment))];
  const warnings = [];
  if (environments.length > 1) {
    warnings.push(
      `the publishing jobs in ${workflow} use different environments (${environments.map((item) => item ?? "none").join(", ")}); pass --environment <name> to choose one`,
    );
  }
  return {
    workflow,
    jobs: [...new Set(jobs.map((item) => item.job))],
    environment: environments.length === 1 ? environments[0] : null,
    candidates: [],
    warnings,
  };
}

function jobList(found) {
  return [...new Set(found.map((item) => item.job))]
    .map((job) => `job ${job}`)
    .join(", ");
}

/**
 * Lists the long-lived registry token secrets that workflows read, which
 * trusted publishing makes unnecessary.
 */
export function tokenSecrets(workflows, registry) {
  const names = TOKEN_SECRETS.get(registry) ?? [];
  const found = [];
  for (const workflow of workflows) {
    const lines = workflow.contents
      .split(/\r?\n/)
      .filter((line) => !/^\s*#/.test(line));
    for (const name of names) {
      if (
        lines.some((line) => new RegExp(`\\bsecrets\\.${name}\\b`).test(line))
      ) {
        found.push({ workflow: workflow.name, secret: name });
      }
    }
  }
  return found;
}

/** Explains how to replace a long-lived token secret with trusted publishing. */
export function tokenSecretWarning(registry, { workflow, secret }) {
  return `${workflow} reads secrets.${secret}; publish with ${TRUSTED_PUBLISHING_HINTS.get(registry)} and delete the long-lived token`;
}

/** Names of the long-lived token secrets that trusted publishing replaces. */
export function registryTokenSecrets(registry) {
  return TOKEN_SECRETS.get(registry) ?? [];
}

async function anyJobPublishes(root, parsed, registry) {
  for (const job of parsed.jobs) {
    if (await jobPublishes(root, parsed, job, registry)) {
      return true;
    }
  }
  return false;
}

async function jobPublishes(root, parsed, job, registry) {
  const pattern = JOB_PATTERNS.get(registry);
  if (publishingLine(job.lines, pattern)) {
    return true;
  }
  const directories = workingDirectories([...parsed.header, ...job.lines]);
  const scripts = [];
  for (const script of scriptReferences(job.lines)) {
    const contents = await readInside(root, directories, script);
    const lines = contents === null ? [] : codeLines(contents);
    if (scriptPublishes(lines, registry)) {
      return true;
    }
    scripts.push(...lines);
  }
  const commands = job.lines.some((line) => /changesets\/action@/.test(line))
    ? job.lines
        .map((line) => CHANGESETS_PUBLISH.exec(line)?.[1])
        .filter(Boolean)
        .map(unquote)
    : [];
  if (publishingLine(commands, pattern)) {
    return true;
  }
  // A package script may run from the job, its changesets command, or a
  // script, as in `$\`npm run changeset:publish\``.
  for (const line of [...job.lines, ...commands, ...scripts]) {
    for (const match of line.matchAll(RUN_SCRIPT)) {
      const manifest = await readInside(root, directories, "package.json");
      const command =
        manifest === null
          ? null
          : packageScript(manifest, match[1] ?? match[2]);
      if (command && publishingLine([command], pattern)) {
        return true;
      }
    }
  }
  return false;
}

function onlyCalled(header) {
  const block = childBlock(header, 0, "[\"']?on[\"']?");
  if (!block) {
    return false;
  }
  if (block.inline) {
    return /^\[?\s*workflow_call\s*\]?$/.test(unquote(block.inline));
  }
  const indent = childIndent(block.nested);
  const triggers = block.nested
    .filter((line) => line.trim() && indentOf(line) === indent)
    .map((line) =>
      line
        .trim()
        .replace(/^-\s*/, "")
        .replace(/\s*:.*$/, ""),
    );
  return (
    triggers.length > 0 && triggers.every((item) => item === "workflow_call")
  );
}

function publishingLine(lines, pattern) {
  return lines.some(
    (line) => pattern.test(line) && !line.includes("--dry-run"),
  );
}

function codeLines(contents) {
  return contents
    .split(/\r?\n/)
    .filter(
      (line) => !line.includes("--dry-run") && !SCRIPT_COMMENT.test(line),
    );
}

function scriptPublishes(lines, registry) {
  const pattern = new RegExp(JOB_PATTERNS.get(registry).source, "g");
  if (
    lines.some((line) =>
      [...line.matchAll(pattern)].some((match) =>
        commandPosition(line.slice(0, match.index)),
      ),
    )
  ) {
    return true;
  }
  const [program, subcommand] = SCRIPT_PATTERNS.get(registry);
  return lines.some(
    (line, index) =>
      program.test(line) &&
      lines
        .slice(index, index + SCRIPT_WINDOW)
        .some((nearby) => subcommand.test(nearby)),
  );
}

// A script mentions `cargo publish` in messages and comments too; it runs it
// only where the command starts a line, a shell command or a string literal.
function commandPosition(prefix) {
  const segment = prefix.split(/["'`;&|(]/).at(-1);
  return COMMAND_PREFIX.test(segment);
}

function packageScript(contents, name) {
  try {
    const command = JSON.parse(contents).scripts?.[name];
    return typeof command === "string" ? command : null;
  } catch {
    return null;
  }
}

/** Lists repository script paths that a job's lines run, such as `node x.mjs`. */
export function scriptReferences(lines) {
  const references = [];
  for (const line of lines) {
    for (const token of line.split(/[\s"'=;()|&]+/)) {
      if (SCRIPT_REFERENCE.test(token) && !references.includes(token)) {
        references.push(token);
      }
    }
  }
  return references;
}

function workingDirectories(lines) {
  const directories = [""];
  for (const line of lines) {
    const value = /^\s*working-directory\s*:\s*(.+?)\s*$/.exec(line)?.[1];
    const directory = value && unquote(value);
    if (
      directory &&
      !directory.includes("${{") &&
      !directories.includes(directory)
    ) {
      directories.push(directory);
    }
  }
  return directories;
}

async function readInside(root, directories, file) {
  // Compare real paths with the real root: a checkout reached through a
  // symlink (macOS's /var, a Windows short name) resolves elsewhere.
  const realRoot = await realpath(root).catch(() => null);
  if (!realRoot) {
    return null;
  }
  for (const directory of directories) {
    const candidate = path.resolve(root, directory, file);
    if (!isInside(root, candidate)) {
      continue;
    }
    try {
      const resolved = await realpath(candidate);
      if (!isInside(realRoot, resolved)) {
        continue;
      }
      const metadata = await stat(resolved);
      if (metadata.isFile() && metadata.size <= MAX_SCRIPT_BYTES) {
        return await readFile(resolved, "utf8");
      }
    } catch {
      // A reference that does not exist in the checkout is not a script.
    }
  }
  return null;
}

function isInside(root, candidate) {
  const relative = path.relative(root, candidate);
  return (
    relative !== "" && !relative.startsWith("..") && !path.isAbsolute(relative)
  );
}

function indentOf(line) {
  return line.length - line.trimStart().length;
}

function childIndent(lines) {
  const first = lines.find((line) => line.trim());
  return first === undefined ? 0 : indentOf(first);
}

function childBlock(lines, indent, key) {
  const expression = new RegExp(`^\\s*${key}\\s*:\\s*(.*)$`);
  const start = lines.findIndex(
    (line) => indentOf(line) === indent && expression.test(line),
  );
  if (start === -1) {
    return null;
  }
  const nested = [];
  for (const line of lines.slice(start + 1)) {
    if (line.trim() && indentOf(line) <= indent) {
      break;
    }
    nested.push(line);
  }
  return { inline: stripComment(expression.exec(lines[start])[1]), nested };
}

function grantsIdToken(block) {
  if (!block) {
    return null;
  }
  const inline = unquote(block.inline);
  if (inline) {
    return (
      inline === "write-all" ||
      (inline.startsWith("{") && /\bid-token\s*:\s*['"]?write\b/.test(inline))
    );
  }
  return block.nested.some((line) =>
    /^\s*id-token\s*:\s*['"]?write\b/.test(line),
  );
}

function environmentName(block) {
  if (!block) {
    return null;
  }
  let value = block.inline;
  if (value.startsWith("{")) {
    value = /\bname\s*:\s*([^,}]+)/.exec(value)?.[1] ?? "";
  } else if (!value) {
    value =
      block.nested
        .map((line) => /^\s*name\s*:\s*(.+)$/.exec(line)?.[1])
        .find(Boolean) ?? "";
  }
  value = unquote(stripComment(value));
  return value && !value.includes("${{") ? value : null;
}

function stripComment(value) {
  return value.replace(/\s+#.*$/, "").trim();
}

function unquote(value) {
  return value.trim().replace(/^(['"])(.*)\1$/, "$2");
}
