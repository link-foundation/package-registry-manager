/**
 * npm's browser sign-in and approval links: how long they stay valid, how the
 * tool recognizes an expired one, and how often it asks npm for a fresh one.
 */

/** How long npm keeps a web login or approval session open. */
export const APPROVAL_WINDOW_MS = 5 * 60_000;
/** How many links the tool requests before it gives up on one step. */
export const APPROVAL_ATTEMPTS = 3;
/** npm's error once its approval session expired while it polled for the result. */
export const EXPIRED_APPROVAL = /Invalid response from web login endpoint/i;

/** Shown with every npm approval link. */
export const TWO_FACTOR_HINT =
  "If npm's approval page offers to skip two-factor checks for the next 5 minutes, choose it so the approvals that follow need no new confirmation.";

const pad = (number) => String(number).padStart(2, "0");

/**
 * The deadline printed with a sign-in or approval link, such as
 * "Sign in within about 5 minutes (until 19:05).".
 */
export function approvalDeadline(kind, now = new Date()) {
  const until = new Date(now.getTime() + APPROVAL_WINDOW_MS);
  const action = kind === "login" ? "Sign in" : "Approve";
  return `${action} within about 5 minutes (until ${pad(until.getHours())}:${pad(until.getMinutes())}).`;
}

/**
 * Why an npm run ended on an expired link, or undefined when it did not: a
 * web login falls back to npm's legacy `Username:` prompt, and a publish or
 * trust approval fails with npm's web-login error.
 */
export function expiredApproval(result) {
  if (result.legacyLogin) {
    return "npm fell back to its legacy username prompt";
  }
  if (result.code !== 0 && result.approvalExpired) {
    return "npm's approval session ended";
  }
  return undefined;
}

/**
 * Runs `attempt()` until it does not end on an expired link, up to
 * `attempts` times, printing why each fresh link is requested.
 */
export async function withFreshLinks(
  attempt,
  { attempts = APPROVAL_ATTEMPTS, log = console.log } = {},
) {
  for (let number = 1; ; number += 1) {
    const result = await attempt();
    const reason = expiredApproval(result);
    if (!reason) {
      return result;
    }
    if (number >= attempts) {
      throw new Error(
        `the browser link expired ${attempts} times (${reason}); re-run the command when you are ready to approve within 5 minutes`,
      );
    }
    log(
      `\nThe link expired before it was approved (${reason}); requesting a fresh one (attempt ${number + 1} of ${attempts}).`,
    );
  }
}

/** The closing note once a package publishes through trusted publishing. */
export function trustedReleaseNote(workflow) {
  return `Future releases publish from ${workflow} through trusted publishing; no login is needed.`;
}
