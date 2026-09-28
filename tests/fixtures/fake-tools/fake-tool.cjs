// Stand-in for npm, npx, git, and gh in the end-to-end bootstrap tests of both
// implementations. It records every argument vector and keeps just enough state
// (session, trusted publisher) for a resumed run to behave like the real tools.
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
if (tool === "npm") {
  if (args[0] === "whoami") process.exit(flag("session") ? 0 : 1);
  if (args[0] === "login") {
    console.log("Login at:");
    console.log("https://www.npmjs.com/login?next=/login/cli/fake");
    set("session", true);
  }
  if (args[0] === "logout") set("session", false);
  if (args[0] === "pack") {
    fs.writeFileSync(
      args[args.indexOf("--pack-destination") + 1] + "/pipeline-app-0.1.0.tgz",
      "",
    );
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
  if (args[0] === "worktree" && args[1] === "add")
    fs.mkdirSync(args[3], { recursive: true });
  if (args[0] === "worktree" && args[1] === "remove")
    fs.rmSync(args[3], { recursive: true, force: true });
}
if (tool === "gh" && command.startsWith("secret list"))
  console.log(JSON.stringify([{ name: "NPM_TOKEN" }]));
