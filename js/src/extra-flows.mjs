import { credentialRequirements } from "./credential-requirements.mjs";
import { credentialSteps } from "./credential-cycle.mjs";
import { tokenSecretSteps } from "./tokens.mjs";
import {
  gradlePublicationStep,
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
    return steps.concat(
      mavenCredentialSteps(item),
      gradle ? [gradlePublicationStep(item)] : [],
    );
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
        "Maven Central rejects unsigned artifacts. Provide an armored GPG private key whose public key is on a public keyserver, and its passphrase, as encrypted GitHub secrets; keys are never written to disk or printed.",
    },
  ];
}

/** Public Packagist: tags plus hook or update API; no OIDC, explicit states otherwise. */
export function packagistSteps(item) {
  const requirements =
    item.credential_requirements ?? credentialRequirements("packagist");
  const api = requirements.secrets;
  const referenced = api.every((entry) => entry.referenced);
  return [
    {
      id: "verify-packagist-package",
      title: "Read back the Packagist package and exact stable tag",
      kind: "check",
      description: `Read https://repo.packagist.org/p2/${item.name}.json and require the exact stable version ${item.version ?? "(unresolved)"} from a tag; dev branches do not count.`,
      url: `https://packagist.org/packages/${item.name}`,
    },
    {
      id: "packagist-github-hook",
      title: "Confirm the Packagist GitHub integration",
      kind: "manual",
      state: "manual-prerequisite",
      description:
        'Packagist updates on tag pushes through its GitHub hook/app integration. GitHub does not expose that installation to this tool, so confirm it on the package page (no "not auto-updated" warning).',
      url: `https://packagist.org/packages/${item.name}`,
    },
    {
      id: "packagist-update-api",
      title: referenced
        ? `Keep the update-package API path ${api.map((entry) => entry.secret).join(" + ")}`
        : "Packagist update-package API is not configured",
      kind: "manual",
      state: referenced ? "configured" : "unsupported-without-credentials",
      secrets: api,
      oidc: "unsupported",
      description: `${requirements.oidc_note} ${referenced ? "The workflow calls the update-package API; keep both secrets as encrypted GitHub secrets. Nothing is rotated or revoked." : "Rely on the GitHub hook, or add PACKAGIST_USERNAME and PACKAGIST_TOKEN (Packagist profile API token) as encrypted GitHub secrets for the update-package API."}`,
      url: "https://packagist.org/profile/",
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
