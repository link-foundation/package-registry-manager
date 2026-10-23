//! npm session reuse and the pre-sign-in tarball checks through the CLI.
use std::fs;

use tempfile::TempDir;

use super::npm_bootstrap_e2e::{
    failing_setup, missing_registry, prepare, published_registry, read_log, run_setup,
    run_setup_with, setup_command,
};
use crate::mock_registry::MockRegistry;

#[test]
fn setup_all_reuses_the_npm_session_and_signs_out_after_the_last_package_with_sign_out() {
    let temporary = TempDir::new().unwrap();
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let second = repository.join("second");
    fs::create_dir_all(&second).unwrap();
    fs::write(
        second.join("package.json"),
        r#"{"name":"second-package","version":"0.1.0"}"#,
    )
    .unwrap();
    let workflow = repository.join(".github/workflows/release.yml");
    let mut contents = fs::read_to_string(&workflow).unwrap();
    contents = contents.replace(
        "  docker-hub:",
        "      - run: npm publish\n        working-directory: second\n  docker-hub:",
    );
    fs::write(workflow, contents).unwrap();
    let registry = MockRegistry::start(|path| {
        path.starts_with("/npm/")
            .then(|| r#"{"version":"0.1.0"}"#.to_owned())
    });
    let run = run_setup_with(
        &repository,
        &state,
        &registry,
        &["--no-browser", "--all", "--sign-out"],
    );
    let npm: Vec<_> = run
        .log
        .iter()
        .filter(|entry| entry["argv"][0] == "npm")
        .filter_map(|entry| entry["argv"][1].as_str())
        .collect();
    assert_eq!(npm.iter().filter(|command| **command == "login").count(), 1);
    assert_eq!(
        npm.iter().filter(|command| **command == "logout").count(),
        1
    );
    assert_eq!(npm.last(), Some(&"logout"));
    assert!(run.stdout.contains("second-package: configured"));
    assert!(!state.join("session").exists());
}

#[test]
fn keeps_the_npm_session_unless_sign_out_is_given() {
    let temporary = TempDir::new().expect("create temporary directory");
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let registry = published_registry();
    let run = run_setup(&repository, &state, &registry);
    assert!(!run.commands().iter().any(|command| command == "npm logout"));
    assert!(
        run.stdout
            .lines()
            .any(|line| line.starts_with("Keeping the npm session")),
        "{}",
        run.stdout
    );
    assert!(state.join("session").exists(), "still signed in");

    // The hidden --keep-session still parses, for scripts that pass it.
    let kept = run_setup_with(
        &repository,
        &state,
        &registry,
        &["--no-browser", "--keep-session"],
    );
    assert!(!kept
        .commands()
        .iter()
        .any(|command| command == "npm logout"));

    // --sign-out ends the session the run signed in, here after it expired.
    fs::remove_file(state.join("session")).expect("expire the session");
    let signed_out = run_setup_with(
        &repository,
        &state,
        &registry,
        &["--no-browser", "--sign-out"],
    );
    assert!(signed_out
        .commands()
        .iter()
        .any(|command| command == "npm logout"));
    assert!(!state.join("session").exists(), "signed out");
}

#[test]
fn rejects_sign_out_together_with_keep_session() {
    let temporary = TempDir::new().expect("create temporary directory");
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let output = setup_command(
        &repository,
        &state,
        &missing_registry(),
        &["--no-browser", "--sign-out", "--keep-session"],
        &[],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--sign-out"), "{stderr}");
    assert!(read_log(&state).is_empty(), "nothing runs");
}

#[test]
fn stops_before_signing_in_when_the_tarball_has_development_files() {
    let temporary = TempDir::new().expect("create temporary directory");
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let (_, stderr, commands) = failing_setup(
        &repository,
        &state,
        &missing_registry(),
        &[(
            "FAKE_PACK_FILES",
            "src/index.js,.github/workflows/ci.yml,tests/a.test.js",
        )],
    );
    assert!(
        stderr.contains("would publish repository development files (.github/, tests/)"),
        "{stderr}"
    );
    assert!(!commands
        .iter()
        .any(|command| command.starts_with("npm login") || command.starts_with("npm publish")));
}
