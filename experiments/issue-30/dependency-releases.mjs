// Record the latest stable releases of every direct dependency for issue #30.
import { readFileSync, writeFileSync } from "node:fs";

const npm = JSON.parse(readFileSync("js/package.json", "utf8"));
const cargo = readFileSync("rust/Cargo.toml", "utf8");
const dependencySections = cargo
  .split(/\[(?:dev-)?dependencies\]/)
  .slice(1)
  .map((section) => section.split(/^\[/m)[0])
  .join("\n");
const crates = [
  ...dependencySections.matchAll(
    /^([\w-]+) = (?:"([\d.]+)"|\{ version = "([\d.]+)")/gm,
  ),
];
const requests = [
  ...["dependencies", "devDependencies"].flatMap((section) =>
    Object.keys(npm[section]).map((name) => ({
      registry: "npm",
      section,
      name,
      declared: npm[section][name],
    })),
  ),
  ...[...new Set(crates.map((match) => match[1]))].map((name) => ({
    registry: "crates",
    name,
    declared: crates
      .find((match) => match[1] === name)
      .slice(2)
      .find(Boolean),
  })),
];
const releases = await Promise.all(
  requests.map(async (entry) => {
    const url =
      entry.registry === "npm"
        ? `https://registry.npmjs.org/${entry.name}/latest`
        : `https://crates.io/api/v1/crates/${entry.name}`;
    const response = await fetch(url, {
      headers: {
        "User-Agent": "package-registry-manager issue 30 dependency audit",
      },
    });
    if (!response.ok) throw new Error(`${url}: ${response.status}`);
    const data = await response.json();
    return {
      ...entry,
      version:
        entry.registry === "npm" ? data.version : data.crate.max_stable_version,
    };
  }),
);
writeFileSync(
  "ci-logs/dependency-releases.json",
  `${JSON.stringify(releases, null, 2)}\n`,
);
for (const release of releases) {
  const current = release.declared.replace(/^[~^]/, "");
  const latest = release.version.split("+")[0];
  console.log(
    `${release.registry} ${release.name}: ${current} / latest ${latest}`,
  );
  if (current !== latest) process.exitCode = 1;
}
