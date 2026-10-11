/** Minimum runtime for the CLI and its GitHub dependencies. */
export const NODE_ENGINES = ">=22.12.0";

export function supportsNode(version) {
  const [major, minor] = String(version)
    .replace(/^v/, "")
    .split(".")
    .map(Number);
  return major > 22 || (major === 22 && minor >= 12);
}

export function nodeRequirement(version) {
  return `package-registry-manager needs Node.js 22.12 or newer (found ${version})`;
}
