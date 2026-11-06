// Read-only public verification. No credential access, builds or uploads.
import { verifyCentralPublication } from "../../js/src/central-publication.mjs";
const group = "io.github.link-foundation";
const artifact = "links-notation-kotlin";
const result = await verifyCentralPublication({
  plan: { package: { gradle: { multiplatform: true } } },
  options: { fetch: (url, init) => fetch(url.replace("https://repo1.maven.org/maven2/", "https://repo.maven.apache.org/maven2/"), init) },
}, {
  coordinates: { group, artifact, version: "0.31.0" },
  variants: ["android", "iosarm64", "iossimulatorarm64", "iosx64", "js", "jvm", "wasm-js"].map((platform) => `${group}:${artifact}-${platform}`),
});
console.log(JSON.stringify(result, null, 2));
if (result.status !== "verified") process.exitCode = 1;
