import { setTimeout as delay } from "node:timers/promises";

import { APPROVAL_WINDOW_MS } from "./approvals.mjs";

const COOLDOWN = "#cooldownOptin_didOptForCooldown";
const KEY = "[data-prm-npm-security-key]";

export const npmApprovalState = `(() => {
  if (location.protocol === 'chrome-error:') return { failure: 'the approval page could not load' };
  if (location.protocol !== 'https:' || !/(^|\\.)npmjs\\.com$/i.test(location.hostname)) return {};
  const text = document.body?.innerText ?? '';
  if (/something went wrong/i.test(text)) return { failure: 'the approval page reported Something went wrong' };
  if (/\\bexpired\\b|\\binvalid (?:approval|link|session)\\b/i.test(text)) {
    return { failure: 'the approval page reported an expired or invalid link' };
  }
  const box = document.querySelector('${COOLDOWN}');
  const button = Array.from(document.querySelectorAll('button')).find((candidate) =>
    /^(?:use|authenticate with|continue with)\\s+security key$/i.test(candidate.innerText.trim()) &&
    !candidate.matches(':disabled') && candidate.getClientRects().length
  );
  for (const marked of document.querySelectorAll('${KEY}')) {
    if (marked !== button) marked.removeAttribute('data-prm-npm-security-key');
  }
  button?.setAttribute('data-prm-npm-security-key', '');
  return { cooldown: box ? box.checked : null, key: Boolean(button) };
})()`;

/** Watch one npm link until its command ends or the page needs a fresh link. */
export async function watchNpmApproval(
  page,
  {
    signal,
    timeoutMs = APPROVAL_WINDOW_MS,
    pollIntervalMs = 500,
    log = console.log,
  } = {},
) {
  if (
    typeof page?.evaluate !== "function" ||
    typeof page?.click !== "function"
  ) {
    return undefined;
  }
  const deadline = Date.now() + timeoutMs;
  let requestedKey = false;
  while (!signal?.aborted) {
    const state = await page.evaluate(npmApprovalState).catch(() => null);
    if (signal?.aborted) {
      break;
    }
    if (state?.failure) {
      return state.failure;
    }
    if (!requestedKey && state?.cooldown === false) {
      await page.click(COOLDOWN, { timeout: 1_000 }).catch(() => {});
    } else if (!requestedKey && state?.key) {
      // A click can start navigation before its driver acknowledgement arrives.
      // Only request the native prompt once for this link.
      requestedKey = true;
      const clicked = await page.click(KEY, { timeout: 1_000 }).then(
        () => true,
        () => false,
      );
      if (clicked && !signal?.aborted) {
        log(
          '  Clicked "Use security key"; approve with Touch ID or your security key.',
        );
      }
    }
    if (Date.now() >= deadline) {
      return "the browser approval deadline elapsed";
    }
    await delay(pollIntervalMs, undefined, { signal }).catch(() => {});
  }
  return undefined;
}
