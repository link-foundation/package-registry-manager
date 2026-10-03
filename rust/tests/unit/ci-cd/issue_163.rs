//! Regression tests for issues #163 and #167, reworked for
//! package-registry-manager issue #16.
//!
//! The release credentials were first exercised by the very step that
//! publishes -- 50 capped minutes into the run -- so a revoked token cost the
//! whole matrix before anyone learned the release could not happen. The
//! `release-preflight` job probes every registry before the expensive jobs
//! spend their minutes.
//!
//! Issue #16 made the verdict per registry. crates.io and npm publish with
//! trusted publishing (OIDC), which cannot create a package, so their probe
//! is whether the package exists (`bootstrap` otherwise). Docker Hub still
//! uses a token and is probed with an attempted blob-upload write (a login
//! proves authentication, not authorisation). Each verdict is a job output
//! (`crates`, `npm`, `docker`) and each publishing job gates on its own one:
//! a refused or missing registry skips only its own publication.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Output};
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_path(relative: &str) -> PathBuf {
    let prefix = if relative.starts_with(".github/") {
        "../"
    } else {
        ""
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(prefix)
        .join(relative)
}

fn release_yml() -> String {
    fs::read_to_string(repo_path(".github/workflows/release.yml"))
        .expect("release.yml should exist")
        .replace("\r\n", "\n")
}

fn job_block<'a>(workflow: &'a str, job: &str) -> &'a str {
    let marker = format!("  {job}:\n");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("release.yml should declare the {job} job"));
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

#[cfg(unix)]
fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("issue-163-{name}-{nanos}"));
    fs::create_dir_all(&path).unwrap();
    path
}

/// The fake npm registry the tests point `NPM_REGISTRY` at.
#[cfg(unix)]
const NPM_REGISTRY: &str = "https://registry.npm.test";

/// An offline curl: answers each URL the preflight script probes with a
/// configurable verdict, so the behaviour tests never touch a network. The
/// response table is the environment (`FAKE_*` variables); the defaults are a
/// fully working registry set. `fail` as a status simulates a network error.
/// Every request is appended to `$FAKE_LOG` as `METHOD URL ua=<user agent>`.
#[cfg(unix)]
fn write_curl_stub(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let stub = r#"#!/usr/bin/env bash
method=GET; mode=body; url=''; ua=''
args=("$@"); i=0
while [ $i -lt $# ]; do
  a="${args[$i]}"
  case "$a" in
    -X) method="${args[$((i+1))]}"; i=$((i+1)) ;;
    -A) ua="${args[$((i+1))]}"; i=$((i+1)) ;;
    -D) mode=headers ;;
    http://*|https://*) url="$a" ;;
  esac
  i=$((i+1))
done
[ -n "${FAKE_LOG:-}" ] && printf '%s %s ua=%s\n' "$method" "$url" "$ua" >> "$FAKE_LOG"
answer() {
  if [ "$1" = fail ]; then exit 7; fi
  printf '{}\n%s\n' "$1"
}
case "$url" in
  */api/v1/crates/*)
    answer "${FAKE_CRATE_STATUS:-200}" ;;
  https://registry.npm.test/*)
    answer "${FAKE_NPM_STATUS:-200}" ;;
  */token?*)
    printf '{"token":"fake-jwt","access":"pull"}\n200\n' ;;
  */blobs/uploads/*)
    if [ "$method" = DELETE ]; then exit 0; fi
    if [ "$mode" = headers ]; then
      printf 'HTTP/1.1 %s\r\nLocation: https://registry-1.docker.io/v2/%s/blobs/uploads/session-1\r\n\r\n' \
        "${FAKE_UPLOAD_STATUS:-202}" "${FAKE_IMAGE:-user/img}"
    else
      printf '{}\n%s\n' "${FAKE_UPLOAD_STATUS:-202}"
    fi ;;
  *)
    printf 'unexpected url: %s\n' "$url" >&2
    exit 1 ;;
esac
"#;
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("curl"), stub).unwrap();
    fs::set_permissions(bin.join("curl"), fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
struct PreflightRun {
    status: Output,
    outputs: String,
    summary: String,
    log: String,
}

#[cfg(unix)]
impl PreflightRun {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.status.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.status.stderr).into_owned()
    }

    /// The value the script wrote to `$GITHUB_OUTPUT` for `key`.
    fn output(&self, key: &str) -> Option<String> {
        self.outputs.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_owned)
        })
    }

    fn verdicts(&self) -> (String, String, String) {
        let get = |key| {
            self.output(key)
                .unwrap_or_else(|| panic!("no {key}= output: {}", self.stdout()))
        };
        (get("crates"), get("npm"), get("docker"))
    }
}

#[cfg(unix)]
fn run_preflight_with_package(
    workdir: &Path,
    env: &[(&str, &str)],
    mode: &str,
    package_json: &str,
) -> PreflightRun {
    let fixture = temp_dir("run");
    write_curl_stub(&fixture);
    // The crates.io probe reads the crate name from the manifest in the
    // working directory; give every fixture the same one.
    fs::write(
        workdir.join("Cargo.toml"),
        "[package]\nname = \"test-crate\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let package_json_path = fixture.join("package.json");
    fs::write(&package_json_path, package_json).unwrap();
    let outputs = fixture.join("github-output");
    let summary = fixture.join("step-summary");
    let log = fixture.join("curl-log");

    let script = repo_path("scripts/preflight-credentials.sh");
    let mut command = Command::new("bash");
    command
        .arg(script)
        .current_dir(workdir)
        .env("PREFLIGHT_MODE", mode)
        .env("NPM_REGISTRY", NPM_REGISTRY)
        .env("NPM_PACKAGE_JSON", &package_json_path)
        .env("GITHUB_OUTPUT", &outputs)
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env("FAKE_LOG", &log)
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixture.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        );
    for (key, value) in env {
        command.env(key, value);
    }

    let status = command.output().expect("preflight script should run");
    let read = |path: &Path| fs::read_to_string(path).unwrap_or_default();
    PreflightRun {
        status,
        outputs: read(&outputs),
        summary: read(&summary),
        log: read(&log),
    }
}

#[cfg(unix)]
fn run_preflight(workdir: &Path, env: &[(&str, &str)], mode: &str) -> PreflightRun {
    run_preflight_with_package(
        workdir,
        env,
        mode,
        "{\n  \"name\": \"test-package\",\n  \"version\": \"0.1.0\"\n}\n",
    )
}

#[cfg(unix)]
fn release_env() -> Vec<(&'static str, &'static str)> {
    vec![
        ("DOCKERHUB_IMAGE", "user/img"),
        ("DOCKERHUB_USERNAME", "user"),
        ("DOCKERHUB_TOKEN", "docker-secret-value"),
    ]
}

#[cfg(unix)]
fn verdicts(crates: &str, npm: &str, docker: &str) -> (String, String, String) {
    (crates.to_owned(), npm.to_owned(), docker.to_owned())
}

/// The happy path: both packages exist and Docker Hub opens a write session.
#[cfg(unix)]
#[test]
fn every_registry_ready_is_ok_in_release_mode() {
    let workdir = temp_dir("happy");
    let run = run_preflight(&workdir, &release_env(), "release");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(run.verdicts(), verdicts("ok", "ok", "ok"));
    let out = run.stdout();
    assert!(out.contains("test-crate exists on crates.io"), "{out}");
    assert!(out.contains("test-package exists on npm"), "{out}");
    assert!(out.contains("blob-upload write"), "{out}");
    assert!(!out.contains("::error::"), "{out}");
    assert!(!out.contains("::warning::"), "{out}");
}

/// Trusted publishing cannot create a crate: a crate missing from crates.io
/// is `bootstrap`, names the bootstrap command, and leaves npm and Docker
/// publishing untouched.
#[cfg(unix)]
#[test]
fn a_crate_missing_from_crates_io_needs_bootstrap_and_blocks_nothing_else() {
    let workdir = temp_dir("crate-404");
    let mut env = release_env();
    env.push(("FAKE_CRATE_STATUS", "404"));
    let run = run_preflight(&workdir, &env, "release");

    assert!(
        run.status.status.success(),
        "a per-registry problem never fails the preflight job: {}",
        run.stdout()
    );
    assert_eq!(run.verdicts(), verdicts("bootstrap", "ok", "ok"));
    let out = run.stdout();
    assert!(
        out.contains("::warning::release-preflight: test-crate is not on crates.io yet"),
        "{out}"
    );
    assert!(
        out.contains("package-registry-manager setup --registry crates-io --execute"),
        "the annotation must name the bootstrap command: {out}"
    );
}

/// Same for npm: the package must exist before OIDC can publish to it.
#[cfg(unix)]
#[test]
fn a_package_missing_from_npm_needs_bootstrap_and_blocks_nothing_else() {
    let workdir = temp_dir("npm-404");
    let mut env = release_env();
    env.push(("FAKE_NPM_STATUS", "404"));
    let run = run_preflight(&workdir, &env, "release");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(run.verdicts(), verdicts("ok", "bootstrap", "ok"));
    assert!(
        run.stdout()
            .contains("package-registry-manager setup --registry npm --execute"),
        "{}",
        run.stdout()
    );
}

/// The measured Docker Hub behaviour: the token endpoint hands out a token
/// for a pull,push scope request, then the registry refuses the write. The
/// probe must catch what the login the publishing jobs run cannot -- and the
/// refusal is an error for Docker alone.
#[cfg(unix)]
#[test]
fn a_refused_docker_write_is_refused_for_docker_only() {
    let workdir = temp_dir("docker-403");
    let mut env = release_env();
    env.push(("FAKE_UPLOAD_STATUS", "403"));
    let run = run_preflight(&workdir, &env, "release");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(run.verdicts(), verdicts("ok", "ok", "refused"));
    let out = run.stdout();
    assert!(
        out.contains("::error::release-preflight: Docker Hub refused the write"),
        "the write refusal must be named as an error in release mode: {out}"
    );
}

/// A Docker image without credentials would fail at login: refused.
#[cfg(unix)]
#[test]
fn a_docker_image_without_credentials_is_refused() {
    let workdir = temp_dir("docker-no-token");
    let env = vec![("DOCKERHUB_IMAGE", "user/img")];
    let run = run_preflight(&workdir, &env, "release");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(run.verdicts(), verdicts("ok", "ok", "refused"));
}

/// Every problem is reported, not the first: all three registries are named
/// in one pass, each with its own verdict.
#[cfg(unix)]
#[test]
fn every_problem_is_reported_not_just_the_first() {
    let workdir = temp_dir("all-broken");
    let mut env = release_env();
    env.push(("FAKE_CRATE_STATUS", "404"));
    env.push(("FAKE_NPM_STATUS", "404"));
    env.push(("FAKE_UPLOAD_STATUS", "403"));
    let run = run_preflight(&workdir, &env, "release");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(
        run.verdicts(),
        verdicts("bootstrap", "bootstrap", "refused")
    );
    let out = run.stdout();
    assert!(out.contains("not on crates.io yet"), "{out}");
    assert!(out.contains("not on npm yet"), "{out}");
    assert!(out.contains("Docker Hub refused the write"), "{out}");
}

/// A 429, a 5xx or a network failure has not said anything about the
/// registry -- `unknown`, never a guess, and never `ok`.
#[cfg(unix)]
#[test]
fn a_probe_without_an_answer_is_unknown_never_ok() {
    let workdir = temp_dir("unknown");
    let mut env = release_env();
    env.push(("FAKE_CRATE_STATUS", "429"));
    env.push(("FAKE_NPM_STATUS", "fail"));
    env.push(("FAKE_UPLOAD_STATUS", "503"));
    let run = run_preflight(&workdir, &env, "release");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(run.verdicts(), verdicts("unknown", "unknown", "unknown"));
    assert!(
        !run.stdout().contains("::error::"),
        "no verdict is a warning, not an error: {}",
        run.stdout()
    );
}

/// Pull requests may come from forks without publishing secrets: the same
/// probes run and annotate, but only ever as warnings.
#[cfg(unix)]
#[test]
fn report_mode_reports_but_never_errors() {
    let workdir = temp_dir("report-mode");
    let mut env = release_env();
    env.push(("FAKE_CRATE_STATUS", "404"));
    env.push(("FAKE_UPLOAD_STATUS", "403"));
    let run = run_preflight(&workdir, &env, "report");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(run.verdicts(), verdicts("bootstrap", "ok", "refused"));
    let out = run.stdout();
    assert!(out.contains("advisory"), "{out}");
    assert!(!out.contains("::error::"), "{out}");
    assert!(out.contains("::warning::"), "{out}");
}

/// Docker publishing is optional (`DOCKERHUB_IMAGE` unset disables it
/// everywhere else too) -- a skip is not a problem.
#[cfg(unix)]
#[test]
fn a_disabled_docker_path_is_a_skip_not_a_problem() {
    let workdir = temp_dir("docker-off");
    let run = run_preflight(&workdir, &[], "release");

    assert!(run.status.status.success(), "{}", run.stdout());
    assert_eq!(run.verdicts(), verdicts("ok", "ok", "skipped"));
    assert!(run.stdout().contains("SKIP"), "{}", run.stdout());
    assert!(!run.stdout().contains("::warning::"), "{}", run.stdout());
}

/// crates.io answers 403 to API calls without a User-Agent, and a scoped npm
/// name must be addressed as `@scope%2Fname`. The top-level package name is
/// read, never a nested `name` key.
#[cfg(unix)]
#[test]
fn the_probes_address_the_registries_correctly() {
    let workdir = temp_dir("requests");
    let package_json = "{\n  \"repository\": {\n    \"name\": \"nested\"\n  },\n  \"name\": \"@scope/test-package\",\n  \"version\": \"0.1.0\"\n}\n";
    let run = run_preflight_with_package(&workdir, &[], "release", package_json);

    assert_eq!(run.verdicts(), verdicts("ok", "ok", "skipped"));
    assert!(
        run.log
            .contains("GET https://crates.io/api/v1/crates/test-crate ua=release-preflight"),
        "{}",
        run.log
    );
    assert!(
        run.log
            .contains("GET https://registry.npm.test/@scope%2Ftest-package ua="),
        "{}",
        run.log
    );
}

/// The step summary carries one row per registry.
#[cfg(unix)]
#[test]
fn the_step_summary_tabulates_every_registry() {
    let workdir = temp_dir("summary");
    let mut env = release_env();
    env.push(("FAKE_CRATE_STATUS", "404"));
    let run = run_preflight(&workdir, &env, "release");

    for row in [
        "| crates.io | bootstrap | no |",
        "| npm | ok | yes |",
        "| Docker Hub | ok | yes |",
    ] {
        assert!(
            run.summary.contains(row),
            "missing {row:?}: {}",
            run.summary
        );
    }
}

/// Rule 4: never print a credential, in any channel the script writes to.
#[cfg(unix)]
#[test]
fn the_docker_token_is_never_printed() {
    let workdir = temp_dir("no-leak");
    let mut env = release_env();
    env.push(("FAKE_UPLOAD_STATUS", "403"));
    let run = run_preflight(&workdir, &env, "release");

    for (channel, text) in [
        ("stdout", run.stdout()),
        ("stderr", run.stderr()),
        ("GITHUB_OUTPUT", run.outputs.clone()),
        ("GITHUB_STEP_SUMMARY", run.summary),
    ] {
        assert!(
            !text.contains("docker-secret-value"),
            "{channel} leaked the Docker Hub token"
        );
    }
}

/// The job exists, computes its mode from the event -- release for
/// push-to-main and the manual instant release, report for everything else --
/// and publishes one verdict per registry as a job output.
#[test]
fn release_yml_declares_the_preflight_job_with_per_registry_outputs() {
    let workflow = release_yml();
    let job = job_block(&workflow, "release-preflight");

    for required in [
        "timeout-minutes: 5",
        "persist-credentials: false",
        "id: probe",
        "bash rust/scripts/preflight-credentials.sh",
        "github.event_name == 'push' && github.ref == 'refs/heads/main'",
        "github.event_name == 'workflow_dispatch' && github.event.inputs.release_mode == 'instant'",
        "'release' || 'report'",
        "crates: ${{ steps.probe.outputs.crates }}",
        "npm: ${{ steps.probe.outputs.npm }}",
        "docker: ${{ steps.probe.outputs.docker }}",
    ] {
        assert!(
            job.contains(required),
            "release-preflight is missing {required:?}"
        );
    }
    assert!(
        !job.contains("id-token: write"),
        "release-preflight runs branch code on pull requests and must not mint OIDC tokens"
    );
    assert!(
        !job.contains("CARGO_"),
        "release-preflight no longer has a crates.io token to probe"
    );
}

/// Every job that publishes declares the preflight, refuses to run on
/// anything but a successful preflight job -- skipped is not good enough
/// (box#117: a green run that published nothing) -- and gates on its own
/// registry's verdict.
#[test]
fn every_publishing_job_gates_on_its_own_preflight_verdict() {
    let workflow = release_yml();

    for (job, verdict_gate) in [
        (
            "auto-release",
            "PREFLIGHT_CRATES: ${{ needs.release-preflight.outputs.crates }}",
        ),
        (
            "manual-release",
            "PREFLIGHT_CRATES: ${{ needs.release-preflight.outputs.crates }}",
        ),
        (
            "javascript-release",
            "needs.release-preflight.outputs.npm == 'ok'",
        ),
        (
            "docker-publish",
            "needs.release-preflight.outputs.docker == 'ok'",
        ),
        (
            "docker-merge-manifest",
            "needs.release-preflight.outputs.docker == 'ok'",
        ),
    ] {
        let block = job_block(&workflow, job);
        assert!(
            block.contains("release-preflight"),
            "{job} must declare release-preflight in needs"
        );
        assert!(
            block.contains("needs.release-preflight.result == 'success'"),
            "{job} must gate on the preflight job succeeding"
        );
        assert!(
            block.contains(verdict_gate),
            "{job} must gate on its own registry verdict: {verdict_gate}"
        );
    }
}

/// The terminal status gate observes the preflight too: a cancelled preflight
/// on main is exactly the hidden failure the gate exists to surface.
#[test]
fn the_status_gate_observes_the_preflight() {
    let workflow = release_yml();
    let gate = job_block(&workflow, "pipeline-status");

    assert!(
        gate.contains("release-preflight"),
        "pipeline-status must observe release-preflight"
    );
}
