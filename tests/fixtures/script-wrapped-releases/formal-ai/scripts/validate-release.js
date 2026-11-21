import { execFileSync } from "node:child_process";

execFileSync("npm", [
  "publish",
  "--dry-run",
]);
