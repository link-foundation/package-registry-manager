import { spawn } from "node:child_process";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

/**
 * Preloaded into npm so it treats piped stdout as a terminal. npm only offers
 * web authentication (`Authenticate your account at:`) on a TTY; the tool
 * pipes stdout to find those URLs and open them in a browser.
 */
export const TTY_SHIM = `for (const stream of [process.stdin, process.stdout]) {
  if (!stream.isTTY) {
    Object.defineProperty(stream, "isTTY", { value: true, configurable: true });
  }
}
`;

const PROMPTS = [/^Login at:?$/i, /^Authenticate your account at:?$/i];
const INLINE =
  /(?:Login at|Authenticate your account at):?\s+(https?:\/\/\S+)/i;
// npm falls back to this prompt when a web login is not completed in time.
const LEGACY_LOGIN = /^Username:/i;
/** Terminal color and cursor escape sequences. */
// eslint-disable-next-line no-control-regex
export const ANSI = /\u001b\[[0-9;?]*[ -/]*[@-~]/g;

const clean = (raw) => raw.replace(ANSI, "").trim();

/**
 * Returns a line scanner that reports web-authentication URLs printed by npm
 * and, once, npm's legacy `Username:` prompt, which is printed without a
 * trailing newline.
 */
export function authUrlScanner(onUrl, onLegacyLogin) {
  let pending = "";
  let expectUrl = false;
  let legacy = false;
  const seen = new Set();
  const detectLegacy = (line) => {
    if (onLegacyLogin && !legacy && LEGACY_LOGIN.test(line)) {
      legacy = true;
      onLegacyLogin();
    }
  };
  const report = (url) => {
    if (!seen.has(url)) {
      seen.add(url);
      onUrl(url);
    }
  };
  const scanLine = (raw) => {
    const line = clean(raw);
    detectLegacy(line);
    const inline = INLINE.exec(line);
    if (inline) {
      report(inline[1]);
      expectUrl = false;
    } else if (PROMPTS.some((prompt) => prompt.test(line))) {
      expectUrl = true;
    } else if (expectUrl && /^https?:\/\/\S+$/.test(line)) {
      report(line);
      expectUrl = false;
    } else if (line) {
      expectUrl = false;
    }
  };
  return (chunk) => {
    pending += chunk;
    const lines = pending.split(/\r?\n/);
    pending = lines.pop();
    lines.forEach(scanLine);
    detectLegacy(clean(pending));
  };
}

/** Appends a `--require` of the TTY shim to existing NODE_OPTIONS. */
export function nodeOptionsWithShim(existing, shimPath) {
  const quoted = shimPath.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
  return [existing, `--require "${quoted}"`].filter(Boolean).join(" ");
}

/** Writes the TTY shim to a private temporary directory. */
export async function writeTtyShim() {
  const directory = await mkdtemp(path.join(os.tmpdir(), "prm-shim-"));
  const shim = path.join(directory, "tty-shim.cjs");
  await writeFile(shim, TTY_SHIM, { mode: 0o600 });
  return { directory, shim };
}

/**
 * Runs an exact argument vector with the terminal's stdin and stderr, while
 * mirroring and capturing stdout. stdin stays a real terminal so masked
 * prompts (cargo login, gh secret set) keep working. With
 * `options.stopOnLegacyLogin` the command is stopped at npm's legacy
 * `Username:` prompt and the result has `legacyLogin: true`.
 */
export function runInteractive(command, options) {
  return new Promise((resolve, reject) => {
    const child = spawn(command.program, command.args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: ["inherit", "pipe", "inherit"],
      shell: false,
    });
    let legacyLogin = false;
    const stop = options.stopOnLegacyLogin
      ? () => {
          legacyLogin = true;
          child.kill();
        }
      : undefined;
    const scan =
      options.onUrl || stop
        ? authUrlScanner(options.onUrl ?? (() => {}), stop)
        : null;
    let stdout = "";
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
      if (options.mirror !== false) {
        process.stdout.write(chunk);
      }
      scan?.(chunk);
    });
    child.on("error", reject);
    child.on("close", (code) => {
      scan?.("\n");
      resolve({ code: code ?? 1, stdout, legacyLogin });
    });
  });
}
