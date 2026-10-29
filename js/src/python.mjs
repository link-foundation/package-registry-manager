import { readdir, mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { exec } from "./exec.mjs";

// requires-python describes interpreter releases. Reject unsupported or
// prerelease syntax rather than silently choosing an incompatible executable.
function release(value) {
  return /^\d+(?:\.\d+){0,2}$/.test(value)
    ? value.split(".").map(Number)
    : null;
}

function compare(left, right) {
  for (let i = 0; i < 3; i += 1) {
    const difference = (left[i] ?? 0) - (right[i] ?? 0);
    if (difference) {
      return Math.sign(difference);
    }
  }
  return 0;
}

/** Tests stable interpreter versions against requires-python specifiers. */
export function pythonMatches(version, requirement = "") {
  const actual = release(version);
  if (!actual) {
    return false;
  }
  if (!requirement.trim()) {
    return true;
  }
  return requirement.split(",").every((clause) => {
    const match =
      /^\s*(~=|==|!=|<=|>=|<|>)\s*(\d+(?:\.\d+){0,2})(\.\*)?\s*$/.exec(clause);
    if (!match) {
      return false;
    }
    const [, operator, text, wildcard] = match;
    const expected = release(text);
    if (wildcard) {
      const equal = expected.every((part, i) => actual[i] === part);
      return operator === "==" ? equal : operator === "!=" && !equal;
    }
    const order = compare(actual, expected);
    switch (operator) {
      case "==":
        return order === 0;
      case "!=":
        return order !== 0;
      case ">=":
        return order >= 0;
      case "<=":
        return order <= 0;
      case ">":
        return order > 0;
      case "<":
        return order < 0;
      case "~=": {
        if (expected.length < 2) {
          return false;
        }
        const upper = expected.slice(0, -1);
        upper[upper.length - 1] += 1;
        return order >= 0 && compare(actual, upper) < 0;
      }
      default:
        return false;
    }
  });
}

/** Selects the first installed compatible interpreter, preferring PATH defaults. */
export function choosePython(tools, requirement) {
  return tools.find((tool) => pythonMatches(tool.version, requirement)) ?? null;
}

async function candidatePrograms() {
  const candidates = new Set(["python", "python3"]);
  for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
    if (!directory) {
      continue;
    }
    const entries = await readdir(directory).catch(() => []);
    for (const entry of entries.sort()) {
      if (/^python3\.\d+(?:\.exe)?$/.test(entry)) {
        candidates.add(entry);
      }
    }
  }
  return [...candidates].slice(0, 32);
}

/** Probes only interpreter versions, without loading manifests or package code. */
export async function probePython(options = {}) {
  const candidates = options.candidates ?? (await candidatePrograms());
  const probe =
    options.probe ??
    (async (program, args) => {
      if (options.verbose) {
        console.error(`+ ${program} ${args.join(" ")}`);
      }
      return exec(program, args, {
        capture: true,
        mirror: false,
        stdin: "ignore",
      });
    });
  const tools = [];
  for (const program of candidates) {
    try {
      const result = await probe(program, ["--version"]);
      const version = /^Python (\d+\.\d+\.\d+)\s*$/.exec(
        String(result.stdout ?? "").trim(),
      )?.[1];
      if (result.code === 0 && version) {
        tools.push({ program, version });
      }
    } catch {
      // An unavailable candidate is not an installed interpreter.
    }
  }
  return tools;
}

/** Adds a Python prerequisite and selects the executable used by the build step. */
export function applyPython(plan, tools) {
  const build = plan.steps.find((step) => step.id === "build-package");
  if (plan.registry !== "pypi" || !build || !tools) {
    return;
  }
  const requirement = plan.package.requires_python ?? "";
  const selected = choosePython(tools, requirement);
  const item = {
    id: "python",
    title: "Python",
    detected: selected
      ? `${selected.program} (${selected.version})`
      : "no compatible interpreter found",
    required: `Install Python satisfying ${requirement || "the project requirements"} with venv support; the tool installs build in a temporary environment`,
    ok: Boolean(selected),
  };
  plan.prerequisites = [
    ...(plan.prerequisites ?? []).filter((entry) => entry.id !== "python"),
    item,
  ];
  if (selected) {
    build.command = { program: selected.program, args: ["-m", "build"] };
  }
}

/** Provision the build frontend without modifying the selected interpreter. */
export async function buildPythonDistribution(program, cwd, options = {}) {
  const directory = await mkdtemp(path.join(os.tmpdir(), "prm-python-build-"));
  const executable = path.join(
    directory,
    process.platform === "win32" ? "Scripts/python.exe" : "bin/python",
  );
  const run =
    options.run ??
    ((file, args) =>
      exec(file, args, { cwd, capture: true, mirror: true, stdin: "ignore" }));
  try {
    for (const [file, args] of [
      [program, ["-m", "venv", directory]],
      [
        executable,
        ["-m", "pip", "install", "--disable-pip-version-check", "build"],
      ],
      [executable, ["-m", "build"]],
    ]) {
      if (options.verbose) {
        console.error(`+ ${file} ${args.join(" ")}`);
      }
      const result = await run(file, args);
      if (result.code !== 0) {
        throw new Error(
          `Python build preparation or build failed (${file}, status ${result.code}); ensure the selected Python supports venv and pip`,
        );
      }
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}
