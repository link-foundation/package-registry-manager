import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import {
  readFile,
  readlink,
  realpath,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

const ENDPOINT_FILE = ".prm-cdp-endpoint";
const BROWSER_SOCKET = /^ws:\/\/127\.0\.0\.1:(\d+)\/devtools\/browser\/[\w-]+$/;
const run = promisify(execFile);

// Ask for only the processes that have both Chromium switches. No profile
// path is interpolated into the command.
const WINDOWS_COMMAND_LINES =
  "[Console]::OutputEncoding = [Text.Encoding]::UTF8; " +
  "ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process " +
  "-Filter \"CommandLine LIKE '%--user-data-dir=%' AND CommandLine LIKE '%--remote-debugging-port=%'\" " +
  "| ForEach-Object { $_.CommandLine })";

function listedFlags(commandLine) {
  // macOS ps flattens argv. Windows may quote a whole switch or its value.
  return commandLine
    .trim()
    .split(/\s+"?--/)
    .slice(1);
}

async function profileCommands(profile) {
  if (process.platform === "win32") {
    if (!(await stat(path.join(profile, "Local State")).catch(() => null))) {
      return [];
    }
    const { stdout } = await run(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-Command", WINDOWS_COMMAND_LINES],
      { windowsHide: true, timeout: 5_000 },
    );
    return JSON.parse(stdout).map(listedFlags);
  }
  const lock = await readlink(path.join(profile, "SingletonLock"));
  const pid = /-(\d+)$/.exec(lock)?.[1];
  if (!pid || Number(pid) === 0) {
    return [];
  }
  if (process.platform === "linux") {
    const commandLine = await readFile(`/proc/${pid}/cmdline`, "utf8");
    return [
      commandLine
        .split("\0")
        .filter((arg) => arg.startsWith("--"))
        .map((arg) => arg.slice(2)),
    ];
  }
  if (process.platform === "darwin") {
    const { stdout } = await run("ps", ["-ww", "-p", pid, "-o", "command="], {
      timeout: 1_000,
    });
    return [listedFlags(stdout)];
  }
  return [];
}

async function processPort(profile) {
  const ports = new Set();
  const physical = await realpath(profile);
  for (const flags of await profileCommands(profile)) {
    if (flags.some((flag) => /^type=/.test(flag))) {
      continue;
    }
    const directories = flags.filter((flag) =>
      flag.startsWith("user-data-dir="),
    );
    const debugging = flags.filter((flag) =>
      flag.startsWith("remote-debugging-port="),
    );
    if (directories.length !== 1 || debugging.length !== 1) {
      continue;
    }
    const directory = directories[0]
      .slice("user-data-dir=".length)
      .trim()
      .replace(/^"|"$/g, "");
    const port = Number(
      /^remote-debugging-port=(\d+)"?(?:\s|$)/.exec(debugging[0])?.[1],
    );
    if (path.isAbsolute(directory) && port > 0 && port <= 65_535) {
      const active = await realpath(directory).catch(() => null);
      if (active && path.relative(physical, active) === "") {
        ports.add(port);
      }
    }
  }
  return ports.size === 1 ? ports.values().next().value : undefined;
}

export function configuredCdpPort(value = process.env.PRM_BROWSER_CDP_PORT) {
  if (value === undefined) {
    return undefined;
  }
  const port = Number(value);
  if (!/^\d+$/.test(value) || port < 1 || port > 65_535) {
    throw new Error("PRM_BROWSER_CDP_PORT must be an integer from 1 to 65535");
  }
  return port;
}

async function browserSocket(port) {
  try {
    const response = await fetch(`http://127.0.0.1:${port}/json/version`, {
      redirect: "error",
      signal: AbortSignal.timeout(750),
    });
    if (!response.ok) {
      return undefined;
    }
    const { webSocketDebuggerUrl } = await response.json();
    if (Number(BROWSER_SOCKET.exec(webSocketDebuggerUrl)?.[1]) === port) {
      return webSocketDebuggerUrl;
    }
  } catch {
    // An absent or stale endpoint is normal between browser runs.
  }
}

/** A recorded WebSocket ID prevents attaching to a different browser on a reused port. */
export async function runningEndpoint(profile, configuredPort) {
  if (configuredPort) {
    return browserSocket(configuredPort);
  }
  const recorded = await readFile(path.join(profile, ENDPOINT_FILE), "utf8")
    .then((value) => value.trim())
    .catch(() => "");
  const [port, target] = await readFile(
    path.join(profile, "DevToolsActivePort"),
    "utf8",
  ).then(
    (value) => value.trim().split(/\r?\n/),
    () => [],
  );
  for (const endpoint of [recorded, `ws://127.0.0.1:${port}${target}`]) {
    const match = BROWSER_SOCKET.exec(endpoint);
    if (match && (await browserSocket(Number(match[1]))) === endpoint) {
      return endpoint;
    }
  }
  // Chromium only writes DevToolsActivePort when it chooses the port itself.
  // Older CLI versions and external fixed-port launches leave neither file.
  const active = await processPort(profile).catch(() => undefined);
  return active ? browserSocket(active) : undefined;
}

export async function rememberEndpoint(profile, port) {
  if (!port) {
    return;
  }
  const endpoint = await browserSocket(port);
  if (!endpoint) {
    return;
  }
  const target = path.join(profile, ENDPOINT_FILE);
  const temporary = `${target}.${randomUUID()}`;
  try {
    await writeFile(temporary, `${endpoint}\n`, { mode: 0o600 });
    await rename(temporary, target);
  } finally {
    await rm(temporary, { force: true });
  }
}

export function isNpmPage(url) {
  try {
    const { hostname, protocol } = new URL(url);
    return (
      protocol === "https:" &&
      (hostname === "npmjs.com" || hostname.endsWith(".npmjs.com"))
    );
  } catch {
    return false;
  }
}
