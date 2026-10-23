//! Regression tests for package-registry-manager issues #16 and #62 (CI half).
//!
//! The release workflow published with long-lived registry tokens
//! (`secrets.CARGO_TOKEN`, `secrets.CARGO_REGISTRY_TOKEN`, `secrets.NPM_TOKEN`).
//! Issue #16 moved crates.io and npm to trusted publishing (OIDC); issue #62
//! keeps the token secrets as a fallback for when the OIDC exchange fails:
//!
//! * the two crate-publishing jobs hold `id-token: write`, mint a short-lived
//!   crates.io token with a hash-pinned rust-lang/crates-io-auth-action
//!   (`continue-on-error`, so a failed exchange does not end the job), and
//!   hand it -- or, failing that, the token secret -- to the publish step alone;
//! * a crate that is not on crates.io yet (trusted publishing cannot create
//!   one) skips crates.io only -- the GitHub release, npm and Docker go ahead;
//! * npm publishes with OIDC first, `NODE_AUTH_TOKEN` from `NPM_TOKEN` as the
//!   publish step's fallback, and only when the package exists.

use std::fs;

fn read(relative: &str) -> String {
    fs::read_to_string(format!("{}/{relative}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|error| panic!("{relative} should be readable: {error}"))
        .replace("\r\n", "\n")
}

fn release_workflow() -> String {
    read("../.github/workflows/release.yml")
}

fn job_block<'a>(workflow: &'a str, job_name: &str) -> &'a str {
    let marker = format!("  {job_name}:\n");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("release.yml should declare the {job_name} job"));
    let body_start = start + marker.len();
    let rest = &workflow[body_start..];

    let next_job = rest
        .lines()
        .scan(0usize, |offset, line| {
            let current_offset = *offset;
            *offset += line.len() + 1;
            Some((current_offset, line))
        })
        .find_map(|(offset, line)| {
            let starts_at_job_indent = line.starts_with("  ") && !line.starts_with("    ");
            (starts_at_job_indent && line.trim_end().ends_with(':')).then_some(offset)
        });

    next_job.map_or_else(
        || &workflow[start..],
        |end| &workflow[start..body_start + end],
    )
}

fn step_block<'a>(job: &'a str, step_name: &str) -> &'a str {
    let marker = format!("      - name: {step_name}\n");
    let start = job
        .find(&marker)
        .unwrap_or_else(|| panic!("could not find workflow step {step_name:?}"));
    let body_start = start + marker.len();
    let rest = &job[body_start..];

    let next_step = rest
        .lines()
        .scan(0usize, |offset, line| {
            let current_offset = *offset;
            *offset += line.len() + 1;
            Some((current_offset, line))
        })
        .find_map(|(offset, line)| line.starts_with("      - ").then_some(offset));

    next_step.map_or_else(|| &job[start..], |end| &job[start..body_start + end])
}

const RELEASE_JOBS: [&str; 2] = ["auto-release", "manual-release"];

/// The crates.io publish step's credential: trusted publishing first, then the
/// token secrets (issue #62).
const CRATES_IO_CREDENTIALS: &str = "CARGO_REGISTRY_TOKEN: ${{ steps.crates-io-auth.outputs.token || secrets.CARGO_REGISTRY_TOKEN || secrets.CARGO_TOKEN }}";

/// The npm publish step's fallback credential (issue #62).
const NPM_CREDENTIALS: &str = "NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}";

/// The long-lived registry tokens are read only as fallbacks, and only by the
/// three publish steps: the two crates.io publish steps and the npm one.
#[test]
fn release_workflow_reads_registry_tokens_only_as_publish_step_fallbacks() {
    let workflow = release_workflow();

    assert_eq!(
        workflow.matches(CRATES_IO_CREDENTIALS).count(),
        2,
        "both crates.io publish steps fall back to the token secret"
    );
    assert_eq!(
        workflow.matches(NPM_CREDENTIALS).count(),
        1,
        "the npm publish step falls back to NPM_TOKEN"
    );

    for job_name in RELEASE_JOBS {
        let publish = step_block(job_block(&workflow, job_name), "Publish to Crates.io");
        assert!(
            publish.contains(CRATES_IO_CREDENTIALS),
            "{job_name}: {publish}"
        );
    }
    let npm = step_block(
        job_block(&workflow, "javascript-release"),
        "Publish and verify npm package",
    );
    assert!(npm.contains(NPM_CREDENTIALS), "{npm}");

    // Every other mention of a registry token is one of those declarations.
    let readers: Vec<&str> = workflow
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter(|line| {
            [
                "secrets.CARGO_TOKEN",
                "secrets.CARGO_REGISTRY_TOKEN",
                "secrets.NPM_TOKEN",
                "NODE_AUTH_TOKEN",
            ]
            .iter()
            .any(|token| line.contains(token))
        })
        .collect();
    assert_eq!(
        readers,
        [
            CRATES_IO_CREDENTIALS,
            CRATES_IO_CREDENTIALS,
            NPM_CREDENTIALS
        ],
        "release.yml reads the registry tokens only as publish-step fallbacks (issue #62)"
    );
}

/// Both crate-publishing jobs mint the crates.io token with the official,
/// hash-pinned auth action, which needs `id-token: write`.
#[test]
fn release_jobs_use_crates_io_trusted_publishing() {
    let workflow = release_workflow();

    for job_name in RELEASE_JOBS {
        let job = job_block(&workflow, job_name);
        assert!(
            job.contains("      id-token: write\n"),
            "{job_name} needs id-token: write for crates.io trusted publishing"
        );
        assert!(
            job.contains("      contents: write\n"),
            "{job_name} still pushes the release commit and tag"
        );

        let auth = step_block(job, "Authenticate to crates.io with trusted publishing");
        assert!(auth.contains("id: crates-io-auth"), "{job_name}: {auth}");
        let uses = auth
            .lines()
            .find_map(|line| line.trim().strip_prefix("uses: "))
            .unwrap_or_else(|| panic!("{job_name}: the auth step must use an action"));
        let (action, pin) = uses
            .split_once('@')
            .unwrap_or_else(|| panic!("{job_name}: {uses} must be pinned"));
        assert_eq!(action, "rust-lang/crates-io-auth-action");
        let sha = pin.split_whitespace().next().unwrap_or_default();
        assert!(
            sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "{job_name}: rust-lang/* is not on the zizmor ref-pin allowlist, so the auth \
             action must be pinned to a full commit SHA, got {pin}"
        );
        assert!(
            auth.contains("steps.crates-io.outputs.publish == 'true'"),
            "{job_name}: only mint a token when the crate can be published"
        );
        assert!(
            auth.contains("        continue-on-error: true\n"),
            "{job_name}: a failed OIDC exchange must fall through to the token secret"
        );

        let publish = step_block(job, "Publish to Crates.io");
        assert!(
            publish.contains(CRATES_IO_CREDENTIALS),
            "{job_name}: the publish step must receive the trusted-publishing token, \
             falling back to the token secret"
        );
        assert!(
            publish.contains("CRATES_IO_AUTH_OUTCOME: ${{ steps.crates-io-auth.outcome }}"),
            "{job_name}: the publish script reports which credential it used"
        );
        assert!(
            publish.contains("steps.crates-io.outputs.publish == 'true'"),
            "{job_name}: the publish step must skip when crates.io is not ready"
        );

        let readiness = job
            .find("- name: Check crates.io trusted publishing readiness")
            .unwrap_or_else(|| panic!("{job_name} should check crates.io readiness"));
        let auth_at = job
            .find("- name: Authenticate to crates.io with trusted publishing")
            .unwrap();
        let publish_at = job.find("- name: Publish to Crates.io").unwrap();
        assert!(
            readiness < auth_at && auth_at < publish_at,
            "{job_name}: readiness check, then token exchange, then publish"
        );
    }
}

/// A crate that is not on crates.io yet skips crates.io with the bootstrap
/// command -- and nothing else.
#[test]
fn a_crate_that_needs_bootstrap_skips_only_crates_io() {
    let workflow = release_workflow();

    for job_name in RELEASE_JOBS {
        let job = job_block(&workflow, job_name);

        let readiness = step_block(job, "Check crates.io trusted publishing readiness");
        for required in [
            "id: crates-io",
            "PREFLIGHT_CRATES: ${{ needs.release-preflight.outputs.crates }}",
            "echo \"publish=true\" >> \"$GITHUB_OUTPUT\"",
            "echo \"publish=false\" >> \"$GITHUB_OUTPUT\"",
            "::warning::",
            "package-registry-manager setup --registry crates-io --execute",
        ] {
            assert!(
                readiness.contains(required),
                "{job_name}: readiness step is missing {required:?}"
            );
        }

        // The GitHub release (and the Docker configuration feeding the Docker
        // jobs) still runs when crates.io was skipped...
        for step in ["Configure Docker Hub publishing", "Create GitHub Release"] {
            assert!(
                step_block(job, step).contains("steps.crates-io.outputs.publish == 'false'"),
                "{job_name}: {step} must not be blocked by a skipped crates.io publication"
            );
        }

        // ...but nothing verifies a crate that was not published.
        for step in [
            "Wait for Crate availability on Crates.io",
            "Smoke-test published crate",
        ] {
            let block = step_block(job, step);
            assert!(
                !block.contains("steps.crates-io.outputs.publish == 'false'"),
                "{job_name}: {step} must only run for a crate that is on crates.io"
            );
            assert!(
                block.contains("steps.publish-crate.outputs.publish_result == 'success'"),
                "{job_name}: {step} must be gated on the publish result"
            );
        }
    }
}

/// The Docker configuration step disables Docker when the preflight could
/// not prove the push credential, so the Docker jobs never start.
#[test]
fn docker_configuration_honours_the_docker_verdict() {
    let workflow = release_workflow();

    for job_name in RELEASE_JOBS {
        let job = job_block(&workflow, job_name);
        let docker = step_block(job, "Configure Docker Hub publishing");
        assert!(
            docker.contains("PREFLIGHT_DOCKER: ${{ needs.release-preflight.outputs.docker }}"),
            "{job_name}: {docker}"
        );
        assert!(
            docker.contains("if [ \"$PREFLIGHT_DOCKER\" != \"ok\" ]; then"),
            "{job_name}: {docker}"
        );
    }
}

/// npm publishes with OIDC first and NPM_TOKEN as the publish step's
/// fallback, and only when the package already exists.
#[test]
fn javascript_release_uses_npm_trusted_publishing_with_token_fallback() {
    let workflow = release_workflow();
    let job = job_block(&workflow, "javascript-release");

    assert!(job.contains("      id-token: write\n"));
    assert!(job.contains("needs.release-preflight.outputs.npm == 'ok'"));
    let publish = step_block(job, "Publish and verify npm package");
    assert!(publish.contains(NPM_CREDENTIALS), "{publish}");
    assert_eq!(
        job.matches("secrets.").count(),
        1,
        "NPM_TOKEN on the publish step is the job's only secret"
    );

    // The comment above the job and the preflight's bootstrap warning both
    // name the command that publishes the first version.
    let job_at = workflow.find("  javascript-release:\n").unwrap();
    let comment = workflow[..job_at].rsplit("\n\n").next().unwrap_or_default();
    let bootstrap = "package-registry-manager setup --registry npm --execute";
    assert!(
        comment.contains(bootstrap),
        "the javascript-release comment should name the npm bootstrap command"
    );
    assert!(
        read("scripts/preflight-credentials.sh").contains(bootstrap),
        "the npm bootstrap warning should name the bootstrap command"
    );
}

/// The publish script reads the token from Cargo's own variable and never
/// puts it on the command line; the workflow maps the fallback secrets into
/// that variable, so the script never reads the legacy name itself.
#[test]
fn publish_script_keeps_the_token_off_argv() {
    let script = read("scripts/publish-crate.rs");
    let code: String = script
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !code.contains("\"--token\""),
        "the token must stay off argv"
    );
    assert!(
        !code.contains("env::var(\"CARGO_TOKEN\")"),
        "the workflow maps CARGO_TOKEN into CARGO_REGISTRY_TOKEN"
    );
    assert!(code.contains("env::var(\"CARGO_REGISTRY_TOKEN\")"));
    assert!(code.contains("env::var(\"CRATES_IO_AUTH_OUTCOME\")"));
    assert!(code.contains("package-registry-manager setup --registry crates-io --execute"));
}
