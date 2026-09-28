import {
  npmName,
  registryEndpoint,
  registryStateUrl,
} from "./registry-state.mjs";

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
        when: "verify-release",
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
        when: "verify-release",
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

export function npmFlow(packageInfo, context) {
  const name = packageInfo.name;
  const registry = registryEndpoint("npm");
  const trustList = command("npx", [
    "-y",
    "npm@latest",
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
    step(
      "sign-in",
      "Sign in to npm in the browser",
      "command",
      "Start a web login; the tool opens the printed URL in its browser, where you sign in and approve 2FA. No token is created or read by the tool.",
      {
        command: command("npm", [
          "login",
          "--auth-type=web",
          "--browser=false",
        ]),
        when: "signed-out",
      },
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
      step(
        "attach-trusted-publisher",
        "Attach the GitHub Actions trusted publisher",
        "command",
        "Trust the release workflow to publish through OIDC (npm 11.10 or newer, run through npx).",
        {
          command: command("npx", [
            "-y",
            "npm@latest",
            "trust",
            "github",
            name,
            "--repo",
            context.slug,
            "--file",
            context.workflow,
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
      step(
        "audit-token-secrets",
        "Look for a leftover NPM_TOKEN secret",
        "check",
        "Trusted publishing makes long-lived npm tokens unnecessary.",
        {
          command: command("gh", [
            "secret",
            "list",
            "--repo",
            context.slug,
            "--json",
            "name",
          ]),
          cwd: ".",
        },
      ),
      step(
        "delete-token-secret",
        "Delete the NPM_TOKEN secret",
        "command",
        "Remove the unused long-lived token secret from the repository.",
        {
          command: command("gh", [
            "secret",
            "delete",
            "NPM_TOKEN",
            "--repo",
            context.slug,
          ]),
          when: "token-secret-present",
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
  return [
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
      "Create a first-publish token",
      "browser",
      `crates.io requires an API token for a crate's first upload. Create one limited to the publish-new scope and the crate name ${name}, with a short expiry; paste it only into cargo login in the next step.`,
      { url: "https://crates.io/settings/tokens/new", when: "package-missing" },
    ),
    step(
      "sign-in",
      "Sign cargo in",
      "command",
      "cargo reads the token from the terminal; the tool never sees it.",
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
      { url: "https://crates.io/settings/tokens", when: "package-missing" },
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
    step(
      "sign-out",
      "Sign cargo out",
      "command",
      "Remove the local copy of the first-publish token.",
      { command: command("cargo", ["logout"]), when: "tool-signed-in" },
    ),
    removeWorktreeStep(),
  ];
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
