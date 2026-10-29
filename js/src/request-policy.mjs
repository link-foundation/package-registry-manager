/** A queue shared by every client in this process. Request starts are >=1s apart. */
export function createRequestLimiter({ now = Date.now, sleep = delay } = {}) {
  let queue = Promise.resolve();
  let next = 0;
  return () => {
    const permit = queue.then(async () => {
      const wait = next - now();
      if (wait > 0) {
        await sleep(wait);
      }
      next = now() + 1000;
    });
    queue = permit.catch(() => {});
    return permit;
  };
}
export const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export const cratesPermit = createRequestLimiter();

/** Retry-After supports delay seconds and HTTP dates. Never retry early. */
export function retryDelay(value, attempt, now = Date.now()) {
  const seconds = value !== null && value !== undefined ? Number(value) : NaN;
  const until = Number.isFinite(seconds)
    ? seconds * 1000
    : Date.parse(value) - now;
  return Math.max(1000 * 2 ** attempt, Number.isFinite(until) ? until : 0);
}
