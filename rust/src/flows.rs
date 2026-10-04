//! Registry flows: the ordered, conditional steps that take a package from
//! unpublished to trusted publishing without long-lived tokens.

use crate::model::{Package, Registry, SetupStep, StepKind};
use crate::prerequisites::{DEFAULT_TRUST_NPM, NPM_TFA_URL};
use crate::registry_state::{encode_uri_component, npm_name, Endpoints};
use crate::tokens::{token_secret_steps, CRATES_TOKENS_URL};

/// Step conditions that only hold while a package is not yet published.
pub const BOOTSTRAP_CONDITIONS: [&str; 2] = ["package-missing", "worktree-created"];

/// Step conditions evaluated in the cleanup phase, after every other step.
pub const CLEANUP_CONDITIONS: [&str; 2] = ["tool-signed-in", "worktree-created"];

/// Repository facts that shape a flow.
#[derive(Debug, Clone)]
pub struct FlowContext<'a> {
    /// Package directory relative to the repository root, or `.`.
    pub directory: &'a str,
    /// `owner/repository` on GitHub, when known.
    pub slug: Option<String>,
    /// Release workflow file name, when known.
    pub workflow: Option<String>,
    /// GitHub environment the trusted publisher is bound to, when any.
    pub environment: Option<String>,
    /// Append the release-verification steps.
    pub verify_release: bool,
    /// Registry API base URLs.
    pub endpoints: &'a Endpoints,
    /// The npm package spec that runs `npm trust`; `npm@^11.10` when unset.
    pub trust_npm: Option<&'a str>,
}

impl FlowContext<'_> {
    fn repo_args(&self) -> Vec<&str> {
        self.slug
            .as_deref()
            .map_or_else(Vec::new, |slug| vec!["--repo", slug])
    }

    fn package_cwd(&self) -> String {
        if self.directory == "." {
            "{worktree}".to_owned()
        } else {
            format!("{{worktree}}/{}", self.directory)
        }
    }

    fn state_url(&self, package: &Package) -> String {
        self.endpoints.state_url(package).unwrap_or_default()
    }
}

fn step(id: &str, title: &str, kind: StepKind, description: impl Into<String>) -> SetupStep {
    SetupStep::new(id, title, kind, description)
}

fn worktree_steps() -> [SetupStep; 2] {
    [
        step(
            "fetch-default-branch",
            "Fetch the default branch",
            StepKind::Command,
            "Fetch the remote default branch so the first release is built from a clean, pushed commit.",
        )
        .command("git", &["fetch", "origin", "HEAD"])
        .when("package-missing")
        .cwd("."),
        step(
            "prepare-worktree",
            "Check out a temporary worktree",
            StepKind::Command,
            "Check out the fetched commit into a temporary git worktree, leaving the working copy untouched.",
        )
        .command("git", &["worktree", "add", "--detach", "{worktree}", "FETCH_HEAD"])
        .when("package-missing")
        .cwd("."),
    ]
}

fn remove_worktree_step() -> SetupStep {
    step(
        "remove-worktree",
        "Remove the temporary worktree",
        StepKind::Command,
        "Delete the temporary worktree and packed artifact.",
    )
    .command("git", &["worktree", "remove", "--force", "{worktree}"])
    .when("worktree-created")
    .cwd(".")
}

fn check_registry_step(registry_name: &str, url: String) -> SetupStep {
    step(
        "check-registry",
        &format!("Look up the package on {registry_name}"),
        StepKind::Check,
        "A missing package needs a first publish (bootstrap); an existing one only needs trusted publishing attached.",
    )
    .url(url)
}

fn verify_release_steps(context: &FlowContext<'_>, workflow: &str, url: String) -> [SetupStep; 4] {
    let repo = context.repo_args();
    let mut run = vec!["workflow", "run", workflow];
    run.extend(&repo);
    let mut list = vec!["run", "list"];
    list.extend(&repo);
    list.extend([
        "--workflow",
        workflow,
        "--limit",
        "1",
        "--json",
        "databaseId,status,url",
    ]);
    let mut watch = vec!["run", "watch", "{run_id}"];
    watch.extend(&repo);
    watch.push("--exit-status");
    [
        step(
            "trigger-release",
            "Trigger the release workflow",
            StepKind::Command,
            "Start the release workflow; if it has no workflow_dispatch trigger, the latest run is watched instead.",
        )
        .command("gh", &run)
        .when("release-dispatch")
        .cwd("."),
        step(
            "find-release-run",
            "Find the release run",
            StepKind::Check,
            "Look up the newest run of the release workflow.",
        )
        .command("gh", &list)
        .when("release-dispatch")
        .cwd("."),
        step(
            "watch-release",
            "Watch the release run",
            StepKind::Command,
            "Follow the run until it finishes and fail if it fails.",
        )
        .command("gh", &watch)
        .when("verify-release")
        .cwd("."),
        step(
            "confirm-provenance",
            "Confirm the release used trusted publishing",
            StepKind::Wait,
            "Poll the registry until the latest version was published by the workflow through OIDC, with provenance.",
        )
        .url(url)
        .when("verify-release"),
    ]
}

fn two_factor_steps() -> [SetupStep; 3] {
    let profile = ["profile", "get", "--json"];
    [
        step(
            "check-2fa",
            "Check npm two-factor authentication",
            StepKind::Check,
            "npm trust requires account-level two-factor authentication, so check it before anything is published.",
        )
        .command("npm", &profile),
        step(
            "enable-2fa",
            "Turn on npm two-factor authentication",
            StepKind::Browser,
            "Open the npm account's 2FA settings in your browser and turn on two-factor authentication.",
        )
        .url(NPM_TFA_URL)
        .when("tfa-disabled"),
        step(
            "verify-2fa",
            "Verify npm two-factor authentication",
            StepKind::Check,
            "Stop before publishing if two-factor authentication is still off.",
        )
        .command("npm", &profile)
        .when("tfa-disabled"),
    ]
}

fn release_run_steps(context: &FlowContext<'_>, workflow: &str) -> [SetupStep; 2] {
    let repo = context.repo_args();
    let mut list = vec!["run", "list"];
    list.extend(&repo);
    list.extend([
        "--workflow",
        workflow,
        "--limit",
        "1",
        "--json",
        "databaseId,conclusion,status,url",
    ]);
    let mut view = vec!["run", "view", "{failed_run_id}"];
    view.extend(&repo);
    view.push("--log-failed");
    [
        step(
            "inspect-release-run",
            "Read the latest release run",
            StepKind::Check,
            "Look up the newest run of the release workflow; a run that npm rejected can be re-run once trust is attached.",
        )
        .command("gh", &list)
        .when("trust-missing")
        .cwd("."),
        step(
            "read-release-failure",
            "Read why the release run failed",
            StepKind::Check,
            "Search the failed jobs' log for npm's E404 or invalid-publisher rejection.",
        )
        .command("gh", &view)
        .when("release-failed")
        .cwd("."),
    ]
}

/// npm: web sign-in, one confirmed first publish from a clean worktree, then
/// `npm trust github` with a prefilled browser fallback.
#[must_use]
pub fn npm_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
    let name = package.name.as_str();
    let registry = context.endpoints.base(package.registry).unwrap_or_default();
    let trust_npm = context.trust_npm.unwrap_or(DEFAULT_TRUST_NPM);
    // npm trust list needs a 2FA approval; with --json npm holds the approval
    // URL back until it exits, so the human output is read instead (#24).
    let trust_list = ["-y", trust_npm, "trust", "list", name, "--browser=false"];
    let package_cwd = context.package_cwd();
    let mut steps = vec![
        step(
            "validate-package",
            "Validate npm metadata",
            StepKind::Check,
            "Read the package metadata with npm before changing registry settings.",
        )
        .command("npm", &["pkg", "get", "name", "version", "repository"]),
        check_registry_step("npm", context.state_url(package)),
        step(
            "check-sign-in",
            "Check the npm session",
            StepKind::Check,
            "Ask npm which account is signed in.",
        )
        .command("npm", &["whoami"]),
    ];
    steps.extend(worktree_steps());
    steps.extend([
        step(
            "pack",
            "Pack the package",
            StepKind::Command,
            "Pack the clean checkout without lifecycle scripts and print the file list and size for review.",
        )
        .command(
            "npm",
            &["pack", "--ignore-scripts", "--json", "--pack-destination", "{pack_destination}"],
        )
        .when("package-missing")
        .cwd(package_cwd.clone()),
        step(
            "test-install",
            "Install the packed tarball",
            StepKind::Command,
            "Install the tarball into a scratch directory, without lifecycle scripts, to test it the way users get it.",
        )
        .command(
            "npm",
            &[
                "install",
                "--no-save",
                "--no-package-lock",
                "--no-audit",
                "--no-fund",
                "--ignore-scripts",
                "--prefix",
                "{pack_destination}/install",
                "{tarball}",
            ],
        )
        .when("package-missing")
        .cwd("{pack_destination}"),
        step(
            "verify-bins",
            "Verify the packed bin entries",
            StepKind::Check,
            "Compare the bin entries of the packed package.json with package.json and check that each installed bin prints its version for --version.",
        )
        .when("package-missing"),
        // Sign in right before publishing, so the sign-in, publish, and trust
        // approvals happen together and npm can skip repeated 2FA prompts.
        step(
            "sign-in",
            "Sign in to npm in the browser",
            StepKind::Command,
            "Start a web login; the tool opens the printed URL in your default browser, where you are usually already signed in and only approve. No token is created or read by the tool.",
        )
        .command("npm", &["login", "--auth-type=web", "--browser=false"])
        .when("signed-out"),
    ]);
    steps.extend(two_factor_steps());
    steps.extend([
        step(
            "first-publish",
            "Publish the first version",
            StepKind::Command,
            "Publish the reviewed tarball once, approving 2FA in the browser. Later versions publish from CI through trusted publishing.",
        )
        .command(
            "npm",
            &[
                "publish",
                "{tarball}",
                "--access",
                "public",
                "--auth-type=web",
                "--browser=false",
                "--provenance=false",
            ],
        )
        .when("package-missing")
        .cwd(package_cwd)
        .confirmed(),
        step(
            "wait-for-registry",
            "Wait for the registry",
            StepKind::Wait,
            "Poll the registry until the published version is visible.",
        )
        .url(format!("{registry}/{}/{{version}}", npm_name(name)))
        .when("package-missing"),
    ]);
    let trusted_target = context.slug.as_deref().zip(context.workflow.as_deref());
    if let Some((slug, workflow)) = trusted_target {
        steps.extend([step(
            "check-trust",
            "Check trusted publishing",
            StepKind::Check,
            "List the trusted publishers configured for the package.",
        )
        .command("npx", &trust_list)]);
        steps.extend(release_run_steps(context, workflow));
        let mut trust_args = vec![
            "-y", trust_npm, "trust", "github", name, "--repo", slug, "--file", workflow,
        ];
        if let Some(environment) = context.environment.as_deref() {
            trust_args.extend(["--env", environment]);
        }
        trust_args.extend(["--allow-publish", "--yes", "--browser=false"]);
        steps.extend([
            step(
                "attach-trusted-publisher",
                "Attach the GitHub Actions trusted publisher",
                StepKind::Command,
                format!("Trust the release workflow to publish through OIDC with {trust_npm}, run through npx; npm trust needs npm 11.10 or newer and account-level 2FA."),
            )
            .command("npx", &trust_args)
            .when("trust-missing"),
        ]);
    }
    steps.push(
        step(
            "configure-trusted-publisher",
            "Configure npm trusted publishing in the browser",
            StepKind::Browser,
            "Fallback when the npm CLI cannot attach the publisher: review the prefilled GitHub Actions identity and explicitly confirm submission.",
        )
        .url(format!(
            "https://www.npmjs.com/package/{}/access",
            url_path_segment(name)
        ))
        .when(if trusted_target.is_some() {
            "trust-cli-failed"
        } else {
            "trust-missing"
        }),
    );
    if let Some((slug, workflow)) = trusted_target {
        steps.push(
            step(
                "verify-trusted-publisher",
                "Verify trusted publishing",
                StepKind::Check,
                "Confirm that npm lists the GitHub Actions trusted publisher.",
            )
            .command("npx", &trust_list)
            .when("trust-missing"),
        );
        steps.extend(token_secret_steps(package, slug));
        steps.push(rerun_release_step(context));
        if context.verify_release {
            steps.extend(verify_release_steps(
                context,
                workflow,
                context.state_url(package),
            ));
        }
    }
    steps.extend([
        step(
            "sign-out",
            "Sign out of npm",
            StepKind::Command,
            "Revoke the session token created by the web login.",
        )
        .command("npm", &["logout"])
        .when("tool-signed-in"),
        remove_worktree_step(),
    ]);
    steps
}

fn rerun_release_step(context: &FlowContext<'_>) -> SetupStep {
    let mut rerun = vec!["run", "rerun", "{failed_run_id}"];
    rerun.extend(context.repo_args());
    rerun.push("--failed");
    step(
        "rerun-release",
        "Re-run the failed release jobs",
        StepKind::Command,
        "Re-run the jobs of the release run that npm rejected, now that the trusted publisher is attached.",
    )
    .command("gh", &rerun)
    .when("release-failed-publish")
    .cwd(".")
    .confirmed()
}

/// crates.io: a short-lived publish-new token entered into `cargo login` for
/// the first upload only, then trusted publishing in the browser.
#[must_use]
pub fn crates_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
    let name = package.name.as_str();
    let api = context.state_url(package);
    let mut steps = vec![
        step(
            "validate-package",
            "Validate the crate",
            StepKind::Check,
            "Package the crate without uploading it.",
        )
        .command("cargo", &["publish", "--dry-run"]),
        check_registry_step("crates.io", api.clone()),
    ];
    steps.extend(worktree_steps());
    steps.extend([
        step(
            "create-publish-token",
            "Create a one-time first-publish token",
            StepKind::Browser,
            format!("A one-time exception that needs your approval: crates.io has no token-free first publish (no pending publisher as on PyPI), so the first upload needs an API token. Create one limited to the publish-new scope and the crate name {name}, expiring in a day; paste it only into cargo login in the next step. It is revoked, and the revocation verified, before setup ends."),
        )
        .url("https://crates.io/settings/tokens/new")
        .when("package-missing")
        .confirmed(),
        step(
            "sign-in",
            "Sign cargo in",
            StepKind::Command,
            "cargo reads the token from the terminal, so it is never typed into the tool.",
        )
        .command("cargo", &["login"])
        .when("package-missing"),
        step(
            "first-publish",
            "Publish the first version",
            StepKind::Command,
            "Publish once from the clean checkout. Later versions publish from CI through trusted publishing.",
        )
        .command("cargo", &["publish"])
        .when("package-missing")
        .cwd(context.package_cwd())
        .confirmed(),
        step(
            "wait-for-registry",
            "Wait for the registry",
            StepKind::Wait,
            "Poll crates.io until the crate is visible.",
        )
        .url(api)
        .when("package-missing"),
        step(
            "revoke-publish-token",
            "Revoke the first-publish token",
            StepKind::Browser,
            "Revoke the token created for the first publish; it is no longer needed.",
        )
        .url(CRATES_TOKENS_URL)
        .when("package-missing"),
        step(
            "verify-token-revoked",
            "Verify the first-publish token is revoked",
            StepKind::Check,
            "Send the token cargo stored to crates.io, which must reject it; the token is never printed.",
        )
        .url(format!(
            "{}/me/tokens",
            context.endpoints.base(Registry::CratesIo).unwrap_or_default()
        ))
        .when("package-missing"),
        step(
            "configure-trusted-publisher",
            "Configure crates.io trusted publishing",
            StepKind::Browser,
            "Add the repository's GitHub Actions workflow as the crate's trusted publisher, then review and submit.",
        )
        .url(format!(
            "https://crates.io/crates/{}/settings/new-trusted-publisher",
            encode_uri_component(name)
        ))
        .when("trust-missing"),
    ]);
    if let Some(slug) = context.slug.as_deref() {
        steps.extend(token_secret_steps(package, slug));
    }
    steps.extend([
        step(
            "sign-out",
            "Sign cargo out",
            StepKind::Command,
            "Remove the local copy of the first-publish token.",
        )
        .command("cargo", &["logout"])
        .when("tool-signed-in"),
        remove_worktree_step(),
    ]);
    steps
}

/// `PyPI`: a pending publisher lets CI perform the first upload through OIDC.
#[must_use]
pub fn pypi_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
    let state_url = context.state_url(package);
    let mut steps = vec![
        step(
            "build-package",
            "Build the Python distribution",
            StepKind::Check,
            "Build source and wheel distributions locally.",
        )
        .command("python", &["-m", "build"]),
        check_registry_step("PyPI", state_url.clone()),
        step(
            "create-pending-publisher",
            "Create a PyPI pending publisher",
            StepKind::Browser,
            "Register the GitHub Actions workflow as a pending publisher, so the first upload comes from CI through OIDC. A pending publisher does not reserve the name.",
        )
        .url("https://pypi.org/manage/account/publishing/")
        .when("package-missing"),
    ];
    if let Some(workflow) = context.workflow.as_deref() {
        let mut args = vec!["workflow", "run", workflow];
        args.extend(context.repo_args());
        steps.push(
            step(
                "trigger-release",
                "Run the release workflow",
                StepKind::Command,
                "Start the release workflow, which performs the first upload through OIDC with no manual upload.",
            )
            .command("gh", &args)
            .when("package-missing")
            .cwd("."),
        );
    }
    steps.extend([
        step(
            "wait-for-registry",
            "Wait for the registry",
            StepKind::Wait,
            "Poll PyPI until the project is visible.",
        )
        .url(state_url)
        .when("package-missing"),
        step(
            "configure-trusted-publisher",
            "Configure a PyPI trusted publisher",
            StepKind::Browser,
            "Sign in and add the repository's GitHub Actions workflow as a trusted publisher.",
        )
        .url(format!(
            "https://pypi.org/manage/project/{}/settings/publishing/",
            url_path_segment(&package.name)
        ))
        .when("trust-missing"),
    ]);
    if let Some(slug) = context.slug.as_deref() {
        steps.extend(token_secret_steps(package, slug));
    }
    steps
}

/// Docker Hub: create the repository and store the image, account, and a
/// scoped access token in GitHub; `gh` reads the token from the terminal.
#[must_use]
pub fn docker_hub_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
    let name = package.name.as_str();
    let namespace = name.split('/').next().unwrap_or_default();
    let repo = context.repo_args();
    vec![
        check_registry_step("Docker Hub", context.state_url(package)),
        step(
            "create-repository",
            "Create the Docker Hub repository",
            StepKind::Browser,
            format!("Create the public repository {name}."),
        )
        .url(format!(
            "https://hub.docker.com/repository/create?namespace={}",
            encode_uri_component(namespace)
        ))
        .when("package-missing"),
        step(
            "create-access-token",
            "Create a Read & Write access token",
            StepKind::Browser,
            format!("Create a token with Read & Write access and a short expiry. Personal access tokens cannot be limited to one repository; an organization access token can be restricted to {name}. Keep the token only for the next step."),
        )
        .url("https://app.docker.com/settings/personal-access-tokens/create"),
        step(
            "check-github-cli",
            "Check the GitHub CLI session",
            StepKind::Check,
            "gh stores the variables and secret in the repository.",
        )
        .command("gh", &["auth", "status"])
        .cwd("."),
        step(
            "set-image-variable",
            "Set DOCKERHUB_IMAGE",
            StepKind::Command,
            "Store the image name as a repository variable.",
        )
        .command("gh", &with_repo(&repo, &["variable", "set", "DOCKERHUB_IMAGE", "--body", name]))
        .cwd("."),
        step(
            "set-username-variable",
            "Set DOCKERHUB_USERNAME",
            StepKind::Command,
            "Store the Docker Hub account as a repository variable.",
        )
        .command(
            "gh",
            &with_repo(&repo, &["variable", "set", "DOCKERHUB_USERNAME", "--body", namespace]),
        )
        .cwd("."),
        step(
            "set-token-secret",
            "Set DOCKERHUB_TOKEN",
            StepKind::Command,
            "gh reads the token from the terminal without echoing it; the tool never sees it.",
        )
        .command("gh", &with_repo(&repo, &["secret", "set", "DOCKERHUB_TOKEN"]))
        .cwd(".")
        .confirmed(),
    ]
}

/// GHCR: `GITHUB_TOKEN` pushes with `packages: write`; after the first push
/// the package is linked to the repository.
#[must_use]
pub fn ghcr_flow(package: &Package, context: &FlowContext<'_>) -> Vec<SetupStep> {
    let mut parts = package.name.split('/');
    let owner = parts.next().unwrap_or_default();
    let rest = parts
        .map(encode_uri_component)
        .collect::<Vec<_>>()
        .join("%2F");
    let mut steps = Vec::new();
    if !package.warnings.is_empty() {
        steps.push(
            step(
                "grant-packages-write",
                "Grant packages: write",
                StepKind::Manual,
                format!(
                    "Add \"permissions: packages: write\" to the job in {} that pushes to ghcr.io.",
                    context.workflow.as_deref().unwrap_or("undefined")
                ),
            )
            .url("https://docs.github.com/en/packages/managing-github-packages-using-github-actions-workflows/publishing-and-installing-a-package-with-github-actions"),
        );
    }
    steps.push(
        step(
            "link-package",
            "Link the package to the repository",
            StepKind::Browser,
            format!(
                "After the first push, open Package settings, connect {} and grant it Actions access. A LABEL org.opencontainers.image.source=https://github.com/{} in the Dockerfile links it automatically.",
                context.slug.as_deref().unwrap_or("the repository"),
                context.slug.as_deref().unwrap_or("<owner>/<repository>")
            ),
        )
        .url(format!(
            "https://github.com/users/{}/packages/container/package/{rest}",
            encode_uri_component(owner)
        )),
    );
    steps
}

fn with_repo<'a>(repo: &[&'a str], args: &[&'a str]) -> Vec<&'a str> {
    let mut args = args.to_vec();
    args.extend_from_slice(repo);
    args
}

/// Encode a URL path segment, keeping `@` readable for scoped npm names.
#[must_use]
pub fn url_path_segment(value: &str) -> String {
    encode_uri_component(value).replace("%40", "@")
}
