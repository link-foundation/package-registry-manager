/** Registry knowledge belongs to this consumer, never to gh-manager. */
export const AUTH_FAILURE_PATTERNS = Object.freeze({
  npm: [
    "\\bENEEDAUTH\\b",
    "\\bE401\\b",
    "\\bE403\\b",
    "invalid-publisher",
    "E404.*PUT|PUT.*E404",
    "Access token expired or revoked",
  ],
  "crates-io": [
    "No Trusted Publishing config found",
    "invalid-publisher",
    "token.*(?:invalid|expired|revoked)",
    "authentication failed",
  ],
  pypi: [
    "invalid-publisher",
    "invalid.*(?:token|authentication)",
    "403.*Forbidden",
  ],
  rubygems: ["API key.*(?:invalid|expired)", "invalid-publisher"],
  nuget: ["API key.*(?:invalid|expired)", "invalid-publisher"],
  jsr: ["invalid-publisher", "publishing.*(?:unauthorized|permission)"],
  "docker-hub": [
    "unauthorized: authentication required",
    "denied: requested access",
    "incorrect username or password",
  ],
  "maven-central": [
    "401.*Unauthorized",
    "403.*Forbidden",
    "Invalid user token",
  ],
  "vscode-marketplace": ["Invalid.*(?:PAT|token)", "TF400813", "VSCE_PAT"],
  "open-vsx": ["Invalid access token", "token.*(?:expired|invalid)"],
  "chrome-web-store": [
    "invalid_grant",
    "invalid_client",
    "invalid.*refresh.token",
  ],
});

export const RELEASE_FAILURE_PATTERNS = Object.freeze([
  ...new Set(Object.values(AUTH_FAILURE_PATTERNS).flat()),
]);
