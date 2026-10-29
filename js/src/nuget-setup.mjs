import { readFile } from "node:fs/promises";
import path from "node:path";
import nugetPage from "./nuget-page.mjs";
import { validateNugetPolicy, nugetPolicyState } from "./nuget-policy.mjs";
import { githubServices } from "./github.mjs";
import { offerSignInImport, signInDomains } from "./sign-in-import.mjs";

const URL = "https://www.nuget.org/account/TrustedPublishing";

/** Account names are explicit configuration; GitHub immutable IDs are read fresh. */
export async function nugetSetupPolicy(session, github) {
  const config = JSON.parse(
    await readFile(
      path.join(session.options.repository, ".package-registry-manager.json"),
      "utf8",
    ).catch((error) => {
      if (error.code === "ENOENT") {
        return "{}";
      }
      throw error;
    }),
  );
  const publisher = session.plan.trusted_publisher;
  const settings = config.nuget ?? {};
  if (!publisher) {
    throw new Error(
      "NuGet requires an explicit GitHub repository and workflow",
    );
  }
  const repository = await github.rest.request(
    `/repos/${publisher.organization}/${publisher.repository}`,
  );
  const expected = validateNugetPolicy({
    ...settings,
    ...publisher,
    username: settings.username,
    package_owner: settings.package_owner,
    package_ids: settings.package_ids ?? [session.plan.package.name],
    operations: settings.operations,
    owner_id: String(repository.owner.id),
    repository_id: String(repository.id),
  });
  if (
    !expected.package_ids.some(
      (id) => id.toLowerCase() === session.plan.package.name.toLowerCase(),
    )
  ) {
    throw new Error("selected package is outside configured NuGet policy IDs");
  }
  return { expected, route: settings.sign_in_route ?? "work" };
}

/** Discover before creation, verify signed-in identity, and read server state again. */
export async function configureNuget(session) {
  const github = session.options.github ?? githubServices(session.options);
  const { expected, route } = await nugetSetupPolicy(session, github);
  session.options.domains = signInDomains("nuget", route);
  const script = (create = false) =>
    `(${nugetPage.toString()})(${JSON.stringify(expected)},${create})`;
  const inspect = async () => {
    const page = await session.visiblePage();
    await page.goto(URL);
    const deadline = Date.now() + (session.options.waitTimeoutMs ?? 600000);
    for (;;) {
      const snapshot = await page.evaluate(script());
      if (snapshot.ready || !snapshot.username || snapshot.error) {
        return snapshot;
      }
      if (Date.now() >= deadline) {
        throw new Error("NuGet policy page did not become ready");
      }
      await new Promise((resolve) =>
        setTimeout(resolve, session.options.pollIntervalMs ?? 500),
      );
    }
  };
  let snapshot = await inspect();
  if (snapshot.username?.toLowerCase() !== expected.username.toLowerCase()) {
    await offerSignInImport(session, session.options.domains, async () => {
      snapshot = await inspect();
      return (
        snapshot.username?.toLowerCase() === expected.username.toLowerCase()
      );
    });
  }
  const deadline = Date.now() + (session.options.waitTimeoutMs ?? 600000);
  while (
    !snapshot.ready ||
    snapshot.username?.toLowerCase() !== expected.username.toLowerCase()
  ) {
    if (snapshot.username) {
      throw new Error(
        `signed-in NuGet profile differs from configured ${expected.username}; policy was not changed`,
      );
    }
    if (session.options.yes) {
      throw new Error(
        "NuGet sign-in requires interaction; local session reuse did not verify the configured identity",
      );
    }
    await session.prompt(
      `Sign in as ${expected.username} using ${route}; complete password, 2FA or platform passkey authentication, then press Enter...`,
    );
    if (Date.now() >= deadline) {
      throw new Error("NuGet authentication remains unverified");
    }
    snapshot = await inspect();
  }
  let policyChanged = false;
  let state = nugetPolicyState(snapshot.policies, expected, snapshot.username);
  if (state.status === "missing-or-mismatched") {
    const submitted = await session.automation.evaluate(script(true));
    if (!submitted.submitted || submitted.error) {
      throw new Error(
        `NuGet policy was not created: ${submitted.error ?? "unsupported page"}`,
      );
    }
    while (true) {
      await new Promise((resolve) =>
        setTimeout(resolve, session.options.pollIntervalMs ?? 500),
      );
      const pending = await session.automation.evaluate(script());
      if (pending.error) {
        throw new Error(`NuGet policy creation failed: ${pending.error}`);
      }
      if (!pending.pending) {
        break;
      }
      if (Date.now() >= deadline) {
        throw new Error("NuGet policy creation remains pending");
      }
    }
    snapshot = await inspect();
    policyChanged = true;
    state = nugetPolicyState(snapshot.policies, expected, snapshot.username);
  }
  console.log(JSON.stringify({ nuget_policy: state }));
  if (!state.verified) {
    throw new Error(
      `NuGet policy is ${state.status}; publication remains unverified`,
    );
  }
  const scope = {
    repo: `${expected.organization}/${expected.repository}`,
    ...(expected.environment ? { environment: expected.environment } : {}),
  };
  const variable = await github
    .variables(scope)
    .ensure("NUGET_USER", expected.username);
  if (
    !variable.verified ||
    !variable.effective?.length ||
    variable.effective.some((value) => value.value !== expected.username)
  ) {
    throw new Error("effective NUGET_USER readback did not verify the profile");
  }
  session.nugetEvidence = {
    policy: state,
    variable,
    policy_changed: policyChanged,
  };
  if (policyChanged || variable.changed) {
    session.recordRecoveryChange();
  }
  session.conditions.delete("trust-missing");
  console.log(
    "NuGet policy and effective NUGET_USER verified; authentication exchange, actual push and public consumer validation remain separate CI gates. Existing fallback secrets are retained.",
  );
}
