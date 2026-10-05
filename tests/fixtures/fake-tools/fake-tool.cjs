// Stand-in for node, npm, npx, cargo, git, gh, the default-browser openers
// (open, xdg-open), and the default-browser queries (defaults, xdg-settings,
// both answering Firefox) in the end-to-end tests of both implementations.
// It records every argument vector and keeps just enough state (session,
// trusted publisher) for a resumed run to behave like the real tools.
// Scenarios: FAKE_TFA=off (no 2FA), FAKE_PACK_WARNINGS (npm drops the bin
// while packing), FAKE_BIN_FAILS (the installed bin exits 1), FAKE_BIN_SILENT
// (the installed bin exits 0 without output), FAKE_PACK_JSON=object (npm 12's
// `npm pack --json`, an object keyed by package name), and
// FAKE_RELEASE_RUN=failed-publish (the last release failed with E404), and
// FAKE_REPO_TOKEN_NAMES (comma-separated repository secret names; default NPM_TOKEN),
// FAKE_LEGACY_LOGIN=<n> (the first n web logins fall back to Username:), and
// FAKE_EXPIRED_PUBLISH=<n> (the first n publish approvals expire), and
// FAKE_PAGES=missing|legacy|workflow (the GitHub Pages site; default workflow),
// and FAKE_TRUST_GITHUB=fail (npm trust github fails), and
// FAKE_CARGO_PUBLISH=fail (cargo publish fails). cargo records the
// CARGO_REGISTRY_TOKEN of its environment, so a test can check which child
// received the first-publish token. Like npm 11, npm trust
// asks for a 2FA approval, and `npm trust list --json` holds its URL back, so
// the approval expires and npm fails with E404.
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
  ...(tool === "cargo"
    ? { registryToken: process.env.CARGO_REGISTRY_TOKEN ?? null }
    : {}),
});
const command = args.join(" ");
const sidecar = (tarball) => tarball + ".json";
// Counts the calls of a scenario and reports whether this one still fails.
const failsAgain = (scenario) => {
  const file = state + "/" + scenario + ".count";
  const count = fs.existsSync(file) ? Number(fs.readFileSync(file, "utf8")) : 0;
  fs.writeFileSync(file, String(count + 1));
  return count < Number(process.env[scenario] || 0);
};
if (tool === "xdg-settings") console.log("firefox.desktop");
if (tool === "defaults")
  console.log(
    '(\n    {\n    LSHandlerRoleAll = "org.mozilla.firefox";\n    LSHandlerURLScheme = https;\n}\n)',
  );
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
    if (failsAgain("FAKE_LEGACY_LOGIN")) {
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
    const packed = {
      filename: "pipeline-app-0.1.0.tgz",
      version: "0.1.0",
      size: 120,
      unpackedSize: 300,
      entryCount: 1,
      files: [{ path: "package.json", size: 300 }],
    };
    console.log(
      JSON.stringify(
        process.env.FAKE_PACK_JSON === "object"
          ? { [manifest.name]: packed }
          : [packed],
      ),
    );
  }
  if (args[0] === "publish") {
    console.log("Authenticate your account at:");
    console.log("https://www.npmjs.com/auth/cli/fake");
    if (failsAgain("FAKE_EXPIRED_PUBLISH")) {
      console.error("npm error Invalid response from web login endpoint");
      process.exit(1);
    }
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
        : process.env.FAKE_BIN_SILENT
          ? ""
          : `console.log(${JSON.stringify(manifest.version)});`;
      fs.writeFileSync(
        path.join(bin, name),
        `#!${process.execPath}\n${body}\n`,
        { mode: 0o755 },
      );
    }
  }
}
if (tool === "cargo") {
  // cargo login stores the token it reads from the terminal in CARGO_HOME.
  const credentials = path.join(process.env.CARGO_HOME, "credentials.toml");
  if (args[0] === "login")
    fs.writeFileSync(credentials, '[registry]\ntoken = "cio-fake"\n');
  if (args[0] === "logout") fs.rmSync(credentials, { force: true });
  if (
    command === "publish" &&
    process.env.FAKE_CARGO_PUBLISH === "fail"
  ) {
    console.error("error: failed to publish to registry");
    process.exit(101);
  }
}
if (tool === "npx" && args.includes("trust")) {
  if (args.includes("--json")) {
    console.error("npm error code E404");
    console.error(
      "npm error 404 Not Found - GET https://registry.npmjs.org/-/v1/done?authId=fake",
    );
    process.exit(1);
  }
  console.log("Authenticate your account at:");
  console.log("https://www.npmjs.com/auth/cli/fake-trust");
  if (command.includes("trust github")) {
    if (process.env.FAKE_TRUST_GITHUB === "fail") process.exit(1);
    set("trusted", true);
    console.log(
      "Trust configuration created successfully for pipeline-app with the following settings:",
    );
  }
  if (command.includes("trust github") || flag("trusted"))
    console.log(
      "\ntype: \u001b[32mgithub\u001b[39m\nid: \u001b[32mfake-id\u001b[39m\nfile: \u001b[32mrelease.yml\u001b[39m\nrepository: \u001b[32macme/pipeline-app\u001b[39m\n",
    );
  else console.log("No trust configurations found for package (pipeline-app)");
}
if (tool === "git") {
  if (args[0] === "worktree" && args[1] === "add") {
    fs.mkdirSync(args[3], { recursive: true });
    for (const entry of ["package.json", "Cargo.toml", "bin"])
      if (fs.existsSync(entry))
        fs.cpSync(entry, path.join(args[3], entry), { recursive: true });
  }
  if (args[0] === "worktree" && args[1] === "remove")
    fs.rmSync(args[3], { recursive: true, force: true });
}
if (tool === "gh") {
  if (/^api (?:-X (?:POST|PUT) )?repos\/[^ ]+\/pages\b/.test(command)) {
    if (command.includes("-X ")) set("pages", true);
    const site = flag("pages") ? "workflow" : process.env.FAKE_PAGES;
    if (site === "missing") {
      console.log('{"message":"Not Found","status":"404"}');
      console.error("gh: Not Found (HTTP 404)");
      process.exit(1);
    }
    console.log(
      JSON.stringify({
        build_type: site === "legacy" ? "legacy" : "workflow",
        html_url: "https://acme.github.io/demo/",
      }),
    );
  }
  if (command.startsWith("secret list"))
    console.log(
      JSON.stringify(
        (process.env.FAKE_REPO_TOKEN_NAMES ?? "NPM_TOKEN")
          .split(",")
          .map((name) => ({ name })),
      ),
    );
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
