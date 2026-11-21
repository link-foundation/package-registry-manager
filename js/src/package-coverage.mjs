import { readFile } from "node:fs/promises";
import path from "node:path";
import {
  executableLines,
  parseWorkflow,
  scriptReferences,
} from "./publishers.mjs";
import { commandPosition } from "./source-code.mjs";

function npmTarballTargets(commands) {
  const targets = [];
  for (const line of commands) {
    const text = line.replace(/\?\$\{\{[^}]+\}\}/g, "$VERSION");
    for (const match of text.matchAll(
      /\bnpm\s+publish\s+("[^"]*"|'[^']*'|[^\s;&|]+)/g,
    )) {
      const file = match[1].replace(/^['"]|['"]$/g, "");
      if (
        file.endsWith(".tgz") &&
        commandPosition(text.slice(0, match.index))
      ) {
        targets.push(path.posix.basename(file).slice(0, -4));
      }
    }
  }
  return targets;
}

function matchesNpmTarball(name, stem) {
  // npm pack flattens @scope/name to scope-name. Require a version-like
  // suffix so e.g. foo-extra-1.0.0.tgz does not cover foo.
  const expected = name.replace(/^@/, "").replace("/", "-");
  return (
    stem === expected ||
    (stem.startsWith(`${expected}-`) &&
      /^(?:v?\d|\$)/.test(stem.slice(expected.length + 1)))
  );
}

/** Keep executable steps for this manifest, preserving the job's OIDC identity. */
export function packageWorkflows(
  workflows,
  packageInfo,
  matchDirectories = true,
) {
  const directory = path.posix.dirname(packageInfo.manifest);
  const working = (lines) =>
    lines
      .map(
        (line) => /^\s*working-directory\s*:\s*['"]?([\w./-]+)/.exec(line)?.[1],
      )
      .filter(Boolean)
      .at(-1);
  // A step running in a parent directory publishes this package only when
  // it names it, as in `cargo publish -p name` or a shell function invoked
  // with the name, or reads its manifest by path.
  const ancestor = (cwd, child) =>
    child !== cwd && (cwd === "." || child.startsWith(`${cwd}/`));
  const escape = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const namesPackage = (text, cwd) => {
    const relative =
      cwd === "."
        ? packageInfo.manifest
        : packageInfo.manifest.slice(cwd.length + 1);
    return [packageInfo.name, relative]
      .filter(Boolean)
      .some((token) =>
        new RegExp(`(?<![\\w./-])${escape(token)}(?![\\w-])`).test(text),
      );
  };
  return workflows.map((workflow) => {
    const parsed = parseWorkflow(workflow.contents);
    const jobs = parsed.jobs.flatMap((job) => {
      const start = job.lines.findIndex((line) => /^\s*steps\s*:/.test(line));
      if (start < 0) {
        const called = job.lines.some((line) =>
          /^\s*uses\s*:\s*["']?\.\/\.github\/workflows\//.test(line),
        );
        // A reusable-workflow caller has no steps or working directory of its
        // own; it carries the OIDC identity of whatever its callee publishes,
        // and the callee is scoped separately.
        return called ? [`  ${job.name}:\n${job.lines.join("\n")}`] : [];
      }
      const prefix = job.lines.slice(0, start + 1);
      const blocks = [];
      for (const line of job.lines.slice(start + 1)) {
        if (/^\s*-\s+[\w-]+\s*:/.test(line)) {
          blocks.push([]);
        }
        if (blocks.length) {
          blocks.at(-1).push(line);
        }
      }
      const fallback = working(prefix) ?? working(parsed.header) ?? ".";
      const matching = blocks.filter((block) => {
        const commands = executableLines(block);
        if (!commands.length) {
          return false;
        }
        if (packageInfo.registry === "npm") {
          const targets = npmTarballTargets(commands);
          if (targets.length > 0) {
            return targets.some((stem) =>
              matchesNpmTarball(packageInfo.name, stem),
            );
          }
        }
        if (!matchDirectories) {
          return true;
        }
        const text = commands.join("\n");
        if (/--workspaces|--recursive|\bpublish\s+--all\b/.test(text)) {
          return true;
        }
        if (
          text.includes(`--workspace=${packageInfo.name}`) ||
          text.includes(`--workspace ${packageInfo.name}`)
        ) {
          return true;
        }
        const cd = /\bcd\s+['"]?([\w./-]+)/.exec(text)?.[1];
        const scripts = scriptReferences(commands);
        const inferred = scripts.find(
          (script) => directory !== "." && script.startsWith(`${directory}/`),
        )
          ? directory
          : undefined;
        const cwd =
          (working(block) ?? cd ?? inferred ?? fallback)
            .replace(/^\.\//, "")
            .replace(/\/$/, "") || ".";
        return (
          cwd === directory ||
          (ancestor(cwd, directory) && namesPackage(text, cwd))
        );
      });
      // Action-only steps (authentication exchanges such as
      // crates-io-auth-action) belong to the job that publishes this package.
      const kept = blocks.filter(
        (block) =>
          matching.includes(block) ||
          !block.some((line) => /^\s*-?\s*run\s*:/.test(line)),
      );
      return matching.length
        ? [`  ${job.name}:\n${prefix.join("\n")}\n${kept.flat().join("\n")}`]
        : [];
    });
    return {
      ...workflow,
      contents: `${parsed.header.join("\n")}\njobs:\n${jobs.join("\n")}`,
    };
  });
}

/** Warn when a second npm manifest re-exports a sibling but stops tracking it. */
export async function auditWrappers(root, packages) {
  const npm = packages.filter((item) => item.registry === "npm");
  for (const wrapper of npm) {
    const data = JSON.parse(
      (await readFile(path.join(root, wrapper.manifest), "utf8")).replace(
        /^\uFEFF/,
        "",
      ),
    );
    for (const target of npm.filter(
      (target) => target !== wrapper && data.dependencies?.[target.name],
    )) {
      if (wrapper.version !== target.version) {
        wrapper.warnings = [
          ...(wrapper.warnings ?? []),
          `wrapper ${wrapper.name}@${wrapper.version} differs from ${target.name}@${target.version}; update both manifests and the wrapper dependency for every release`,
        ];
      }
    }
  }
}
