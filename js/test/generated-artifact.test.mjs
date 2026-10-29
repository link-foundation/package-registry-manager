import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { execFile } from "node:child_process";
import { prepareGeneratedArtifact } from "../src/generated-artifact.mjs";
const run = promisify(execFile);

test("bootstrap executes the real generator and validates the selected artifact", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "prm-alias-"));
  try {
    await mkdir(path.join(root, "scripts"));
    await writeFile(
      path.join(root, "package.json"),
      JSON.stringify({ name: "original", version: "1.2.3" }),
    );
    await writeFile(
      path.join(root, "package-lock.json"),
      JSON.stringify({
        name: "original",
        version: "1.2.3",
        lockfileVersion: 3,
        packages: { "": { name: "original", version: "1.2.3" } },
      }),
    );
    await writeFile(
      path.join(root, "scripts/prepare.mjs"),
      `import { mkdir,writeFile } from 'node:fs/promises'; import path from 'node:path'; const directory=path.join(process.argv[3],'alias'); await mkdir(directory,{recursive:true}); await writeFile(path.join(directory,'package.json'),JSON.stringify({name:'alias',version:'1.2.3'}));`,
    );
    const session = {
      plan: {
        package: {
          name: "alias",
          version: "1.2.3",
          manifest: "package.json",
          generated_alias: { generator: "scripts/prepare.mjs" },
        },
      },
      values: { worktree: root, pack_destination: root },
      capture: async ({ command, cwd }) => {
        await run(command.program, command.args, { cwd });
        return { code: 0 };
      },
    };
    await prepareGeneratedArtifact(session);
    assert.equal(
      JSON.parse(
        await readFile(
          path.join(session.values.generated_package, "package.json"),
        ),
      ).name,
      "alias",
    );
    session.plan.package.version = "2.0.0";
    await assert.rejects(
      prepareGeneratedArtifact(session),
      /identity\/version differs/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
