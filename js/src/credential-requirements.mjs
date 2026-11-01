/**
 * Registries without public OIDC publishing: the exact credential set a
 * workflow needs, which secrets it already references, and where values may
 * live. Values are only ever held in memory or written to encrypted GitHub
 * secrets; nothing here rotates, revokes or deletes an existing credential.
 */
export const CREDENTIAL_SETS = Object.freeze({
  "maven-central": {
    oidc: "unsupported",
    oidc_note:
      "Sonatype Central Portal has no OIDC trusted publishing; publish with a Central user-token pair and a GPG signing key.",
    namespace: "verified Central Portal namespace matching the groupId",
    roles: [
      {
        role: "central-username",
        candidates: [
          "CENTRAL_USERNAME",
          "MAVEN_CENTRAL_USERNAME",
          "OSSRH_USERNAME",
          "SONATYPE_USERNAME",
          "MAVEN_USERNAME",
        ],
        default: "CENTRAL_USERNAME",
      },
      {
        role: "central-token",
        candidates: [
          "CENTRAL_TOKEN",
          "MAVEN_CENTRAL_TOKEN",
          "MAVEN_CENTRAL_PASSWORD",
          "OSSRH_TOKEN",
          "OSSRH_PASSWORD",
          "SONATYPE_TOKEN",
          "MAVEN_PASSWORD",
        ],
        default: "CENTRAL_TOKEN",
      },
      {
        role: "gpg-private-key",
        candidates: ["GPG_PRIVATE_KEY", "MAVEN_GPG_PRIVATE_KEY", "SIGNING_KEY"],
        default: "GPG_PRIVATE_KEY",
      },
      {
        role: "gpg-passphrase",
        candidates: [
          "GPG_PASSPHRASE",
          "MAVEN_GPG_PASSPHRASE",
          "SIGNING_PASSWORD",
        ],
        default: "GPG_PASSPHRASE",
      },
    ],
  },
  packagist: {
    oidc: "unsupported",
    oidc_note:
      "Public packagist.org has no OIDC publishing (Private Packagist OIDC is a separate product); releases come from tags via the GitHub hook or the update-package API.",
    roles: [
      {
        role: "packagist-username",
        candidates: ["PACKAGIST_USERNAME"],
        default: "PACKAGIST_USERNAME",
        optional: true,
      },
      {
        role: "packagist-api-token",
        candidates: ["PACKAGIST_TOKEN", "PACKAGIST_API_TOKEN"],
        default: "PACKAGIST_TOKEN",
        optional: true,
      },
    ],
  },
});

const SECRET = /\bsecrets\.([A-Za-z_][A-Za-z0-9_]*)\b/g;

/** Structured requirements resolved against the secrets the workflows read. */
export function credentialRequirements(registry, workflows = []) {
  const set = CREDENTIAL_SETS[registry];
  if (!set) {
    return null;
  }
  const referenced = new Set(
    workflows.flatMap((workflow) =>
      [...workflow.contents.replace(/^\s*#.*$/gm, "").matchAll(SECRET)].map(
        (match) => match[1],
      ),
    ),
  );
  return {
    oidc: set.oidc,
    oidc_note: set.oidc_note,
    ...(set.namespace ? { namespace: set.namespace } : {}),
    storage: "memory-or-encrypted-github-secret",
    secrets: set.roles.map((role) => {
      const found = role.candidates.find((name) => referenced.has(name));
      return {
        role: role.role,
        secret: found ?? role.default,
        referenced: Boolean(found),
        ...(role.optional ? { optional: true } : {}),
      };
    }),
  };
}
