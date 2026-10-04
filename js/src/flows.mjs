import { DEFAULT_TRUST_NPM, NPM_TFA_URL } from "./prerequisites.mjs";
import {
  npmName,
  registryEndpoint,
  registryStateUrl,
} from "./registry-state.mjs";
import { CRATES_TOKENS_URL, tokenSecretSteps } from "./tokens.mjs";

/** Step conditions that only hold while a package is not yet published. */
export const BOOTSTRAP_CONDITIONS = new Set([
  "package-missing",
  "worktree-created",
]);

/** Step conditions evaluated in the cleanup phase, after every other step. */
export const CLEANUP_CONDITIONS = new Set([
  "tool-signed-in",
  "worktree-created",
]);

const command = (program, args) => ({ program, args });

function step(id, title, kind, description, fields = {}) {
  return { id, title, kind, description, ...fields };
}

function packageCwd(directory) {
  return directory === "." ? "{worktree}" : `{worktree}/${directory}`;
}

function repoArgs(context) {
  return context.slug ? ["--repo", context.slug] : [];
}

function worktreeSteps() {
  return [
    step(
      "fetch-default-branch",
      "Fetch the default branch",
      "command",
      "Fetch the remote default branch so the first release is built from a clean, pushed commit.",
      {
        command: command("git", ["fetch", "origin", "HEAD"]),
        when: "package-missing",
        cwd: ".",
      },
    ),
    step(
      "prepare-worktree",
      "Check out a temporary worktree",
      "command",
      "Check out the fetched commit into a temporary git worktree, leaving the working copy untouched.",
      {
        command: command("git", [
          "worktree",
          "add",
          "--detach",
          "{worktree}",
          "FETCH_HEAD",
        ]),
        when: "package-missing",
        cwd: ".",
      },
    ),
  ];
}

function removeWorktreeStep() {
  return step(
    "remove-worktree",
    "Remove the temporary worktree",
    "command",
    "Delete the temporary worktree and packed artifact.",
    {
      command: command("git", ["worktree", "remove", "--force", "{worktree}"]),
      when: "worktree-created",
      cwd: ".",
    },
  );
}

function checkRegistryStep(registryName, url) {
  return step(
    "check-registry",
    `Look up the package on ${registryName}`,
    "check",
    "A missing package needs a first publish (bootstrap); an existing one only needs trusted publishing attached.",
    { url },
  );
}

function verifyReleaseSteps(context, url) {
  const repo = repoArgs(context);
  return [
    step(
      "trigger-release",
      "Trigger the release workflow",
      "command",
      "Start the release workflow; if it has no workflow_dispatch trigger, the latest run is watched instead.",
      {
        command: command("gh", ["workflow", "run", context.workflow, ...repo]),
        when: "release-dispatch",
        cwd: ".",
      },
    ),
    step(
      "find-release-run",
      "Find the release run",
      "check",
      "Look up the newest run of the release workflow.",
      {
        command: command("gh", [
          "run",
          "list",
          ...repo,
          "--workflow",
          context.workflow,
          "--limit",
          "1",
          "--json",
          "databaseId,status,url",
        ]),
        when: "release-dispatch",
        cwd: ".",
      },
    ),
    step(
      "watch-release",
      "Watch the release run",
      "command",
      "Follow the run until it finishes and fail if it fails.",
      {
        command: command("gh", [
          "run",
          "watch",
          "{run_id}",
          ...repo,
          "--exit-status",
        ]),
        when: "verify-release",
        cwd: ".",
      },
    ),
    step(
      "confirm-provenance",
      "Confirm the release used trusted publishing",
      "wait",
      "Poll the registry until the latest version was published by the workflow through OIDC, with provenance.",
      { url, when: "verify-release" },
    ),
  ];
}

function twoFactorSteps() {
  const profile = command("npm", ["profile", "get", "--json"]);
  return [
    step(
      "check-2fa",
      "Check npm two-factor authentication",
      "check",
      "npm trust requires account-level two-factor authentication, so check it before anything is published.",
      { command: profile },
    ),
    step(
      "enable-2fa",
      "Turn on npm two-factor authentication",
      "browser",
      "Open the npm account's 2FA settings in your browser and turn on two-factor authentication.",
      { url: NPM_TFA_URL, when: "tfa-disabled" },
    ),
    step(
      "verify-2fa",
      "Verify npm two-factor authentication",
      "check",
      "Stop before publishing if two-factor authentication is still off.",
      { command: profile, when: "tfa-disabled" },
    ),
  ];
}

function releaseRunSteps(context) {
  const repo = repoArgs(context);
  return [
    step(
      "inspect-release-run",
      "Read the latest release run",
      "check",
      "Look up the newest run of the release workflow; a run that npm rejected can be re-run once trust is attached.",
      {
        command: command("gh", [
          "run",
          "list",
          ...repo,
          "--workflow",
          context.workflow,
          "--limit",
          "1",
          "--json",
          "databaseId,conclusion,status,url",
        ]),
        when: "trust-missing",
        cwd: ".",
      },
    ),
    step(
      "read-release-failure",
      "Read why the release run failed",
      "check",
      "Search the failed jobs' log for npm's E404 or invalid-publisher rejection.",
      {
        command: command("gh", [
          "run",
          "view",
          "{failed_run_id}",
          ...repo,
          "--log-failed",
        ]),
        when: "release-failed",
        cwd: ".",
      },
    ),
  ];
}

export function npmFlow(packageInfo, context) {
  const name = packageInfo.name;
  const registry = registryEndpoint("npm");
  const trustNpm = context.trustNpm ?? DEFAULT_TRUST_NPM;
  const trustList = command("npx", [
    "-y",
    trustNpm,
    "trust",
    "list",
    name,
    "--json",
  ]);
  const steps = [
    step(
      "validate-package",
      "Validate npm metadata",
      "check",
      "Read the package metadata with npm before changing registry settings.",
      {
        command: command("npm", [
          "pkg",
          "get",
          "name",
          "version",
          "repository",
        ]),
      },
    ),
    checkRegistryStep("npm", registryStateUrl(packageInfo)),
    step(
      "check-sign-in",
      "Check the npm session",
      "check",
      "Ask npm which account is signed in.",
      { command: command("npm", ["whoami"]) },
    ),
    ...worktreeSteps(),
    step(
      "pack",
      "Pack the package",
      "command",
      "Pack the clean checkout without lifecycle scripts and print the file list and size for review.",
      {
        command: command("npm", [
          "pack",
          "--ignore-scripts",
          "--json",
          "--pack-destination",
          "{pack_destination}",
        ]),
        when: "package-missing",
        cwd: packageCwd(context.directory),
      },
    ),
    step(
      "test-install",
      "Install the packed tarball",
      "command",
      "Install the tarball into a scratch directory, without lifecycle scripts, to test it the way users get it.",
      {
        command: command("npm", [
          "install",
          "--no-save",
          "--no-package-lock",
          "--no-audit",
          "--no-fund",
          "--ignore-scripts",
          "--prefix",
          "{pack_destination}/install",
          "{tarball}",
        ]),
        when: "package-missing",
        cwd: "{pack_destination}",
      },
    ),
    step(
      "verify-bins",
      "Verify the packed bin entries",
      "check",
      "Compare the bin entries of the packed package.json with package.json and check that each installed bin prints its version for --version.",
      { when: "package-missing" },
    ),
    // Sign in right before publishing, so the sign-in, publish, and trust
    // approvals happen together and npm can skip repeated 2FA prompts.
    step(
      "sign-in",
      "Sign in to npm in the browser",
      "command",
      "Start a web login; the tool opens the printed URL in your default browser, where you are usually already signed in and only approve. No token is created or read by the tool.",
      {
        command: command("npm", [
          "login",
          "--auth-type=web",
          "--browser=false",
        ]),
        when: "signed-out",
      },
    ),
    ...twoFactorSteps(),
    step(
      "first-publish",
      "Publish the first version",
      "command",
      "Publish the reviewed tarball once, approving 2FA in the browser. Later versions publish from CI through trusted publishing.",
      {
        command: command("npm", [
          "publish",
          "{tarball}",
          "--access",
          "public",
          "--auth-type=web",
          "--browser=false",
          "--provenance=false",
        ]),
        when: "package-missing",
        cwd: packageCwd(context.directory),
        confirm: true,
      },
    ),
    step(
      "wait-for-registry",
      "Wait for the registry",
      "wait",
      "Poll the registry until the published version is visible.",
      {
        url: `${registry}/${npmName(name)}/{version}`,
        when: "package-missing",
      },
    ),
  ];
  if (context.slug && context.workflow) {
    steps.push(
      step(
        "check-trust",
        "Check trusted publishing",
        "check",
        "List the trusted publishers configured for the package.",
        { command: trustList },
      ),
      ...releaseRunSteps(context),
      step(
        "attach-trusted-publisher",
        "Attach the GitHub Actions trusted publisher",
        "command",
        `Trust the release workflow to publish through OIDC with ${trustNpm}, run through npx; npm trust needs npm 11.10 or newer and account-level 2FA.`,
        {
          command: command("npx", [
            "-y",
            trustNpm,
            "trust",
            "github",
            name,
            "--repo",
            context.slug,
            "--file",
            context.workflow,
            ...(context.environment ? ["--env", context.environment] : []),
            "--allow-publish",
            "--yes",
            "--browser=false",
          ]),
          when: "trust-missing",
        },
      ),
    );
  }
  steps.push(
    step(
      "configure-trusted-publisher",
      "Configure npm trusted publishing in the browser",
      "browser",
      "Fallback when the npm CLI cannot attach the publisher: review the prefilled GitHub Actions identity and explicitly confirm submission.",
      {
        url: `https://www.npmjs.com/package/${urlPathSegment(name)}/access`,
        when:
          context.slug && context.workflow
            ? "trust-cli-failed"
            : "trust-missing",
      },
    ),
  );
  if (context.slug && context.workflow) {
    steps.push(
      step(
        "verify-trusted-publisher",
        "Verify trusted publishing",
        "check",
        "Confirm that npm lists the GitHub Actions trusted publisher.",
        { command: trustList, when: "trust-missing" },
      ),
      ...tokenSecretSteps(packageInfo, context.slug),
      step(
        "rerun-release",
        "Re-run the failed release jobs",
        "command",
        "Re-run the jobs of the release run that npm rejected, now that the trusted publisher is attached.",
        {
          command: command("gh", [
            "run",
            "rerun",
            "{failed_run_id}",
            ...repoArgs(context),
            "--failed",
          ]),
          when: "release-failed-publish",
          cwd: ".",
          confirm: true,
        },
      ),
    );
    if (context.verifyRelease) {
      steps.push(...verifyReleaseSteps(context, registryStateUrl(packageInfo)));
    }
  }
  steps.push(
    step(
      "sign-out",
      "Sign out of npm",
      "command",
      "Revoke the session token created by the web login.",
      { command: command("npm", ["logout"]), when: "tool-signed-in" },
    ),
    removeWorktreeStep(),
  );
  return steps;
}

export function cratesFlow(packageInfo, context) {
  const name = packageInfo.name;
  const api = registryStateUrl(packageInfo);
  const steps = [
    step(
      "validate-package",
      "Validate the crate",
      "check",
      "Package the crate without uploading it.",
      { command: command("cargo", ["publish", "--dry-run"]) },
    ),
    checkRegistryStep("crates.io", api),
    ...worktreeSteps(),
    step(
      "create-publish-token",
      "Create a one-time first-publish token",
      "browser",
      `A one-time exception that needs your approval: crates.io has no token-free first publish (no pending publisher as on PyPI), so the first upload needs an API token. Create one limited to the publish-new scope and the crate name ${name}, expiring in a day; paste it only into cargo login in the next step. It is revoked, and the revocation verified, before setup ends.`,
      {
        url: "https://crates.io/settings/tokens/new",
        when: "package-missing",
        confirm: true,
      },
    ),
    step(
      "sign-in",
      "Sign cargo in",
      "command",
      "cargo reads the token from the terminal, so it is never typed into the tool.",
      { command: command("cargo", ["login"]), when: "package-missing" },
    ),
    step(
      "first-publish",
      "Publish the first version",
      "command",
      "Publish once from the clean checkout. Later versions publish from CI through trusted publishing.",
      {
        command: command("cargo", ["publish"]),
        when: "package-missing",
        cwd: packageCwd(context.directory),
        confirm: true,
      },
    ),
    step(
      "wait-for-registry",
      "Wait for the registry",
      "wait",
      "Poll crates.io until the crate is visible.",
      { url: api, when: "package-missing" },
    ),
    step(
      "revoke-publish-token",
      "Revoke the first-publish token",
      "browser",
      "Revoke the token created for the first publish; it is no longer needed.",
      { url: CRATES_TOKENS_URL, when: "package-missing" },
    ),
    step(
      "verify-token-revoked",
      "Verify the first-publish token is revoked",
      "check",
      "Send the token cargo stored to crates.io, which must reject it; the token is never printed.",
      {
        url: `${registryEndpoint("crates-io")}/me/tokens`,
        when: "package-missing",
      },
    ),
    step(
      "configure-trusted-publisher",
      "Configure crates.io trusted publishing",
      "browser",
      "Add the repository's GitHub Actions workflow as the crate's trusted publisher, then review and submit.",
      {
        url: `https://crates.io/crates/${encodeURIComponent(name)}/settings/new-trusted-publisher`,
        when: "trust-missing",
      },
    ),
  ];
  if (context.slug) {
    steps.push(...tokenSecretSteps(packageInfo, context.slug));
  }
  steps.push(
    step(
      "sign-out",
      "Sign cargo out",
      "command",
      "Remove the local copy of the first-publish token.",
      { command: command("cargo", ["logout"]), when: "tool-signed-in" },
    ),
    removeWorktreeStep(),
  );
  return steps;
}

export function pypiFlow(packageInfo, context) {
  const name = packageInfo.name;
  const steps = [
    step(
      "build-package",
      "Build the Python distribution",
      "check",
      "Build source and wheel distributions locally.",
      { command: command("python", ["-m", "build"]) },
    ),
    checkRegistryStep("PyPI", registryStateUrl(packageInfo)),
    step(
      "create-pending-publisher",
      "Create a PyPI pending publisher",
      "browser",
      "Register the GitHub Actions workflow as a pending publisher, so the first upload comes from CI through OIDC. A pending publisher does not reserve the name.",
      {
        url: "https://pypi.org/manage/account/publishing/",
        when: "package-missing",
      },
    ),
  ];
  if (context.workflow) {
    steps.push(
      step(
        "trigger-release",
        "Run the release workflow",
        "command",
        "Start the release workflow, which performs the first upload through OIDC with no manual upload.",
        {
          command: command("gh", [
            "workflow",
            "run",
            context.workflow,
            ...repoArgs(context),
          ]),
          when: "package-missing",
          cwd: ".",
        },
      ),
    );
  }
  steps.push(
    step(
      "wait-for-registry",
      "Wait for the registry",
      "wait",
      "Poll PyPI until the project is visible.",
      { url: registryStateUrl(packageInfo), when: "package-missing" },
    ),
    step(
      "configure-trusted-publisher",
      "Configure a PyPI trusted publisher",
      "browser",
      "Sign in and add the repository's GitHub Actions workflow as a trusted publisher.",
      {
        url: `https://pypi.org/manage/project/${urlPathSegment(name)}/settings/publishing/`,
        when: "trust-missing",
      },
    ),
  );
  if (context.slug) {
    steps.push(...tokenSecretSteps(packageInfo, context.slug));
  }
  return steps;
}

export function dockerHubFlow(packageInfo, context) {
  const [namespace] = packageInfo.name.split("/");
  const repo = repoArgs(context);
  return [
    checkRegistryStep("Docker Hub", registryStateUrl(packageInfo)),
    step(
      "create-repository",
      "Create the Docker Hub repository",
      "browser",
      `Create the public repository ${packageInfo.name}.`,
      {
        url: `https://hub.docker.com/repository/create?namespace=${encodeURIComponent(namespace)}`,
        when: "package-missing",
      },
    ),
    step(
      "create-access-token",
      "Create a Read & Write access token",
      "browser",
      `Create a token with Read & Write access and a short expiry. Personal access tokens cannot be limited to one repository; an organization access token can be restricted to ${packageInfo.name}. Keep the token only for the next step.`,
      { url: "https://app.docker.com/settings/personal-access-tokens/create" },
    ),
    step(
      "check-github-cli",
      "Check the GitHub CLI session",
      "check",
      "gh stores the variables and secret in the repository.",
      { command: command("gh", ["auth", "status"]), cwd: "." },
    ),
    step(
      "set-image-variable",
      "Set DOCKERHUB_IMAGE",
      "command",
      "Store the image name as a repository variable.",
      {
        command: command("gh", [
          "variable",
          "set",
          "DOCKERHUB_IMAGE",
          "--body",
          packageInfo.name,
          ...repo,
        ]),
        cwd: ".",
      },
    ),
    step(
      "set-username-variable",
      "Set DOCKERHUB_USERNAME",
      "command",
      "Store the Docker Hub account as a repository variable.",
      {
        command: command("gh", [
          "variable",
          "set",
          "DOCKERHUB_USERNAME",
          "--body",
          namespace,
          ...repo,
        ]),
        cwd: ".",
      },
    ),
    step(
      "set-token-secret",
      "Set DOCKERHUB_TOKEN",
      "command",
      "gh reads the token from the terminal without echoing it; the tool never sees it.",
      {
        command: command("gh", ["secret", "set", "DOCKERHUB_TOKEN", ...repo]),
        cwd: ".",
        confirm: true,
      },
    ),
  ];
}

export function ghcrFlow(packageInfo, context) {
  const [owner, ...rest] = packageInfo.name.split("/");
  const steps = [];
  if ((packageInfo.warnings ?? []).length > 0) {
    steps.push(
      step(
        "grant-packages-write",
        "Grant packages: write",
        "manual",
        `Add "permissions: packages: write" to the job in ${context.workflow} that pushes to ghcr.io.`,
        {
          url: "https://docs.github.com/en/packages/managing-github-packages-using-github-actions-workflows/publishing-and-installing-a-package-with-github-actions",
        },
      ),
    );
  }
  steps.push(
    step(
      "link-package",
      "Link the package to the repository",
      "browser",
      `After the first push, open Package settings, connect ${context.slug ?? "the repository"} and grant it Actions access. A LABEL org.opencontainers.image.source=https://github.com/${context.slug ?? "<owner>/<repository>"} in the Dockerfile links it automatically.`,
      {
        url: `https://github.com/users/${encodeURIComponent(owner)}/packages/container/package/${rest.map(encodeURIComponent).join("%2F")}`,
      },
    ),
  );
  return steps;
}

export function urlPathSegment(value) {
  return encodeURIComponent(value).replaceAll("%40", "@");
}
