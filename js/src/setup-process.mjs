import {
  approvalDeadline,
  TWO_FACTOR_HINT,
  withFreshLinks,
} from "./approvals.mjs";
import {
  ANSI,
  nodeOptionsWithShim,
  runInteractive,
  writeTtyShim,
} from "./auth-urls.mjs";
import { watchNpmApproval } from "./npm-approval.mjs";

/** A trusted publisher in `npm trust list` output, human-readable or JSON. */
const LISTED_PUBLISHER = /^\s*(?:id|type|file):\s*\S|"(?:id|type|file)"\s*:/m;

/** Whether `npm trust list` output names a trusted publisher. */
export function listsTrustedPublisher(output) {
  return LISTED_PUBLISHER.test(stripAnsi(output));
}

export function stripAnsi(text) {
  return String(text ?? "").replaceAll(ANSI, "");
}

/** Runs a step's command; `extra` is added to that child's environment only. */
export async function runProcess(session, step, mirror, extra) {
  const { command, cwd } = session.prepare(step);
  let env = extra ? { ...process.env, ...extra } : process.env;
  const npm = ["npm", "npx"].includes(command.program);
  if (npm) {
    await session.detectDefaultBrowser();
    session.shim ??= await writeTtyShim();
    env = {
      ...env,
      NODE_OPTIONS: nodeOptionsWithShim(env.NODE_OPTIONS, session.shim.shim),
    };
  }
  const trustCreate = npm && step.id === "attach-trusted-publisher";
  const run = () =>
    runInteractive(command, {
      cwd,
      env,
      mirror,
      stopOnLegacyLogin: npm,
      watchStderr: npm || step.id === "trigger-release",
      watchApproval: npm,
      trustCreate,
      onUrl: (url, kind, signal) => session.openLink(url, kind, signal),
    });
  // npm's links expire after about 5 minutes; ask npm for a fresh one.
  const result = await (npm ? withFreshLinks(run) : run());
  return result.trustStored ? { ...result, code: 0 } : result;
}

export async function openLink(session, url, kind, signal) {
  console.log(approvalDeadline(kind));
  if (kind === "approve") {
    console.log(TWO_FACTOR_HINT);
  }
  try {
    await session.open(url, !session.options.openWith);
  } catch (error) {
    console.error(`warning: could not open ${url}: ${error.message}`);
    return undefined;
  }
  if (
    !session.automation ||
    session.options.noBrowser ||
    session.options.openWith ||
    signal?.aborted
  ) {
    return undefined;
  }
  if (!signal) {
    await session.submitPrefilledSignIn({ formGraceMs: 5_000 });
    return undefined;
  }
  const link = new globalThis.AbortController();
  const active = AbortSignal.any([signal, link.signal]);
  const signIn = session.submitPrefilledSignIn({
    formGraceMs: kind === "login" ? 15_000 : 5_000,
    signal: active,
  });
  try {
    return await watchNpmApproval(session.automation, { signal: active });
  } finally {
    link.abort();
    await signIn;
  }
}
