/* global document, location */
/** Official profile fields, never rendered, copied to clipboard or logged. */
export default function packagistPage(main = false) {
  if (location.origin !== "https://packagist.org") {
    return { status: "authentication-required" };
  }
  const text = document.body?.textContent ?? "";
  if (/Invalid state parameter passed in callback URL/i.test(text)) {
    return { status: "stale-oauth" };
  }
  const groups = [...document.querySelectorAll(".api-token-group")];
  const token = (label) => {
    const group = groups.find((node) =>
      node.querySelector("button")?.textContent.includes(label),
    );
    const input = group?.querySelector("input.api-token");
    return input?.getAttribute("data-api-token") || input?.value || null;
  };
  const username = document.querySelector("h2.title")?.textContent.trim();
  const safe = token("Safe API Token");
  if (username && safe && /^[A-Za-z0-9_.-]+$/.test(username)) {
    return {
      status: "authenticated",
      username,
      token: main ? token("Main API Token") : safe,
    };
  }
  if (document.querySelector('a[href*="logout"]')) {
    return { status: "profile-needed" };
  }
  const oauth = [...document.querySelectorAll("a[href]")].find((node) =>
    /Log in with GitHub/i.test(node.textContent),
  );
  return {
    status: "authentication-required",
    oauth: oauth ? new URL(oauth.href, location.href).href : null,
  };
}
