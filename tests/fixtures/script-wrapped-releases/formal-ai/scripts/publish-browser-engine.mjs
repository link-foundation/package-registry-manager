import { execFileSync } from "node:child_process";

export function runNpm(argumentsList, directory) {
  return execFileSync("npm", argumentsList, { cwd: directory });
}

export function publishBrowserEngine(directory = process.cwd(), run = runNpm) {
  const archive = "link-assistant-formal-ai-engine-1.0.0.tgz";
  run(["publish", archive, "--provenance"], directory);
}

publishBrowserEngine();
