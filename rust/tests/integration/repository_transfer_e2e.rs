//! Repository-transfer manifest proposals and release gating through the CLI.
use super::mock_registry::MockRegistry;
use super::npm_bootstrap_e2e::{prepare, read_log, setup_command};
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn published() -> MockRegistry {
    MockRegistry::start(|path| {
        path.starts_with("/npm/")
            .then(|| r#"{"version":"0.1.0","_npmUser":{"trustedPublisher":{}}}"#.into())
    })
}

fn commands(log: &[Value]) -> Vec<Vec<String>> {
    log.iter()
        .map(|entry| {
            entry["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|argument| argument.as_str().unwrap().to_owned())
                .collect()
        })
        .collect()
}

#[test]
fn transferred_manifest_creates_draft_pr_and_pauses_release_retry() {
    let temporary = TempDir::new().unwrap();
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let manifest = repository.join("package.json");
    let before = fs::read_to_string(&manifest).unwrap();
    let mut document: Value = serde_json::from_str(&before).unwrap();
    document["repository"] =
        serde_json::json!({"url":"git+https://github.com/old/pipeline-app.git","directory":"js"});
    let old_contents = serde_json::to_string_pretty(&document).unwrap();
    fs::write(&manifest, &old_contents).unwrap();
    let output = setup_command(
        &repository,
        &state,
        &published(),
        &["--no-browser"],
        &[
            ("FAKE_TRANSFER", "1"),
            ("FAKE_TRANSFER_METADATA", "stale"),
            ("FAKE_RELEASE_RUN", "failed-publish"),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = commands(&read_log(&state));
    let proposal = log
        .iter()
        .find(|args| args.starts_with(&["gh".into(), "pr".into(), "create".into()]))
        .unwrap();
    assert!(proposal.contains(&"--draft".into()));
    assert!(log
        .iter()
        .any(|args| args.starts_with(&["git".into(), "push".into()])));
    assert!(log.iter().any(|args| args.starts_with(&[
        "git".into(),
        "worktree".into(),
        "remove".into()
    ])));
    assert!(!log.iter().any(|args| args.contains(&"rerun".into())
        || args.starts_with(&["gh".into(), "workflow".into(), "run".into()])));
    assert_eq!(fs::read_to_string(manifest).unwrap(), old_contents);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Release retries are paused"));
}

#[test]
fn release_retry_requires_correct_metadata_at_the_selected_remote_sha() {
    for (metadata, success, action) in [
        ("stale", false, ""),
        ("fixed-main", true, "workflow"),
        ("correct", true, "rerun"),
    ] {
        let temporary = TempDir::new().unwrap();
        let Some((repository, state)) = prepare(&temporary) else {
            return;
        };
        let output = setup_command(
            &repository,
            &state,
            &published(),
            &["--no-browser"],
            &[
                ("FAKE_TRANSFER", "1"),
                ("FAKE_TRANSFER_METADATA", metadata),
                ("FAKE_RELEASE_RUN", "failed-publish"),
            ],
        );
        assert_eq!(
            output.status.success(),
            success,
            "{metadata}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let log = commands(&read_log(&state));
        let retry = log.iter().find(|args| {
            args.contains(&"rerun".into())
                || args.starts_with(&["gh".into(), "workflow".into(), "run".into()])
        });
        assert_eq!(retry.is_some(), success);
        if let Some(retry) = retry {
            assert!(retry.contains(&action.into()), "{retry:?}");
        }
        if !success {
            assert!(String::from_utf8_lossy(&output.stderr).contains("merge the manifest"));
        }
    }
}

#[test]
fn release_retry_skips_newer_pr_and_lint_runs() {
    for repair in [false, true] {
        let temporary = TempDir::new().unwrap();
        let Some((repository, state)) = prepare(&temporary) else {
            return;
        };
        fs::write(
            state.join("release-runs.json"),
            include_str!("../../../tests/fixtures/repository-transfer/release-runs.json"),
        )
        .unwrap();
        let logfile = state.join("log.jsonl");
        let registry = MockRegistry::start(move |path| {
            path.starts_with("/npm/").then(|| {
                if fs::read_to_string(&logfile).unwrap_or_default().contains(r#""run","rerun""#) {
                    r#"{"version":"0.1.1","_npmUser":{"trustedPublisher":{}},"dist":{"attestations":{}}}"#.into()
                } else { r#"{"version":"0.1.0"}"#.into() }
            })
        });
        let mut env = vec![("FAKE_RELEASE_RUN", "mixed")];
        if repair {
            env.extend([
                ("FAKE_TRANSFER", "1"),
                ("FAKE_TRANSFER_METADATA", "correct"),
            ]);
        }
        let output = setup_command(
            &repository,
            &state,
            &registry,
            &["--no-browser", "--verify-release"],
            &env,
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let log = commands(&read_log(&state));
        assert_eq!(
            log.iter()
                .filter(|args| args.starts_with(&["gh".into(), "run".into(), "list".into()]))
                .count(),
            2
        );
        assert_eq!(
            log.iter()
                .filter(|args| args.contains(&"--log-failed".into()))
                .map(|args| args[3].as_str())
                .collect::<Vec<_>>(),
            ["42"]
        );
        let retries: Vec<_> = log
            .iter()
            .filter(|args| args.starts_with(&["gh".into(), "run".into(), "rerun".into()]))
            .map(|args| args[3].as_str())
            .collect();
        assert_eq!(retries, ["42"], "{stdout}");
        assert!(log.iter().any(|args| args.starts_with(&[
            "gh".into(),
            "run".into(),
            "watch".into(),
            "42".into()
        ])));
        assert!(!log.iter().any(|args| args.starts_with(&[
            "gh".into(),
            "workflow".into(),
            "run".into()
        ])));
        let lines: Vec<_> = stdout.lines().collect();
        let selected = lines
            .iter()
            .position(|line| {
                *line == "  The selected failed release run failure: https://github.com/acme/pipeline-app/actions/runs/42"
            })
            .expect("the selected release URL must be printed");
        let retry_message = if repair {
            "  Selected failed release run: https://github.com/acme/pipeline-app/actions/runs/42"
        } else {
            "  Re-running the failed release jobs."
        };
        let retry = lines
            .iter()
            .position(|line| *line == retry_message)
            .expect("the release retry must be announced");
        assert!(selected < retry, "{stdout}");
        assert!(stdout.contains("OIDC"));
        assert!(!stdout.contains("trusted publishing alone will not fix it"));
        for args in log
            .iter()
            .filter(|args| args.starts_with(&["gh".into(), "run".into(), "list".into()]))
        {
            assert_eq!(
                args[args.iter().position(|arg| arg == "--branch").unwrap() + 1],
                "main"
            );
            assert!(matches!(
                args[args.iter().position(|arg| arg == "--event").unwrap() + 1].as_str(),
                "push" | "workflow_dispatch"
            ));
        }
    }
}
