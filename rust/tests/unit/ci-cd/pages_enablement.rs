//! Regression tests for package-registry-manager issue #16, part 4.
//!
//! `deploy-docs` failed every push to main with "Get Pages site failed" on
//! repositories where GitHub Pages had never been enabled. The job now asks
//! the Pages API first; when Pages is disabled it warns with the command that
//! enables it and skips the build and deployment instead of failing.

use std::fs;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::{Command, Output};
#[cfg(unix)]
use std::time::{SystemTime, UNIX_EPOCH};

const CHECK_STEP: &str = "Check whether GitHub Pages is enabled";
const ENABLED: &str = "if: steps.pages.outputs.enabled == 'true'";

fn release_workflow() -> String {
    fs::read_to_string(format!(
        "{}/../.github/workflows/release.yml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("release.yml should exist")
    .replace("\r\n", "\n")
}

fn deploy_docs(workflow: &str) -> &str {
    let marker = "  deploy-docs:\n";
    let start = workflow.find(marker).expect("deploy-docs job should exist");
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
            (starts_at_job_indent && !line.trim_start().starts_with('#')).then_some(offset)
        });

    next_job.map_or_else(
        || &workflow[start..],
        |end| &workflow[start..body_start + end],
    )
}

/// The steps of a job, split at each `      - ` list item.
fn steps(job: &str) -> Vec<&str> {
    let start = job.find("    steps:\n").expect("job should have steps");
    job[start..]
        .split("\n      - ")
        .skip(1)
        .map(str::trim_end)
        .collect()
}

/// The body of a step's `run: |` block, de-indented.
fn run_script(step: &str) -> String {
    let start = step
        .find("        run: |\n")
        .expect("step should run a script")
        + 15;
    step[start..]
        .lines()
        .take_while(|line| line.is_empty() || line.starts_with("          "))
        .map(|line| line.strip_prefix("          ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn check_step(workflow: &str) -> String {
    steps(deploy_docs(workflow))
        .into_iter()
        .find(|step| step.starts_with(&format!("name: {CHECK_STEP}")))
        .expect("deploy-docs should check whether Pages is enabled")
        .to_owned()
}

#[test]
fn deploy_docs_checks_pages_before_anything_else() {
    let workflow = release_workflow();
    let job = deploy_docs(&workflow);
    let steps = steps(job);

    assert!(
        steps[0].starts_with(&format!("name: {CHECK_STEP}")),
        "the Pages check must be the first step: {}",
        steps[0]
    );
    for step in &steps[1..] {
        assert!(
            step.contains(ENABLED),
            "every step after the Pages check must be skipped when Pages is disabled:\n{step}"
        );
    }
}

#[test]
fn pages_check_warns_with_the_enable_command_and_keeps_permissions_minimal() {
    let workflow = release_workflow();
    let job = deploy_docs(&workflow);
    let step = check_step(&workflow);

    for required in [
        "id: pages",
        "GH_TOKEN: ${{ github.token }}",
        "GH_API_URL: ${{ github.api_url }}",
        "REPOSITORY: ${{ github.repository }}",
        "$GH_API_URL/repos/$REPOSITORY/pages",
        "gh api -X POST repos/$REPOSITORY/pages -f build_type=workflow",
        "::warning::",
        "enabled=false",
        "enabled=true",
    ] {
        assert!(
            step.contains(required),
            "Pages check is missing {required:?}"
        );
    }
    assert!(
        !job.contains("enablement: true"),
        "configure-pages' enablement needs an administration token the workflow token lacks"
    );
    for permission in ["contents: read", "pages: write", "id-token: write"] {
        assert!(job.contains(permission), "deploy-docs needs {permission}");
    }
    for forbidden in ["contents: write", "administration:"] {
        assert!(
            !job.contains(forbidden),
            "deploy-docs must not hold {forbidden}"
        );
    }
}

#[cfg(unix)]
fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("pages-enablement-{name}-{nanos}"));
    fs::create_dir_all(&path).unwrap();
    path
}

/// Run the step's script against a curl stub that answers the Pages API with
/// `status` and `body`. Returns the process output and `$GITHUB_OUTPUT`.
#[cfg(unix)]
fn run_check(status: &str, body: &str) -> (Output, String) {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir(status);
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let stub = r#"#!/usr/bin/env bash
out=''
while [ $# -gt 0 ]; do
  case "$1" in
    --output) out="$2"; shift ;;
  esac
  shift
done
[ -n "$out" ] && printf '%s' "$FAKE_BODY" > "$out"
printf '%s' "$FAKE_STATUS"
"#;
    fs::write(bin.join("curl"), stub).unwrap();
    fs::set_permissions(bin.join("curl"), fs::Permissions::from_mode(0o755)).unwrap();

    let script = dir.join("check.sh");
    fs::write(&script, run_script(&check_step(&release_workflow()))).unwrap();
    let outputs = dir.join("github-output");

    let output = Command::new("bash")
        .arg("-e")
        .arg(&script)
        .env("GH_TOKEN", "fake-token")
        .env("GH_API_URL", "https://api.github.test")
        .env("REPOSITORY", "acme/widget")
        .env("RUNNER_TEMP", &dir)
        .env("GITHUB_OUTPUT", &outputs)
        .env("FAKE_STATUS", status)
        .env("FAKE_BODY", body)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .expect("the Pages check should run");
    let outputs = fs::read_to_string(&outputs).unwrap_or_default();
    (output, outputs)
}

#[cfg(unix)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(unix)]
#[test]
fn disabled_pages_skip_with_a_warning_instead_of_failing() {
    let (output, outputs) = run_check("404", r#"{"message":"Not Found"}"#);

    assert!(output.status.success(), "{}", stdout(&output));
    assert_eq!(outputs.trim(), "enabled=false");
    let out = stdout(&output);
    assert!(out.starts_with("::warning::"), "{out}");
    assert!(
        out.contains("gh api -X POST repos/acme/widget/pages -f build_type=workflow"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn pages_built_by_github_actions_deploy() {
    let body = "{\n  \"url\": \"https://api.github.test/repos/acme/widget/pages\",\n  \"build_type\": \"workflow\",\n  \"source\": {\n    \"branch\": \"main\"\n  }\n}";
    let (output, outputs) = run_check("200", body);

    assert!(output.status.success(), "{}", stdout(&output));
    assert_eq!(outputs.trim(), "enabled=true");
    assert!(!stdout(&output).contains("::warning::"));
}

#[cfg(unix)]
#[test]
fn pages_built_from_a_branch_skip_with_a_warning() {
    let (output, outputs) = run_check("200", r#"{"build_type":"legacy","source":{}}"#);

    assert!(output.status.success(), "{}", stdout(&output));
    assert_eq!(outputs.trim(), "enabled=false");
    let out = stdout(&output);
    assert!(out.contains("::warning::"), "{out}");
    assert!(out.contains("'legacy'"), "{out}");
}

#[cfg(unix)]
#[test]
fn an_unexpected_api_answer_fails_loudly() {
    let (output, outputs) = run_check("500", "{}");

    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(stdout(&output).contains("::error::"));
    assert!(!outputs.contains("enabled="));
}
