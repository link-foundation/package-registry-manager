// Runs the macOS default-browser query against the fake `defaults` tool.
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { detectDefaultBrowser } from "../js/src/default-browser.mjs";

const fake = fileURLToPath(new URL("../tests/fixtures/fake-tools/fake-tool.cjs", import.meta.url));
const bin = await mkdtemp(path.join(os.tmpdir(), "prm-darwin-"));
const file = path.join(bin, "defaults");
await writeFile(file, `#!${process.execPath}\n${await readFile(fake, "utf8")}`);
await chmod(file, 0o755);
process.env.PATH = `${bin}${path.delimiter}${process.env.PATH}`;
process.env.FAKE_STATE = bin;
console.log(await detectDefaultBrowser({ platform: "darwin", verbose: true }));
