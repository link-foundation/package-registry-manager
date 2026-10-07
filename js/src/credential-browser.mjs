/** Account pages used only in the dedicated automated browser. */
export const TOKEN_PROVIDERS = Object.freeze({
  "docker-hub": {
    url: "https://app.docker.com/settings/personal-access-tokens/create",
    secret: "DOCKERHUB_TOKEN",
    scope: "Read & Write",
    domains: ["docker.com", "docker.io"],
  },
  "maven-central": {
    url: "https://central.sonatype.com/usertoken",
    secret: "MAVEN_CENTRAL_TOKEN",
    scope: "publish",
    domains: ["sonatype.com"],
  },
  "vscode-marketplace": {
    url: "https://dev.azure.com/_usersSettings/tokens",
    secret: "VSCE_PAT",
    scope: "Marketplace (Manage)",
    domains: ["dev.azure.com", "login.microsoftonline.com"],
  },
  "open-vsx": {
    url: "https://open-vsx.org/user-settings/tokens",
    secret: "OVSX_PAT",
    scope: "publish",
    domains: ["open-vsx.org", "github.com"],
  },
  "chrome-web-store": {
    url: "https://developers.google.com/oauthplayground/",
    secret: "CHROME_WEB_STORE_REFRESH_TOKEN",
    scope: "https://www.googleapis.com/auth/chromewebstore",
    domains: ["google.com"],
  },
});

/** Fill only credential name, publishing scope, repository and expiry. Never widen scope. */
export function tokenFormScript(provider, name, expiresAt) {
  return `(() => {
    const values = ${JSON.stringify({ name, scope: provider.scope, expires: expiresAt.slice(0, 10) })};
    const fields = { name: ['input[name="name"]', 'input[name="description"]', 'input[name="tokenName"]'], scope: ['select[name="scope"]', 'select[name="permissions"]'], expires: ['input[type="date"]', 'input[name="expires_at"]'] };
    const filled = [];
    for (const [key, selectors] of Object.entries(fields)) {
      const field = selectors.map(selector => document.querySelector(selector)).find(Boolean);
      if (!field) continue;
      if (field.tagName === 'SELECT') {
        const option = [...field.options].find(option => option.textContent.trim() === values[key] || option.value === values[key]);
        if (!option) continue;
        field.value = option.value;
      } else field.value = values[key];
      field.dispatchEvent(new Event('input', { bubbles: true }));
      field.dispatchEvent(new Event('change', { bubbles: true }));
      filled.push(key);
    }
    return { filled };
  })()`;
}

/** Read the one-time value without returning it in any displayed diagnostic. */
export const READ_TOKEN = `(() => {
  const read = selectors => selectors.map(selector => document.querySelector(selector)).find(Boolean);
  const value = read(['input[data-testid="token-value"]', 'input[name="token"]', 'textarea[name="token"]', '[data-testid="generated-token"]', 'input[name="refresh_token"]']);
  const id = read(['[data-token-id]', 'input[name="token_id"]']);
  const expires = read(['input[name="expires_at"]', '[data-expires-at]']);
  const rawExpiry = expires?.value || expires?.dataset?.expiresAt;
  const expiry = rawExpiry && /^\\d{4}-\\d{2}-\\d{2}$/.test(rawExpiry) ? rawExpiry + 'T00:00:00Z' : rawExpiry;
  return { value: value?.value || value?.textContent?.trim(), id: id?.dataset?.tokenId || id?.value, expires_at: expiry };
})()`;

/** Verify absence only when the token list itself is visible and identifies rows. */
export function revokedScript(id) {
  if (typeof id !== "string" || id.length > 512) {
    return "false";
  }
  const characters = Array.from(id);
  if (characters.length === 0 || characters.length > 256) {
    return "false";
  }
  // Only numeric literals enter generated code; registry text remains data.
  const points = characters
    .map((character) => character.codePointAt(0))
    .join(",");
  return `(() => { const id = String.fromCodePoint(${points}); const list = document.querySelector('[data-testid="tokens-list"], [data-token-list]'); if (!list) return false; return ![...list.querySelectorAll('[data-token-id]')].some(row => row.dataset.tokenId === id); })()`;
}
