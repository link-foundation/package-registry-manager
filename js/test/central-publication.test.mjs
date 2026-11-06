import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { verifyCentralPublication } from "../src/central-publication.mjs";

const fixture = (name) =>
  JSON.parse(
    readFileSync(
      new URL(
        `../../tests/fixtures/gradle-publication/${name}.module.json`,
        import.meta.url,
      ),
    ),
  );
const group = "io.github.link-foundation";
const artifact = "links-notation-kotlin";
const version = "0.31.0";
const root = fixture("root");
const jvm = fixture("jvm");
const jvmRoot = {
  ...root,
  variants: root.variants.filter(
    (v) => !v["available-at"] || v["available-at"].module.endsWith("-jvm"),
  ),
};
const step = {
  coordinates: { group, artifact, version },
  variants: [`${group}:${artifact}-jvm`],
};
async function verify(doc = jvmRoot, change = () => null, expected = step) {
  const calls = [];
  const result = await verifyCentralPublication(
    {
      plan: { package: { gradle: { multiplatform: true } } },
      options: {
        fetch: async (url, init) => {
          calls.push([url, init.method]);
          const override = change(url, init);
          if (override instanceof Error) {
            throw override;
          }
          const status = override?.status ?? 200;
          const body =
            override?.body ??
            (url.endsWith(`${artifact}-${version}.module`) ? doc : jvm);
          return { status, ok: status === 200, json: async () => body };
        },
      },
    },
    expected,
  );
  return { result, calls };
}

test("real Gradle 1.1 pointer objects, repeated variants, inline files and root component redirects", async () => {
  const { result, calls } = await verify();
  assert.equal(result.status, "verified", JSON.stringify(result));
  assert.equal(result.files, 4);
  assert.equal(
    calls.filter(([url]) => url.endsWith("-jvm-0.31.0.module")).length,
    1,
  );
  assert.ok(
    calls.some(([url, method]) => url.endsWith(".jar") && method === "HEAD"),
  );
  for (const gate of ["signatures", "consumers", "release"]) {
    assert.equal(result.evidence[gate], "unverified");
  }
  assert.ok(calls.every(([url]) => !url.includes("undefined")));
});
for (const [name, change, status] of [
  [
    "absent root",
    (url) =>
      url.endsWith(`${artifact}-${version}.pom`) ? { status: 404 } : null,
    "unpublished",
  ],
  [
    "absent root module",
    (url) => (url.endsWith(".module") ? { status: 404 } : null),
    "root-metadata-missing",
  ],
  [
    "missing platform binary",
    (url) => (url.endsWith("-jvm-0.31.0.jar") ? { status: 404 } : null),
    "partial",
  ],
  ["transport failure", () => new Error("offline"), "transport-unknown"],
  [
    "server error",
    (url) => (url.endsWith(".module") ? { status: 503 } : null),
    "transport-unknown",
  ],
  [
    "invalid JSON",
    (url) => (url.endsWith(".module") ? { body: "not json" } : null),
    "malformed-metadata",
  ],
]) {
  test(`Gradle publication classifies ${name}`, async () =>
    assert.equal((await verify(jvmRoot, change)).result.status, status));
}
test("wrong pointer version, object module and hostile URL are malformed; missing configured platform is partial", async () => {
  for (const update of [
    { version: "0.30.0" },
    { module: { name: "bad" } },
    { url: "https://evil.invalid/file.module" },
    { url: "http://[" },
  ]) {
    const doc = structuredClone(jvmRoot);
    Object.assign(
      doc.variants.find((v) => v["available-at"])["available-at"],
      update,
    );
    assert.equal((await verify(doc)).result.status, "malformed-metadata");
  }
  const expected = {
    ...step,
    variants: [...step.variants, `${group}:${artifact}-js`],
  };
  assert.equal(
    (await verify(jvmRoot, () => null, expected)).result.status,
    "partial",
  );
});

test("absent-platform fixture retains the required JVM coordinate and reports partial", async () => {
  const missing = JSON.parse(
    readFileSync(
      new URL(
        "../../tests/fixtures/gradle-publication/missing-platform.json",
        import.meta.url,
      ),
    ),
  );
  const { result, calls } = await verify(jvmRoot, (url) =>
    url.endsWith(missing.missing) ? { status: 404 } : null,
  );
  assert.equal(result.status, missing.expected_status);
  assert.ok(result.missing.some((value) => value.includes(missing.platform)));
  assert.ok(calls.some(([url]) => url.endsWith(missing.missing)));
});

test("malformed inline file records are diagnosed without throwing", async () => {
  const doc = structuredClone(jvmRoot);
  doc.variants.find((variant) => variant.files).files.push(null);
  assert.equal((await verify(doc)).result.status, "malformed-metadata");
});
