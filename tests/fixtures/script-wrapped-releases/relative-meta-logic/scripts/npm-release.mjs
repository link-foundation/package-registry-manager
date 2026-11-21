import { spawnSync } from "node:child_process";

function runNpm(args) {
  return spawnSync("npm", args, { cwd: "js" });
}

if (process.argv[2] === "publish") {
  runNpm(["publish", "--provenance"]);
}
