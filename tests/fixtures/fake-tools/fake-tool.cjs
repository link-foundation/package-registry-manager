// Stand-in for node, npm, npx, git, gh, and the default-browser openers
// (open, xdg-open) in the end-to-end bootstrap tests of both implementations.
// It records every argument vector and keeps just enough state (session,
// trusted publisher) for a resumed run to behave like the real tools.
// Scenarios: FAKE_TFA=off (no 2FA), FAKE_PACK_WARNINGS (npm drops the bin
// while packing), FAKE_BIN_FAILS (the installed bin exits 1), and
// FAKE_RELEASE_RUN=failed-publish (the last release failed with E404).
const fs = require("node:fs");
const path = require("node:path");
const tool = path.basename(__filename);
const args = process.argv.slice(2);
const state = process.env.FAKE_STATE;
const log = (entry) =>
  fs.appendFileSync(state + "/log.jsonl", JSON.stringify(entry) + "\n");
const flag = (name) => fs.existsSync(state + "/" + name);
const set = (name, on) =>
  on
    ? fs.writeFileSync(state + "/" + name, "")
    : fs.rmSync(state + "/" + name, { force: true });
log({
  argv: [tool, ...args],
  cwd: process.cwd(),
  shim: String(process.env.NODE_OPTIONS || "").includes("--require"),
  tty: Boolean(process.stdout.isTTY),
});
const command = args.join(" ");
const sidecar = (tarball) => tarball + ".json";
if (tool === "node" && args[0] === "--version")
  console.log(process.env.FAKE_NODE_VERSION || "v20.19.4");
if (tool === "npm") {
  if (args[0] === "--version") console.log("11.6.2");
  if (args[0] === "whoami") process.exit(flag("session") ? 0 : 1);
  if (command === "profile get --json") {
    if (!flag("session")) process.exit(1);
    console.log(
      JSON.stringify({
        name: "octo",
        tfa:
          process.env.FAKE_TFA === "off"
            ? false
            : { pending: false, mode: "auth-and-writes" },
      }),
    );
  }
  if (args[0] === "login") {
    console.log("Login at:");
    console.log("https://www.npmjs.com/login?next=/login/cli/fake");
    if (process.env.FAKE_LEGACY_LOGIN) {
      // npm's fallback when the web login fails: a prompt without a newline.
      process.stdout.write("Username: ");
      setTimeout(() => process.exit(1), 60000);
      return;
    }
    set("session", true);
  }
  if (args[0] === "logout") set("session", false);
  if (args[0] === "pack") {
    const tarball =
      args[args.indexOf("--pack-destination") + 1] + "/pipeline-app-0.1.0.tgz";
    fs.writeFileSync(tarball, "");
    // The packed package/package.json, read back by the fake install.
    const manifest = JSON.parse(fs.readFileSync("package.json", "utf8"));
    if (process.env.FAKE_PACK_WARNINGS) {
      delete manifest.bin;
      for (const line of [
        'npm warn pack npm auto-corrected some errors in your package.json when publishing.  Please run "npm pkg fix" to address these errors.',
        "npm warn pack errors corrected:",
        'npm warn pack "bin[pipeline-app]" script name bin/cli.js was invalid and removed',
      ])
        console.error(line);
    }
    fs.writeFileSync(sidecar(tarball), JSON.stringify(manifest));
    console.log(
      JSON.stringify([
        {
          filename: "pipeline-app-0.1.0.tgz",
          version: "0.1.0",
          size: 120,
          unpackedSize: 300,
          entryCount: 1,
          files: [{ path: "package.json", size: 300 }],
        },
      ]),
    );
  }
  if (args[0] === "publish") {
    console.log("Authenticate your account at:");
    console.log("https://www.npmjs.com/auth/cli/fake");
  }
  if (args[0] === "pkg") console.log("{}");
  if (args[0] === "install") {
    const prefix = args[args.indexOf("--prefix") + 1];
    const manifest = JSON.parse(fs.readFileSync(sidecar(args.at(-1)), "utf8"));
    const root = path.join(prefix, "node_modules", manifest.name);
    const bin = path.join(prefix, "node_modules", ".bin");
    fs.mkdirSync(root, { recursive: true });
    fs.mkdirSync(bin, { recursive: true });
    fs.writeFileSync(path.join(root, "package.json"), JSON.stringify(manifest));
    for (const name of Object.keys(manifest.bin || {})) {
      const body = process.env.FAKE_BIN_FAILS
        ? "process.exit(1);"
        : `console.log(${JSON.stringify(manifest.version)});`;
      fs.writeFileSync(
        path.join(bin, name),
        `#!${process.execPath}\n${body}\n`,
        { mode: 0o755 },
      );
    }
  }
}
if (tool === "npx") {
  if (command.includes("trust github")) set("trusted", true);
  if (command.includes("trust list") && flag("trusted"))
    console.log(
      JSON.stringify({
        type: "github",
        file: "release.yml",
        repository: "acme/pipeline-app",
      }),
    );
}
if (tool === "git") {
  if (args[0] === "worktree" && args[1] === "add") {
    fs.mkdirSync(args[3], { recursive: true });
    for (const entry of ["package.json", "bin"])
      if (fs.existsSync(entry))
        fs.cpSync(entry, path.join(args[3], entry), { recursive: true });
  }
  if (args[0] === "worktree" && args[1] === "remove")
    fs.rmSync(args[3], { recursive: true, force: true });
}
if (tool === "gh") {
  if (command.startsWith("secret list"))
    console.log(JSON.stringify([{ name: "NPM_TOKEN" }]));
  if (command === "auth status --json hosts")
    console.log(
      JSON.stringify({
        hosts: {
          "github.com": [
            {
              state: "success",
              active: true,
              host: "github.com",
              login: "octo",
              scopes: "repo, workflow",
            },
          ],
        },
      }),
    );
  if (command.startsWith("run list")) {
    const failed = process.env.FAKE_RELEASE_RUN === "failed-publish";
    const id = failed ? 42 : 41;
    console.log(
      JSON.stringify([
        {
          databaseId: id,
          conclusion: failed ? "failure" : "success",
          status: "completed",
          url: `https://github.com/acme/pipeline-app/actions/runs/${id}`,
        },
      ]),
    );
  }
  if (command.startsWith("run view") && command.includes("--log-failed")) {
    console.log("release\tPublish\tnpm error code E404");
    console.log(
      "release\tPublish\tnpm error 404 Not Found - PUT https://registry.npmjs.org/pipeline-app",
    );
  }
}
