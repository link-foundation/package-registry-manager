// Read-only regression probe for #56. Run from the repository after npm ci --prefix js.
import { githubServices } from "../js/src/github.mjs";
import { scanRepositories } from "../js/src/organization-scan.mjs";

const github = githubServices();
const scan = await scanRepositories({
  org: "link-foundation",
  offline: true,
  github: {
    ...github,
    repos: {
      list: async () => [
        { full_name: "link-foundation/start", default_branch: "main" },
      ],
      files: (...args) => github.repos.files(...args),
    },
  },
});
const item = scan.repositories[0];
if (item.error) {
  throw new Error(item.error);
}
const crate = item.inspection.packages.find(
  (pkg) => pkg.name === "start-command",
);
console.log(
  JSON.stringify(
    {
      repository: item.repository,
      package: crate.name,
      workflow: crate.workflow,
      environment: crate.environment,
      jobs: crate.workflow_jobs,
      token_secrets: crate.token_secrets,
    },
    null,
    2,
  ),
);
if (crate.workflow !== "js.yml") {
  throw new Error("start-command reusable caller was missed");
}
