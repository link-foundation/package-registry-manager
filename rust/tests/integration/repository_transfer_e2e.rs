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
