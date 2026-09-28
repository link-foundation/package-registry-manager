//! End-to-end npm bootstrap through the binary, with Node.js stand-ins for
//! npm, npx, git, gh, and the default-browser openers on PATH and a mock
//! registry.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

use crate::mock_registry::MockRegistry;

fn find_node() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join("node"))
        .find(|candidate| candidate.is_file())
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture directory");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

fn install_fake_tools(node: &Path, state: &Path) -> PathBuf {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures");
    let script = fs::read_to_string(fixtures.join("fake-tools/fake-tool.cjs")).expect("fake tool");
    let bin = state.join("bin");
    fs::create_dir_all(&bin).expect("create fake bin");
    for tool in ["npm", "npx", "git", "gh", "open", "xdg-open"] {
        let file = bin.join(tool);
        fs::write(&file, format!("#!{}\n{script}", node.display())).expect("write fake tool");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod fake tool");
    }
    bin
}

struct Run {
    stdout: String,
    log: Vec<Value>,
}

impl Run {
    fn commands(&self) -> Vec<String> {
        self.log
            .iter()
            .map(|entry| {
                entry["argv"]
                    .as_array()
                    .expect("argv")
                    .iter()
                    .map(|arg| arg.as_str().expect("argument"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }
}

fn read_log(state: &Path) -> Vec<Value> {
    fs::read_to_string(state.join("log.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse log entry"))
        .collect()
}

fn setup_command(
    repository: &Path,
    state: &Path,
    registry: &MockRegistry,
    browser: &[&str],
    env: &[(&str, &str)],
) -> Output {
    let path = std::env::join_paths(std::iter::once(state.join("bin")).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("join PATH");
    Command::new(env!("CARGO_BIN_EXE_package-registry-manager"))
        .args([
            "--repository",
            repository.to_str().expect("UTF-8 path"),
            "setup",
            "--registry",
            "npm",
            "--package",
            "pipeline-app",
            "--execute",
            "--yes",
        ])
        .args(browser)
        .envs(registry.env())
        .env("PATH", path)
        .env("FAKE_STATE", state)
        .envs(env.iter().copied())
        .env_remove("NODE_OPTIONS")
        .output()
        .expect("run package-registry-manager")
}

fn run_setup(repository: &Path, state: &Path, registry: &MockRegistry) -> Run {
    run_setup_with(repository, state, registry, &["--no-browser"])
}

fn run_setup_with(
    repository: &Path,
    state: &Path,
    registry: &MockRegistry,
    browser: &[&str],
) -> Run {
    let output = setup_command(repository, state, registry, browser, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = read_log(state);
    fs::remove_file(state.join("log.jsonl")).expect("reset fake tool log");
    Run { stdout, log }
}

/// A copy of the fixture repository and fake tools, both in `temporary`.
fn prepare(temporary: &TempDir) -> Option<(PathBuf, PathBuf)> {
    let Some(node) = find_node() else {
        eprintln!("skipped: node is not on PATH");
        return None;
    };
    let repository = temporary.path().join("pipeline-template");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pipeline-template"),
        &repository,
    );
    let state = temporary.path().join("state");
    install_fake_tools(&node, &state);
    Some((repository, state))
}

/// A registry where the package is published and trusted, so only the sign-in
/// and the checks around it run.
fn published_registry() -> MockRegistry {
    MockRegistry::start(|path| {
        path.starts_with("/npm/pipeline-app/")
            .then(|| r#"{"version":"0.1.0"}"#.to_owned())
    })
}

#[test]
fn runs_the_whole_npm_bootstrap_with_web_sign_in_and_resumes_safely() {
    let temporary = TempDir::new().expect("create temporary directory");
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let published = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&published);
    let registry = MockRegistry::start(move |path| {
        if path.ends_with("/pipeline-app/0.1.0") {
            seen.store(true, Ordering::SeqCst);
        }
        (path.starts_with("/npm/pipeline-app/") && seen.load(Ordering::SeqCst))
            .then(|| r#"{"version":"0.1.0"}"#.to_owned())
    });

    let run = run_setup(&repository, &state, &registry);
    let commands = run.commands();
    let worktree = commands
        .iter()
        .find(|command| command.starts_with("git worktree add"))
        .expect("worktree command")
        .split(' ')
        .nth(4)
        .expect("worktree path")
        .to_owned();
    let destination = Path::new(&worktree)
        .parent()
        .expect("destination")
        .display()
        .to_string();
    assert_eq!(
        commands,
        [
            "npm pkg get name version repository".to_owned(),
            "npm whoami".to_owned(),
            "npm login --auth-type=web --browser=false".to_owned(),
            "git fetch origin HEAD".to_owned(),
            format!("git worktree add --detach {worktree} FETCH_HEAD"),
            format!("npm pack --ignore-scripts --json --pack-destination {destination}"),
            format!(
                "npm publish {destination}/pipeline-app-0.1.0.tgz --access public --auth-type=web --browser=false --provenance=false"
            ),
            "npx -y npm@latest trust list pipeline-app --json".to_owned(),
            "npx -y npm@latest trust github pipeline-app --repo acme/pipeline-app --file release.yml --allow-publish --yes --browser=false".to_owned(),
            "npx -y npm@latest trust list pipeline-app --json".to_owned(),
            "gh secret list --repo acme/pipeline-app --json name".to_owned(),
            "gh secret delete NPM_TOKEN --repo acme/pipeline-app".to_owned(),
            "npm logout".to_owned(),
            format!("git worktree remove --force {worktree}"),
        ]
    );
    let login = run
        .log
        .iter()
        .find(|entry| entry["argv"][1] == "login")
        .expect("login entry");
    assert_eq!(
        (&login["shim"], &login["tty"]),
        (&Value::Bool(true), &Value::Bool(true)),
        "npm sees a TTY on stdout"
    );
    let pack = run
        .log
        .iter()
        .find(|entry| entry["argv"][1] == "pack")
        .expect("pack entry");
    assert!(
        Path::new(pack["cwd"].as_str().expect("cwd")).ends_with(
            Path::new(&destination)
                .file_name()
                .map(|name| Path::new(name).join("worktree"))
                .expect("worktree name")
        ),
        "npm pack runs inside the temporary worktree"
    );
    for line in [
        "Open https://www.npmjs.com/login?next=/login/cli/fake",
        "Open https://www.npmjs.com/auth/cli/fake",
    ] {
        assert!(run.stdout.lines().any(|output| output == line), "{line}");
    }
    assert!(run.stdout.contains("pipeline-app-0.1.0.tgz: 120 bytes"));
    assert!(
        !Path::new(&destination).exists(),
        "temporary files are removed"
    );

    let resumed = run_setup(&repository, &state, &registry).commands();
    assert!(!resumed
        .iter()
        .any(|command| command.starts_with("npm publish")));
    assert!(!resumed
        .iter()
        .any(|command| command.contains("trust github")));
    assert!(resumed.contains(&"npx -y npm@latest trust list pipeline-app --json".to_owned()));
}

#[test]
fn opens_npm_web_authentication_urls_in_the_default_browser() {
    let temporary = TempDir::new().expect("create temporary directory");
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let registry = published_registry();
    let profile = temporary.path().join("unused-profile");
    let profile_argument = profile.to_str().expect("UTF-8 path");
    let run = run_setup_with(
        &repository,
        &state,
        &registry,
        &[
            "--browser",
            "default",
            "--browser-profile",
            profile_argument,
        ],
    );
    let url = "https://www.npmjs.com/login?next=/login/cli/fake";
    assert!(
        run.stdout
            .lines()
            .any(|line| line == format!("Opening {url} in your default browser")),
        "{}",
        run.stdout
    );
    // The opener is detached, so wait for it to record its arguments.
    let is_opener = |entry: &&Value| matches!(entry["argv"][0].as_str(), Some("open" | "xdg-open"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut opened = run.log.iter().find(is_opener).cloned();
    while opened.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
        opened = read_log(&state).iter().find(is_opener).cloned();
    }
    assert_eq!(opened.expect("opener ran")["argv"][1], url);
    assert!(!profile.exists(), "the automation profile is not used");
}

#[test]
fn stops_the_legacy_username_prompt_with_a_clear_message() {
    let temporary = TempDir::new().expect("create temporary directory");
    let Some((repository, state)) = prepare(&temporary) else {
        return;
    };
    let registry = published_registry();
    let started = Instant::now();
    let output = setup_command(
        &repository,
        &state,
        &registry,
        &["--no-browser"],
        &[("FAKE_LEGACY_LOGIN", "1")],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("the browser login was not completed in time")
            && stderr.contains("re-run the command to get a fresh login link"),
        "{stderr}"
    );
    assert!(started.elapsed() < Duration::from_secs(40), "not awaited");
}
