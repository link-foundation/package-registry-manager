import { randomUUID } from "node:crypto";
import { readFile, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";

const ENDPOINT_FILE = ".prm-cdp-endpoint";
const BROWSER_SOCKET = /^ws:\/\/127\.0\.0\.1:(\d+)\/devtools\/browser\/[\w-]+$/;

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
