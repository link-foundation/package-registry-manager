// Runs the JavaScript and Rust CLIs on the same fixtures and reports any
// difference in their JSON output. Usage (after `cargo build` in rust/):
//   node experiments/compare-implementations.mjs
import { execFile } from "node:child_process";
import { cp, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { isDeepStrictEqual } from "node:util";

const run = promisify(execFile);
const root = path.resolve(import.meta.dirname, "..");
const rust = path.join(root, "rust/target/debug/package-registry-manager");
const js = path.join(root, "js/src/cli.mjs");

const server = createServer((request, response) => {
  response.writeHead(404, { "content-type": "application/json" });
  response.end("{}");
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const env = {
  ...process.env,
  PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY: `${base}/npm`,
  PACKAGE_REGISTRY_MANAGER_CRATES_IO_API: `${base}/crates`,
  PACKAGE_REGISTRY_MANAGER_PYPI_API: `${base}/pypi`,
  PACKAGE_REGISTRY_MANAGER_DOCKER_HUB_API: `${base}/docker`,
};

const temporary = await mkdtemp(path.join(os.tmpdir(), "prm-compare-"));
let failures = 0;
try {
  for (const [fixture, remote] of [
    ["polyglot", "git@github.com:acme/polyglot.git"],
    ["pipeline-template", "https://github.com/acme/pipeline-app.git"],
  ]) {
    const repository = path.join(temporary, fixture);
    await cp(path.join(root, "tests/fixtures", fixture), repository, {
      recursive: true,
    });
    await mkdir(path.join(repository, ".git"));
    await writeFile(
      path.join(repository, ".git/config"),
      `[remote "origin"]\n\turl = ${remote}\n`,
    );
    for (const args of [
      ["inspect", "--offline"],
      ["plan", "--offline"],
      ["plan", "--offline", "--verify-release"],
      ["inspect"],
      ["plan"],
      ["plan", "--verify-release"],
    ]) {
      const full = ["--repository", repository, "--format", "json", ...args];
      const [fromJs, fromRust] = await Promise.all([
        run(process.execPath, [js, ...full], { env }),
        run(rust, full, { env }),
      ]);
      const same = isDeepStrictEqual(
        JSON.parse(fromJs.stdout),
        JSON.parse(fromRust.stdout),
      );
      failures += same ? 0 : 1;
      console.log(`${same ? "same" : "DIFFERENT"}  ${fixture} ${args.join(" ")}`);
      if (!same) {
        await writeFile(`${temporary}.js.json`, fromJs.stdout);
        await writeFile(`${temporary}.rust.json`, fromRust.stdout);
        console.log(`  diff ${temporary}.js.json ${temporary}.rust.json`);
      }
    }
  }
} finally {
  server.close();
  await rm(temporary, { recursive: true, force: true });
}
process.exitCode = failures === 0 ? 0 : 1;
