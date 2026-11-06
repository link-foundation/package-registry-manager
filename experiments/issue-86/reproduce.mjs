// Reproduce 0.27.0 defects without registry credentials or artifact uploads.
// Run from the repository root: node experiments/issue-86/reproduce.mjs
import { execFileSync } from "node:child_process";
import vm from "node:vm";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import nugetPage from "../../js/src/nuget-page.mjs";
import { verifyCentralPublication } from "../../js/src/central-publication.mjs";

const old = execFileSync("git", ["show", "88f91b9:js/src/nuget-page.mjs"], { encoding: "utf8" }).replace("export default ", "");
const expected = { username: "profile", package_owner: "Org", organization: "acme", repository: "lib", workflow: "publish.yml", package_ids: ["Org.Lib"], operations: ["package:pushversion"] };
const noop = () => {};
const draft = { PackageOwners: [{ Owner: "Org", CanPushExisting: true }], gitHub: {}, Valid: () => true, CreateOrEdit: noop };
for (const key of ["PackageOwner", "SelectedProvider", "PendingPolicyName", "PushScopeChecked", "PushScope", "UnlistScopeChecked", "ScopeSubjectsInput"]) draft[key] = noop;
for (const key of ["PendingRepositoryOwner", "PendingRepository", "PendingWorkflowFile", "PendingEnvironment"]) draft.gitHub[key] = noop;
const context = { document: { body: {}, querySelector: () => ({ textContent: "profile" }), querySelectorAll: () => [] }, ko: { dataFor: () => ({ Policies: () => [], NewPolicy: () => draft }) } };
assert.throws(() => vm.runInNewContext(`(${old})(${JSON.stringify(expected)},true)`, context), /StageScopeChecked/);
assert.equal(vm.runInNewContext(`(${nugetPage})(${JSON.stringify(expected)},true)`, context).submitted, true);
console.log("NuGet: 0.27.0 throws without StageScopeChecked; repaired adapter submits.");

const full = JSON.parse(readFileSync(new URL("../../tests/fixtures/gradle-publication/root.module.json", import.meta.url)));
const root = { ...full, variants: full.variants.filter((v) => !v["available-at"] || v["available-at"].module.endsWith("-jvm")) };
const platform = JSON.parse(readFileSync(new URL("../../tests/fixtures/gradle-publication/jvm.module.json", import.meta.url)));
const wrong = root.variants.filter((v) => v["available-at"]).map((v) => v["available-at"].module).map((entry) => `${entry.group}:${entry.module}`);
assert.ok(wrong.every((coordinate) => coordinate === "undefined:undefined"));
const result = await verifyCentralPublication({ plan: { package: { gradle: { multiplatform: true } } }, options: { fetch: async (url) => ({ status: 200, ok: true, json: async () => url.endsWith("links-notation-kotlin-0.31.0.module") ? root : platform }) } }, { coordinates: { group: "io.github.link-foundation", artifact: "links-notation-kotlin", version: "0.31.0" }, variants: ["io.github.link-foundation:links-notation-kotlin-jvm"] });
assert.equal(result.status, "verified");
console.log("Gradle: old mapping produces undefined:undefined; repaired graph verifies four referenced files.");
