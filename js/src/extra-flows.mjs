import { credentialSteps } from "./credential-cycle.mjs";
import { tokenSecretSteps } from "./tokens.mjs";

/** Additional OIDC registries never fall back to a stored publishing token. */
export function trustedFlow(item, context) {
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
  const steps = checks[item.registry]
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
    steps.push({
      id: "verify-namespace",
      title: "Verify a Central namespace",
      kind: "browser",
      description:
        "Sign in to the Central Portal and verify the namespace used by the package coordinates.",
      url: "https://central.sonatype.com/publishing/namespaces",
    });
  }
  return steps.concat(
    credentialSteps(item.registry, item.token_secrets?.[0] ?? item.registry),
  );
}
