import { readFileSync } from "node:fs";
import { workflowMigrationProposal } from "../../js/src/workflow-migration.mjs";
const root = process.argv[2] ?? "/tmp/ln-src";
import { readdirSync } from "node:fs";
const names = readdirSync(`${root}/.github/workflows`).filter((n) => /\.ya?ml$/.test(n));
const workflows = names.map((name) => ({ name, contents: readFileSync(`${root}/.github/workflows/${name}`, "utf8") }));
const p = await workflowMigrationProposal(root, [{ registry: "nuget", workflow: "js.yml", name: "Link.Foundation.Links.Notation", manifest: "csharp/Link.Foundation.Links.Notation/Link.Foundation.Links.Notation.csproj" }], workflows);
console.log(process.argv[3] ? Object.keys(p.files).map((f) => f + ": " + /id-token: write/.test(p.files[f])).join("\n") : p.files["publish-csharp.yml"]);
