// Tests the package the way users get it: packed, installed from the tarball,
// and started through the node_modules/.bin symlinks npm creates (#22).
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));
const packageDirectory = path.resolve(here, "..");
const fixture = path.resolve(here, "../../tests/fixtures/polyglot");
// Windows links bins as .cmd shims instead of symlinks.
const skip = process.platform === "win32" && "npm links bins as .cmd shims";
const npm = "npm";
let directory;
let bins;

before(
  async () => {
    if (skip) {
      return;
    }
    directory = await mkdtemp(path.join(os.tmpdir(), "prm-installed-"));
    const { stdout } = await execute(
      npm,
      ["pack", "--ignore-scripts", "--json", "--pack-destination", directory],
      { cwd: packageDirectory },
    );
    const parsed = JSON.parse(stdout);
    const [packed] = Array.isArray(parsed) ? parsed : Object.values(parsed);
    // The dependencies come from npm's cache, filled by npm ci.
    await execute(
      npm,
      [
        "install",
        "--no-save",
        "--no-package-lock",
        "--no-audit",
        "--no-fund",
        "--ignore-scripts",
        "--prefer-offline",
        "--prefix",
        path.join(directory, "install"),
        path.join(directory, packed.filename),
      ],
      { cwd: directory },
    );
    bins = path.join(directory, "install", "node_modules", ".bin");
  },
  { timeout: 240000 },
);

after(async () => {
  if (directory) {
    await rm(directory, { recursive: true, force: true });
  }
});

const manifest = JSON.parse(
  await readFile(path.join(packageDirectory, "package.json"), "utf8"),
);

for (const name of Object.keys(manifest.bin)) {
  test(`the installed ${name} prints its usage`, { skip }, async () => {
    const { stdout } = await execute(path.join(bins, name), ["--help"]);
    assert.match(stdout, /^Usage: package-registry-manager/);
  });

  test(`the installed ${name} prints its version`, { skip }, async () => {
    const { stdout } = await execute(path.join(bins, name), ["--version"]);
    assert.equal(stdout, `package-registry-manager ${manifest.version}\n`);
  });

  test(`the installed ${name} inspects a repository`, { skip }, async () => {
    const { stdout } = await execute(path.join(bins, name), [
      "inspect",
      "--offline",
      "--repository",
      fixture,
    ]);
    assert.match(stdout, /^Repository: /m);
    assert.match(stdout, /@acme\/widgets/);
  });

  test(
    `the installed ${name} reports a missing command`,
    { skip },
    async () => {
      await assert.rejects(execute(path.join(bins, name), []), (error) => {
        assert.equal(error.code, 1);
        assert.match(error.stderr, /expected exactly one command/);
        return true;
      });
    },
  );
}
