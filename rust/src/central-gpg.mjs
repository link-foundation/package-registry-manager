import { spawn } from "node:child_process";
import { mkdtemp, readFile, writeFile, rm, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { randomBytes } from "node:crypto";

/** Run exact GPG arguments; private input/output and diagnostics are never logged. */
async function command(program, args, input = "") {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args, { stdio: ["pipe", "pipe", "pipe"] });
    const output = [];
    let length = 0;
    child.stdout.on("data", (chunk) => {
      length += chunk.length;
      if (length > 2 * 1024 * 1024) {
        child.kill();
      } else {
        output.push(chunk);
      }
    });
    child.stderr.resume();
    child.on("error", () =>
      reject(new Error("GnuPG is unavailable; no signing values were logged")),
    );
    child.on("close", (code) =>
      code === 0
        ? resolve(Buffer.concat(output).toString())
        : reject(
            new Error("GnuPG operation failed; private diagnostics withheld"),
          ),
    );
    child.stdin.on("error", () => {});
    child.stdin.end(input);
  });
}

/** Isolated GPG signing round trip and independent public-key verification. */
export async function provisionSigning(
  settings,
  slug,
  priorProof,
  run = command,
) {
  const directory = await mkdtemp(path.join(tmpdir(), "prm-central-gpg-"));
  const signer = path.join(directory, "signer");
  const verifier = path.join(directory, "verifier");
  await mkdir(signer, { mode: 0o700 });
  await mkdir(verifier, { mode: 0o700 });
  const keyserver =
    settings.keyserver ??
    priorProof?.keyserver ??
    "hkps://keyserver.ubuntu.com";
  if (
    !["hkps://keyserver.ubuntu.com", "hkps://keys.openpgp.org"].includes(
      keyserver,
    )
  ) {
    await rm(directory, { recursive: true, force: true });
    throw new Error("use a supported Central public keyserver");
  }
  const gpg = (home, args, input) =>
    run("gpg", ["--homedir", home, "--batch", "--no-tty", ...args], input);
  const listing = async (home, fingerprint) => {
    const result = await gpg(home, [
      "--with-colons",
      "--list-keys",
      fingerprint,
    ]);
    const records = result.split("\n").map((line) => line.split(":"));
    const primary = records.find((row) => row[0] === "pub");
    const actual = records.find((row) => row[0] === "fpr")?.[9];
    const expires = primary?.[6] ? Number(primary[6]) * 1000 : null;
    if (
      actual !== fingerprint ||
      !primary ||
      /[red]/.test(primary[1]) ||
      !primary[11]?.includes("s") ||
      !expires ||
      expires <= Date.now()
    ) {
      throw new Error(
        "signing primary fingerprint, capability or expiry is invalid",
      );
    }
    return new Date(expires).toISOString();
  };
  try {
    let fingerprint =
      settings.fingerprint?.toUpperCase() ?? priorProof?.fingerprint;
    let privateKey;
    let passphrase;
    let proof = priorProof;
    if (!proof) {
      passphrase = settings.passphrase_env
        ? process.env[settings.passphrase_env]
        : undefined;
      if (settings.private_key_file || fingerprint) {
        if (passphrase === undefined) {
          throw new Error(
            "reuse requires passphrase_env for the existing signing material",
          );
        }
        if (settings.private_key_file) {
          privateKey = await readFile(settings.private_key_file, "utf8");
        } else {
          const args = [
            "--batch",
            "--no-tty",
            ...(settings.key_home ? ["--homedir", settings.key_home] : []),
            "--pinentry-mode",
            "loopback",
            "--passphrase-fd",
            "0",
            "--armor",
            "--export-secret-keys",
            fingerprint,
          ];
          privateKey = await run("gpg", args, `${passphrase}\n`);
        }
        await gpg(signer, ["--import"], privateKey);
      } else if (settings.generate === true) {
        if (
          !settings.identity ||
          /[\r\n]/.test(settings.identity) ||
          !settings.identity.includes(slug)
        ) {
          throw new Error("a repository-specific signing identity is required");
        }
        passphrase = randomBytes(32).toString("base64url");
        await gpg(
          signer,
          [
            "--pinentry-mode",
            "loopback",
            "--passphrase-fd",
            "0",
            "--quick-generate-key",
            settings.identity,
            "rsa4096",
            "sign",
            "1y",
          ],
          `${passphrase}\n`,
        );
      } else {
        throw new Error(
          "explicit signing reuse or authorized generation settings are required",
        );
      }
      const keys = await gpg(signer, ["--with-colons", "--list-secret-keys"]);
      const fingerprints = keys
        .split("\n")
        .filter((line) => line.startsWith("fpr:"))
        .map((line) => line.split(":")[9]);
      fingerprint ??= fingerprints[0];
      if (
        !/^[A-F0-9]{40,64}$/.test(fingerprint ?? "") ||
        !fingerprints.includes(fingerprint)
      ) {
        throw new Error(
          "selected private signing fingerprint was not imported",
        );
      }
      const expiresAt = await listing(signer, fingerprint);
      const payload = `package-registry-manager Central signing proof for ${slug}\n${randomBytes(32).toString("hex")}\n`;
      const dataFile = path.join(directory, "proof.txt");
      const signatureFile = path.join(directory, "proof.asc");
      await writeFile(dataFile, payload, { mode: 0o600 });
      await gpg(
        signer,
        [
          "--pinentry-mode",
          "loopback",
          "--passphrase-fd",
          "0",
          "--local-user",
          `${fingerprint}!`,
          "--armor",
          "--detach-sign",
          "--output",
          signatureFile,
          dataFile,
        ],
        `${passphrase}\n`,
      );
      await gpg(signer, ["--verify", signatureFile, dataFile]);
      // send-keys exports public packets only; never upload the private armor.
      await gpg(signer, [
        "--keyserver",
        keyserver,
        "--keyserver-options",
        "timeout=15",
        "--send-keys",
        fingerprint,
      ]);
      privateKey ??= await gpg(
        signer,
        [
          "--pinentry-mode",
          "loopback",
          "--passphrase-fd",
          "0",
          "--armor",
          "--export-secret-keys",
          fingerprint,
        ],
        `${passphrase}\n`,
      );
      proof = {
        repository: slug,
        fingerprint,
        expires_at: expiresAt,
        payload,
        signature: await readFile(signatureFile, "utf8"),
        keyserver,
      };
    }
    if (
      proof.repository !== slug ||
      fingerprint !== proof.fingerprint ||
      !proof.payload.startsWith(
        `package-registry-manager Central signing proof for ${slug}\n`,
      ) ||
      !/^[A-F0-9]{40,64}$/.test(fingerprint ?? "")
    ) {
      throw new Error("signing proof identity mismatch");
    }
    await gpg(verifier, [
      "--keyserver",
      keyserver,
      "--keyserver-options",
      "timeout=15",
      "--recv-keys",
      fingerprint,
    ]);
    const expiresAt = await listing(verifier, fingerprint);
    if (expiresAt !== proof.expires_at) {
      throw new Error("public key expiry changed; review signing settings");
    }
    const dataFile = path.join(directory, "verify.txt");
    const signatureFile = path.join(directory, "verify.asc");
    await writeFile(dataFile, proof.payload, { mode: 0o600 });
    await writeFile(signatureFile, proof.signature, { mode: 0o600 });
    const status = await gpg(verifier, [
      "--status-fd",
      "1",
      "--verify",
      signatureFile,
      dataFile,
    ]);
    if (!status.includes(`[GNUPG:] VALIDSIG ${fingerprint} `)) {
      throw new Error("independent signature fingerprint verification failed");
    }
    return { proof, private_key: privateKey, passphrase };
  } finally {
    for (const home of [signer, verifier]) {
      await run("gpgconf", ["--homedir", home, "--kill", "gpg-agent"]).catch(
        () => {},
      );
    }
    await rm(directory, { recursive: true, force: true });
  }
}
