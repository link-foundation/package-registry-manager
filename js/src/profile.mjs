import { mkdir, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { exec } from "command-stream";
import { findBrowserSource } from "browser-commander";

const APPLICATION = "package-registry-manager";
const PROFILE = "browser-profile";
const REPOSITORY_DIRECTORY = ".package-registry-manager";

/**
 * The dedicated automation profile, shared by every repository of the user.
 * It lives in the per-user state directory, never inside a repository, so its
 * registry session cookies cannot be committed.
 */
export function defaultBrowserProfile({
  platform = process.platform,
  env = process.env,
  home = os.homedir(),
  channel = "chrome",
} = {}) {
  const source = findBrowserSource(channel);
  const profile =
    source?.controlProtocol && source.controlProtocol !== "cdp"
      ? `${PROFILE}-${source.id}`
      : PROFILE;
  const join = platform === "win32" ? path.win32.join : path.posix.join;
  if (platform === "win32") {
    const base = env.LOCALAPPDATA || join(home, "AppData", "Local");
    return join(base, APPLICATION, profile);
  }
  if (platform === "darwin") {
    return join(home, "Library", "Application Support", APPLICATION, profile);
  }
  // The XDG specification ignores relative values.
  const state = path.posix.isAbsolute(env.XDG_STATE_HOME ?? "")
    ? env.XDG_STATE_HOME
    : join(home, ".local", "state");
  return join(state, APPLICATION, profile);
}

/** The profile location used before it moved out of the repository. */
export function legacyBrowserProfile(repository) {
  return path.join(repository, REPOSITORY_DIRECTORY, PROFILE);
}

/**
 * Makes sure Git ignores a browser profile before a browser writes cookies to
 * it. Outside a Git work tree this only creates the directory. Inside one it
 * writes a `.gitignore` containing `*` (into `.package-registry-manager/` for
 * the legacy location, otherwise into the profile itself) and throws when Git
 * still does not ignore the profile or already tracks files in it.
 */
export async function ensureProfileIgnored(profile, options = {}) {
  await mkdir(profile, { recursive: true, mode: 0o700 });
  const git = (...args) =>
    exec("git", args, {
      cwd: profile,
      capture: true,
      mirror: false,
      stdin: "ignore",
    });
  const worktree = await git("rev-parse", "--show-toplevel");
  if (worktree.code !== 0) {
    if (options.verbose) {
      console.error(`${profile} is not inside a Git work tree`);
    }
    return;
  }
  const top = String(worktree.stdout).trim();
  const parent = path.dirname(profile);
  const guarded =
    path.basename(parent) === REPOSITORY_DIRECTORY ? parent : profile;
  await writeFile(path.join(guarded, ".gitignore"), "*\n");
  // Relative to the profile, so symbolic links in its path do not matter.
  const probe = path.join("Default", "Cookies");
  const ignored = await git("check-ignore", "--quiet", "--", probe);
  const tracked = await git("ls-files", "--", ".");
  if (ignored.code !== 0 || String(tracked.stdout).trim() !== "") {
    throw new Error(
      `the browser profile ${profile} is inside the Git work tree ${top} and Git does not ignore it; ` +
        "move it with --browser-profile or ignore it (and untrack its files) before signing in",
    );
  }
  if (options.verbose) {
    console.error(`${profile} is ignored by Git (${guarded}/.gitignore)`);
  }
}

/**
 * Protects a browser profile left inside the repository by an older release
 * and suggests deleting it, because it can hold registry session cookies.
 */
export async function protectLegacyProfile(repository, profile, options = {}) {
  const legacy = legacyBrowserProfile(repository);
  if (path.resolve(profile) === path.resolve(legacy)) {
    return;
  }
  const found = await stat(legacy).then(
    (info) => info.isDirectory(),
    () => false,
  );
  if (found) {
    await ensureProfileIgnored(legacy, options);
    console.error(
      `warning: ${legacy} is no longer used and may hold registry session cookies; ` +
        `delete it (the browser profile is now ${profile})`,
    );
  }
}
