import path from "node:path";
import { readFile } from "node:fs/promises";
import { ANSI } from "./auth-urls.mjs";
/**
 * npm's warnings from `npm pack`, such as fields it auto-corrected or removed
 * from the packed package.json, without npm's advice to run its fixer, which
 * rewrites package.json in place.
 */
export function packWarnings(stderr) {
  return String(stderr ?? "")
    .split(/\r?\n/)
    .map((line) => line.replaceAll(ANSI, "").trim())
    .filter((line) => /^npm warn/i.test(line))
    .map((line) =>
      line.replace(/\s*Please run "npm pkg fix"[^.]*\.?/i, "").trim(),
    )
    .filter((line) => !/npm pkg fix/i.test(line));
}

/**
 * The first package in `npm pack --json` output: an array before npm 12, an
 * object keyed by package name since.
 */
export function packedEntry(output) {
  const parsed = JSON.parse(output);
  const [packed] = Array.isArray(parsed) ? parsed : Object.values(parsed ?? {});
  if (!packed) {
    throw new Error("npm pack printed no package");
  }
  return packed;
}

/**
 * Top-level repository directories of development files: CI, tests, and
 * editor and build state, which belong in no published package.
 */
const DEVELOPMENT_DIRECTORIES = new Set([
  ".changeset",
  ".github",
  ".husky",
  ".idea",
  ".vscode",
  "__tests__",
  "coverage",
  "experiments",
  "target",
  "test",
  "tests",
]);

/**
 * The development directories a packed tarball contains, as `name/`, which
 * a package.json without a `files` allowlist lets in.
 */
export function developmentDirectories(packed) {
  const found = new Set();
  for (const file of packed.files ?? []) {
    const [top, ...rest] = String(file.path).split("/");
    if (rest.length > 0 && DEVELOPMENT_DIRECTORIES.has(top)) {
      found.add(`${top}/`);
    }
  }
  return [...found].sort();
}

export function reportPackWarnings(stderr) {
  const warnings = packWarnings(stderr);
  if (warnings.length === 0) {
    return;
  }
  console.log("  npm warned while packing:");
  for (const line of warnings) {
    console.log(`    ${line}`);
  }
  if (warnings.some((line) => /corrected|invalid and removed/i.test(line))) {
    console.log(
      "  npm changed the packed package.json; correct these fields in package.json itself.",
    );
  }
}

export function binEntries(manifest) {
  if (typeof manifest.bin === "string") {
    return { [String(manifest.name).replace(/^@[^/]+\//, "")]: manifest.bin };
  }
  return manifest.bin && typeof manifest.bin === "object" ? manifest.bin : {};
}

export async function readManifest(file) {
  return JSON.parse(await readFile(file, "utf8"));
}

/** Validate the inspected identity and packed contents before bootstrap. */
export function recordPack(session, output) {
  const packed = packedEntry(output);
  if (
    packed.name !== session.plan.package.name ||
    (session.plan.package.version &&
      packed.version !== session.plan.package.version)
  ) {
    throw new Error(
      "bootstrap ref package identity/version differs from inspection; inspect the selected ref again before publication",
    );
  }
  for (const file of packed.files ?? []) {
    console.log(`  ${String(file.size).padStart(8)}  ${file.path}`);
  }
  console.log(
    `  ${packed.filename}: ${packed.size} bytes packed, ${packed.unpackedSize} bytes unpacked, ${packed.entryCount} files`,
  );
  const development = developmentDirectories(packed);
  if (development.length > 0) {
    throw new Error(
      `the package would publish repository development files (${development.join(", ")}); add a "files" allowlist to package.json, commit it to the published ref, and re-run. Nothing was published.`,
    );
  }
  session.values.tarball = path.join(
    session.values.pack_destination,
    packed.filename,
  );
  session.values.version = packed.version;
}
