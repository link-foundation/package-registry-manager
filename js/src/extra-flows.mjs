import { credentialRequirements } from "./credential-requirements.mjs";
import { credentialSteps } from "./credential-cycle.mjs";
import { tokenSecretSteps } from "./tokens.mjs";
import {
  centralPublicationStep,
  gradleValidationSteps,
} from "./gradle-distribution.mjs";

/** Additional OIDC registries never fall back to a stored publishing token. */
export function trustedFlow(item, context) {
  if (item.registry === "nuget") {
    return nugetFlow(item, context);
  }
  const specs = {
    rubygems: [
      "gem",
      ["build", item.manifest.split("/").at(-1)],
      "https://rubygems.org/profile/oidc/pending_trusted_publishers",
      "Register the GitHub Actions identity as a pending publisher, or add it in the existing gem settings.",
    ],
    nuget: [
      "dotnet",
      ["pack", "--configuration", "Release"],
      "https://www.nuget.org/account/TrustedPublishing",
      "Register a trusted publishing policy for this GitHub Actions identity. Use NuGet/login to exchange OIDC for a short-lived API key; never store that key.",
    ],
    jsr: [
      "deno",
      ["publish", "--dry-run"],
      `https://jsr.io/${item.name}/settings`,
      "Link this package to the GitHub repository; publish from the configured workflow through OIDC.",
    ],
  };
  const [program, args, url, description] = specs[item.registry];
  const steps = [
    {
      id: "validate-package",
      title: `Validate ${item.registry} package`,
      kind: "check",
      description: "Build or validate locally without uploading.",
      command: { program, args },
    },
    {
      id: "configure-trusted-publisher",
      title: `Configure ${item.registry} trusted publishing`,
      kind: "browser",
      description,
      url,
    },
    {
      id: "verify-oidc-release",
      title: "Verify an OIDC release before removing tokens",
      kind: "manual",
      description: `Run ${context.workflow} and verify registry acceptance through OIDC; remove secret references from every workflow before deleting unused credentials.`,
    },
  ];
  if (context.slug) {
    steps.push(...tokenSecretSteps(item, context.slug));
  }
  return steps;
}

export function tokenFlow(item) {
  const checks = {
    "maven-central": { program: "mvn", args: ["--batch-mode", "verify"] },
    "vscode-marketplace": {
      program: "npx",
      args: ["--no-install", "vsce", "package"],
    },
    "open-vsx": { program: "npx", args: ["--no-install", "vsce", "package"] },
  };
  const gradle = item.registry === "maven-central" && item.gradle;
  const steps = gradle
    ? gradleValidationSteps(item)
    : checks[item.registry]
      ? [
          {
            id: "validate-package",
            title: "Validate package before credential changes",
            kind: "check",
            description: "Build without publishing.",
            command: checks[item.registry],
          },
        ]
      : [];
  if (item.registry === "maven-central") {
    const group = /^([\w.-]+):/.exec(item.name ?? "")?.[1];
    steps.push({
      id: "verify-namespace",
      title: `Verify the Central namespace${group ? ` ${group}` : ""}`,
      kind: "browser",
      description:
        "Sign in to the Central Portal and verify the namespace used by the package coordinates. Publishing stays blocked until the namespace reads back as verified.",
      url: "https://central.sonatype.com/publishing/namespaces",
      ...(group ? { namespace: group } : {}),
      state: "manual-prerequisite",
    });
    return steps.concat(mavenCredentialSteps(item), [
      centralPublicationStep(item),
    ]);
  }
  return steps.concat(
    credentialSteps(item.registry, item.token_secrets?.[0] ?? item.registry),
  );
}

/**
 * Central publishing needs a provider-specific pair plus signing material; no
 * OIDC is claimed. Missing secrets are requested, present ones are kept, and
 * nothing is rotated or revoked.
 */
export function mavenCredentialSteps(item) {
  const requirements =
    item.credential_requirements ?? credentialRequirements("maven-central");
  const pick = (...roles) =>
    requirements.secrets.filter((entry) => roles.includes(entry.role));
  const describe = (entries) =>
    entries.map((entry) => entry.secret).join(" + ");
  const pair = pick("central-username", "central-token");
  const signing = pick("gpg-private-key", "gpg-passphrase");
  return [
    {
      id: "ensure-central-credential-pair",
      title: `Ensure the Central user-token pair ${describe(pair)}`,
      kind: "api",
      confirm: true,
      secrets: pair,
      oidc: "unsupported",
      description: `${requirements.oidc_note} Generate a Central Portal user token (username + password pair) only when ${describe(pair)} is missing, hold the values in memory, and store them as encrypted GitHub secrets through gh-manager. Existing secrets are kept; nothing is rotated or revoked.`,
      url: "https://central.sonatype.com/usertoken",
    },
    {
      id: "ensure-signing-key",
      title: `Ensure the signing prerequisites ${describe(signing)}`,
      kind: "manual",
      confirm: true,
      secrets: signing,
      state: "manual-prerequisite",
      description:
        "Maven Central rejects unsigned artifacts. Configure central.signing to reuse local private material or explicitly authorize repository-specific key generation. Verify detached signing and independent public-key readback, store the signing pair encrypted with its actual expiry, and remove temporary private material.",
    },
  ];
}

/** Public Packagist: tags plus hook or update API; no OIDC, explicit states otherwise. */
export function packagistSteps(item) {
  const requirements =
    item.credential_requirements ?? credentialRequirements("packagist");
  const api = requirements.secrets;
  return [
    {
      id: "ensure-packagist-package",
      title: "Validate and register the root Composer package",
      kind: "api",
      secrets: api,
      description:
        "Validate composer.json, discover the exact registered repository, recover sign-in in the same browser profile and submit once only if absent. Never rename a published package.",
    },
    {
      id: "packagist-update-api",
      title: "Encrypt Safe update credentials and verify acceptance",
      kind: "api",
      secrets: api,
      description:
        "Select the Safe API token in memory, encrypt the selected secrets through gh-manager, read presence back and independently require HTTP 202 update acceptance.",
    },
    {
      id: "packagist-github-hook",
      title: "Verify the Packagist push hook and test delivery",
      kind: "api",
      description:
        "Inspect/create/repair the Packagist hook through gh-manager; preserve unrelated hooks, read back active JSON push settings and inspect a fresh test delivery.",
    },
    {
      id: "verify-packagist-package",
      title: "Verify exact Packagist publication and consumer",
      kind: "api",
      description:
        "Resume only affected failed jobs after configuration verification. Separately verify registration, queued update, exact indexed version/source SHA, Composer consumer and GitHub release.",
    },
  ];
}

/** NuGet uses explicit account policy and preserves provided fallback credentials. */
export function nugetFlow(item, context) {
  return [
    {
      id: "validate-package",
      title: "Validate NuGet package",
      kind: "check",
      description:
        "Build and test the selected project before configuration; do not upload.",
      command: {
        program: "dotnet",
        args: [
          "test",
          item.manifest.split("/").at(-1),
          "--configuration",
          "Release",
        ],
      },
    },
    {
      id: "configure-nuget-policy",
      title: "Verify exact NuGet trusted publishing policy",
      kind: "api",
      description:
        "Read explicit nuget.username, package_owner, package_ids and operations from .package-registry-manager.json; resolve immutable GitHub IDs, verify the authenticated profile, discover before creating a narrow policy, read activation back and ensure effective NUGET_USER through gh-manager. Retain existing fallback secrets.",
    },
    {
      id: "verify-nuget-release",
      title: "Verify NuGet CI and exact public consumer",
      kind: "api",
      description: `Verify ${context.workflow}: successful OIDC exchange and actual upload are separate gates. Retain existing token fallbacks. Require the repaired current-main revision, every required build/test/documentation/mirror/release stage, exact public package download and consumer restore/build. A skipped stage or placeholder remains incomplete.`,
    },
    ...(context.slug ? tokenSecretSteps(item, context.slug) : []),
  ];
}
