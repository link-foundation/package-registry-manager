import assert from "node:assert/strict";
import { test } from "node:test";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { readFileSync } from "node:fs";
import { provisionSigning } from "../src/central-gpg.mjs";

const execute = promisify(execFile);
const gpgAvailable = await execute("gpg", ["--version"]).then(
  () => true,
  () => false,
);

test(
  "GnuPG creates a one-year RSA4096 key, signs, independently verifies and reuses proof",
  { skip: !gpgAvailable, timeout: 90000 },
  async () => {
    // Only the external keyserver is replaced; generation, encryption, export,
    // detached signing and independent verification use the real GnuPG binary.
    let publicKey;
    const calls = [];
    const run = async (program, args, input = "") => {
      calls.push([program, ...args]);
      const send = args.indexOf("--send-keys");
      const receive = args.indexOf("--recv-keys");
      if (send >= 0) {
        const home = args[args.indexOf("--homedir") + 1];
        publicKey = await run("gpg", [
          "--homedir",
          home,
          "--armor",
          "--export",
          args[send + 1],
        ]);
        assert.match(publicKey, /BEGIN PGP PUBLIC KEY BLOCK/);
        assert.doesNotMatch(publicKey, /PRIVATE KEY/);
        return "";
      }
      if (receive >= 0) {
        const home = args[args.indexOf("--homedir") + 1];
        return run(
          "gpg",
          ["--homedir", home, "--batch", "--import"],
          publicKey,
        );
      }
      return new Promise((resolve, reject) => {
        const child = execFile(
          program,
          args,
          { maxBuffer: 2 * 1024 * 1024 },
          (error, stdout) =>
            error
              ? reject(new Error("test GPG command failed"))
              : resolve(stdout),
        );
        child.stdin.on("error", () => {});
        child.stdin.end(input);
      });
    };
    const before = Date.now();
    const result = await provisionSigning(
      {
        generate: true,
        identity: "Central acme/library <publishing@example.invalid>",
      },
      "acme/library",
      null,
      run,
    );
    assert.match(result.private_key, /BEGIN PGP PRIVATE KEY BLOCK/);
    assert.ok(result.passphrase.length >= 32);
    assert.ok(Date.parse(result.proof.expires_at) > before + 364 * 86400000);
    assert.ok(Date.parse(result.proof.expires_at) < before + 367 * 86400000);
    assert.ok(
      calls.some((args) => args.includes("rsa4096") && args.includes("1y")),
    );
    const repeated = await provisionSigning(
      {},
      "acme/library",
      result.proof,
      run,
    );
    assert.equal(repeated.private_key, undefined);
    assert.equal(repeated.proof.fingerprint, result.proof.fingerprint);
    await assert.rejects(
      provisionSigning({}, "other/repository", result.proof, run),
      /identity mismatch/,
    );
    const homes = calls
      .filter(([program]) => program === "gpg")
      .map((args) => args[args.indexOf("--homedir") + 1])
      .filter(Boolean);
    for (const home of new Set(homes)) {
      assert.throws(() => readFileSync(home), { code: "ENOENT" });
    }
  },
);

test("shared Central browser and GPG adapters ship identically in both runtimes", () => {
  for (const file of [
    "central-page.mjs",
    "central-token-page.mjs",
    "central-gpg.mjs",
  ]) {
    assert.equal(
      readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8"),
      readFileSync(new URL(`../../rust/src/${file}`, import.meta.url), "utf8"),
    );
  }
});
