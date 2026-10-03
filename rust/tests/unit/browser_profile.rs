use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use browser_commander::browser::open_in_user_browser::build_open_command;
use browser_commander::utilities::subprocess::CommandError;
#[cfg(unix)]
use package_registry_manager::approvals::LinkKind;
use package_registry_manager::auth_urls::AuthUrlScanner;
use package_registry_manager::browser::{open_in_user_browser, opener_platform, opener_succeeded};
use package_registry_manager::profile::{
    default_browser_profile, default_browser_profile_for, ensure_profile_ignored,
    legacy_browser_profile, protect_legacy_profile,
};

fn git(cwd: &Path, args: &[&str]) -> (bool, String) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn ignored(cwd: &Path, file: &str) -> bool {
    git(cwd, &["check-ignore", "--quiet", "--", file]).0
}

fn repository() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("repository");
    fs::create_dir_all(&root).expect("create repository");
    assert!(git(&root, &["init", "--quiet"]).0, "git init");
    (temporary, root)
}

#[test]
fn defaults_to_a_per_user_profile_outside_any_repository() {
    let none = |_: &str| None;
    let suffix = Path::new("package-registry-manager").join("browser-profile");
    assert_eq!(
        default_browser_profile_for("macos", none, Path::new("/Users/me")),
        Path::new("/Users/me/Library/Application Support").join(&suffix)
    );
    assert_eq!(
        default_browser_profile_for("linux", none, Path::new("/home/me")),
        Path::new("/home/me/.local/state").join(&suffix)
    );
    let state = |value: &'static str| {
        move |name: &str| (name == "XDG_STATE_HOME").then(|| value.to_owned())
    };
    assert_eq!(
        default_browser_profile_for("linux", state("/state"), Path::new("/home/me")),
        Path::new("/state").join(&suffix)
    );
    assert_eq!(
        default_browser_profile_for("linux", state("relative"), Path::new("/home/me")),
        Path::new("/home/me/.local/state").join(&suffix)
    );
    let local =
        |name: &str| (name == "LOCALAPPDATA").then(|| r"C:\Users\me\AppData\Local".to_owned());
    assert_eq!(
        default_browser_profile_for("windows", local, Path::new(r"C:\Users\me")),
        Path::new(r"C:\Users\me\AppData\Local").join(&suffix)
    );

    let (_temporary, root) = repository();
    let profile = default_browser_profile().expect("home directory");
    assert!(profile.is_absolute(), "{}", profile.display());
    assert!(!profile.starts_with(&root), "{}", profile.display());
}

#[tokio::test]
async fn ignores_a_profile_inside_the_repository_before_first_use() {
    let (_temporary, root) = repository();
    let profile = legacy_browser_profile(&root);
    let cookies = ".package-registry-manager/browser-profile/Default/Cookies";
    assert!(!ignored(&root, cookies), "reproduces issue #8");

    ensure_profile_ignored(&profile, false)
        .await
        .expect("guard");

    assert_eq!(
        fs::read_to_string(root.join(".package-registry-manager/.gitignore")).expect("read"),
        "*\n"
    );
    assert!(ignored(&root, cookies));
    fs::create_dir_all(profile.join("Default")).expect("create");
    fs::write(root.join(cookies), "session").expect("write cookies");
    assert!(git(&root, &["add", "-A"]).0);
    assert_eq!(git(&root, &["diff", "--cached", "--name-only"]).1, "");
}

#[tokio::test]
async fn ignores_an_explicit_profile_anywhere_in_a_work_tree() {
    let (_temporary, root) = repository();
    let profile = root.join("work").join("profile");
    ensure_profile_ignored(&profile, false)
        .await
        .expect("guard");
    assert_eq!(
        fs::read_to_string(profile.join(".gitignore")).expect("read"),
        "*\n"
    );
    assert!(ignored(&root, "work/profile/Default/Cookies"));
    assert!(!ignored(&root, "work/other"));
}

#[tokio::test]
async fn refuses_a_profile_whose_files_git_already_tracks() {
    let (_temporary, root) = repository();
    let profile = legacy_browser_profile(&root);
    fs::create_dir_all(&profile).expect("create");
    fs::write(profile.join("Local State"), "{}").expect("write");
    assert!(git(&root, &["add", "-A"]).0);
    let error = ensure_profile_ignored(&profile, false)
        .await
        .expect_err("tracked profile");
    assert!(
        error.to_string().contains("Git does not ignore it"),
        "{error}"
    );
}

#[tokio::test]
async fn leaves_a_profile_outside_any_work_tree_alone() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let profile = temporary.path().join("outside").join("profile");
    ensure_profile_ignored(&profile, false)
        .await
        .expect("guard");
    assert!(profile.is_dir());
    assert!(!profile.join(".gitignore").exists());
}

#[tokio::test]
async fn protects_a_profile_left_in_the_repository() {
    let (temporary, root) = repository();
    fs::create_dir_all(legacy_browser_profile(&root)).expect("create");
    protect_legacy_profile(&root, &temporary.path().join("new-profile"), false)
        .await
        .expect("protect");
    assert!(ignored(
        &root,
        ".package-registry-manager/browser-profile/x"
    ));
}

#[test]
fn opens_urls_in_the_default_browser_without_a_shell() {
    let url = "https://www.npmjs.com/login?next=/login/cli/1&a=b";
    let opener = |os: &str| build_open_command(url, opener_platform(os)).expect("opener");
    assert_eq!(opener("macos"), ["open", url]);
    assert_eq!(opener("linux"), ["xdg-open", url]);
    assert_eq!(opener("freebsd"), ["xdg-open", url]);
    assert_eq!(opener("windows"), ["explorer.exe", url]);
}

#[tokio::test]
async fn refuses_to_open_non_web_urls() {
    for hostile in [
        "--help",
        "file:///etc/passwd",
        "https://a b",
        "https://",
        "javascript:alert(1)",
    ] {
        let error = open_in_user_browser(hostile).await.expect_err(hostile);
        assert!(
            error.to_string().contains("non-web URL"),
            "{hostile}: {error}"
        );
    }
}

#[test]
fn treats_explorer_exit_code_one_as_handed_over() {
    let exited = |file: &str, code| {
        anyhow::Error::new(CommandError::Exited {
            file: file.to_owned(),
            args: Vec::new(),
            code,
            stdout: String::new(),
            stderr: String::new(),
        })
    };
    assert!(opener_succeeded(&exited("explorer.exe", 1)));
    assert!(!opener_succeeded(&exited("explorer.exe", 2)));
    assert!(!opener_succeeded(&exited("xdg-open", 1)));
    assert!(!opener_succeeded(&anyhow::anyhow!("xdg-open: missing")));
}

#[test]
fn detects_the_legacy_username_prompt_without_a_newline() {
    let mut scanner = AuthUrlScanner::new();
    let urls = scanner.push("Login at:\nhttps://www.npmjs.com/login?next=/login/cli/1\n");
    assert_eq!(urls, ["https://www.npmjs.com/login?next=/login/cli/1"]);
    assert!(!scanner.legacy_login());
    scanner.push("Enter OTP: ");
    assert!(
        !scanner.legacy_login(),
        "a one-time password prompt is fine"
    );
    scanner.push("\n\u{1b}[0mUser");
    assert!(!scanner.legacy_login());
    scanner.push("name: ");
    assert!(scanner.legacy_login());
}

#[cfg(unix)]
#[tokio::test]
async fn stops_npm_at_its_legacy_username_prompt() {
    use package_registry_manager::auth_urls::run_interactive;
    use package_registry_manager::model::CommandSpec;
    use std::time::{Duration, Instant};

    let temporary = tempfile::tempdir().expect("temporary directory");
    let command = CommandSpec {
        program: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "printf 'Login at:\\nhttps://www.npmjs.com/login?next=/login/cli/1\\nUsername: '; sleep 60"
                .to_owned(),
        ],
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = Instant::now();
    let output = run_interactive(
        &command,
        temporary.path(),
        &std::collections::BTreeMap::new(),
        false,
        Some(sender),
        true,
        false,
    )
    .await
    .expect("run");
    assert!(output.legacy_login);
    assert!(started.elapsed() < Duration::from_secs(20), "not awaited");
    assert_eq!(
        receiver.recv().await,
        Some((
            "https://www.npmjs.com/login?next=/login/cli/1".to_owned(),
            LinkKind::Login
        ))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn notices_an_expired_approval_on_stderr() {
    use package_registry_manager::approvals::expired_approval;
    use package_registry_manager::auth_urls::run_interactive;
    use package_registry_manager::model::CommandSpec;

    let temporary = tempfile::tempdir().expect("temporary directory");
    let command = CommandSpec {
        program: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "printf 'Authenticate your account at:\\nhttps://www.npmjs.com/auth/cli/1\\n'; printf '\\033[31mnpm error\\033[0m Invalid response from web login endpoint\\n' >&2; exit 1"
                .to_owned(),
        ],
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let output = run_interactive(
        &command,
        temporary.path(),
        &std::collections::BTreeMap::new(),
        false,
        Some(sender),
        true,
        true,
    )
    .await
    .expect("run");
    assert_eq!(output.code, 1);
    assert!(output.approval_expired);
    assert_eq!(
        expired_approval(&output),
        Some("npm's approval session ended")
    );
    assert_eq!(
        receiver.recv().await,
        Some((
            "https://www.npmjs.com/auth/cli/1".to_owned(),
            LinkKind::Approve
        ))
    );
}
