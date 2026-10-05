import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

/**
 * Whether this module is the program node started. npm, npx, and global
 * installs start bins through a symlink in node_modules/.bin, while
 * import.meta.url names the real file, so both sides are resolved first.
 */
export function isDirectExecution(moduleUrl, entryPath) {
  if (!entryPath) {
    return false;
  }
  try {
    return realpathSync(fileURLToPath(moduleUrl)) === realpathSync(entryPath);
  } catch {
    return false;
  }
}
