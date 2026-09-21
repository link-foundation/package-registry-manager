export const REGISTRIES = Object.freeze([
  "npm",
  "crates-io",
  "pypi",
  "go-modules",
  "nuget",
  "maven-central",
  "packagist",
]);

const REGISTRY_ALIASES = new Map([
  ["npm", "npm"],
  ["cargo", "crates-io"],
  ["crate", "crates-io"],
  ["crates", "crates-io"],
  ["crates-io", "crates-io"],
  ["python", "pypi"],
  ["pypi", "pypi"],
  ["go", "go-modules"],
  ["go-module", "go-modules"],
  ["go-modules", "go-modules"],
  ["dotnet", "nuget"],
  ["nuget", "nuget"],
  ["java", "maven-central"],
  ["maven", "maven-central"],
  ["maven-central", "maven-central"],
  ["composer", "packagist"],
  ["php", "packagist"],
  ["packagist", "packagist"],
]);

export function parseRegistry(value) {
  const normalized = value.toLowerCase().replaceAll(/[._]/g, "-");
  const registry = REGISTRY_ALIASES.get(normalized);
  if (!registry) {
    throw new Error(
      `unsupported registry '${value}'; expected ${REGISTRIES.join(", ")}`,
    );
  }
  return registry;
}
