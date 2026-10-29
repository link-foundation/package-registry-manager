import { githubServices } from "./github.mjs";
import { repositorySlug } from "./repository-identity.mjs";

/** GitHub configuration is evidence about a channel, never public registry presence. */
export async function auditDistributionChannel(session) {
  const distribution = session.plan.package.distribution ?? {};
  const scope = {
    repo: repositorySlug(session.plan.repository),
    ...(distribution.environment
      ? { environment: distribution.environment }
      : {}),
  };
  const github = session.options.github ?? githubServices(session.options);
  const configured = distribution.remote
    ? { verified: true, value: distribution.remote }
    : await github.variables(scope).effective("CONAN_REMOTE_URL");
  let remote = { status: configured.verified ? "missing" : "unknown" };
  if (configured.value) {
    try {
      const url = new URL(configured.value);
      remote = {
        status:
          configured.verified &&
          ["https:", "http:"].includes(url.protocol) &&
          !url.username &&
          !url.password &&
          !url.search &&
          !url.hash
            ? "configured"
            : "invalid-or-secret-bearing",
      };
    } catch {
      remote = { status: "invalid" };
    }
  }
  const credentials = [];
  for (const name of ["CONAN_LOGIN_USERNAME", "CONAN_PASSWORD"]) {
    try {
      const result = await github.health.health(name, {
        scope: { repo: scope.repo },
        failurePatterns: ["conan.*(?:unauthorized|forbidden|authentication)"],
      });
      credentials.push({
        name,
        status: result.status,
        evidence: (result.evidence ?? []).map(
          ({ reason, status, workflow, job, step, runId, runUrl }) => ({
            reason,
            status,
            workflow,
            job,
            step,
            runId,
            runUrl,
          }),
        ),
        ...(scope.environment
          ? {
              limitation:
                "health is repository workflow evidence; effective environment credentials require the selected CI job",
            }
          : {}),
      });
    } catch {
      credentials.push({
        name,
        status: "unknown",
        reason: "credential-health-unreadable",
      });
    }
  }
  const result = {
    provider: "conan",
    scope,
    remote,
    credentials,
    publication: "unverified",
    ci_validation: [
      "conan create . --build=missing",
      "run the recipe test_package consumer with the reviewed profiles",
      "verify the exact reference on the chosen remote after reviewed CI publication",
    ],
  };
  session.outcome = { status: "unverified", distribution: result };
  console.log(JSON.stringify(result));
  return result;
}
