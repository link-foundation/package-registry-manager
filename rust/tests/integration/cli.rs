use std::path::Path;
use std::process::{Command, Output};

use crate::mock_registry::MockRegistry;

fn polyglot() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/polyglot")
        .to_str()
        .expect("UTF-8 fixture path")
        .to_owned()
}

fn run(args: &[&str], registry: Option<&MockRegistry>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_package-registry-manager"));
    command.args(args);
    if let Some(registry) = registry {
        command.envs(registry.env());
    }
    let output = command.output().expect("run package-registry-manager");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn inspect_emits_machine_readable_output() {
    let repository = polyglot();
    let output = run(
        &[
            "--repository",
            &repository,
            "--format",
            "json",
            "inspect",
            "--offline",
        ],
        None,
    );
    let inspection: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse JSON output");
    assert_eq!(inspection["schema_version"], 1);
    let packages = inspection["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 9);
    assert!(packages
        .iter()
        .all(|package| package.get("exists_on_registry").is_none()));
    assert_eq!(inspection["repository"]["github_owner"], "acme");
    assert_eq!(inspection["repository"]["github_repository"], "polyglot");
}

#[test]
fn setup_is_safe_by_default() {
    let registry = MockRegistry::start(|_| None);
    let repository = polyglot();
    let output = run(
        &["--repository", &repository, "setup", "--registry", "npm"],
        Some(&registry),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "Dry run only",
        "(bootstrap)",
        "npm login --auth-type=web --browser=false",
        "npm publish {tarball} --access public",
        "prerequisites:\n    - Node.js: ",
        "npm two-factor authentication: ",
        "npm logout",
    ] {
        assert!(stdout.contains(expected), "{expected} in\n{stdout}");
    }
    assert!(
        regex::Regex::new(r"npx -y npm@(?:\^12|\^11\.10) trust github @acme/widgets")
            .expect("valid pattern")
            .is_match(&stdout),
        "npm trust through npm 11.10 or 12 in\n{stdout}"
    );
    assert!(registry
        .requests()
        .contains(&"/npm/@acme%2Fwidgets/latest".to_owned()));
}

#[test]
fn setup_all_selects_every_publishable_package_and_accepts_a_registry_filter() {
    let repository = polyglot();
    let output = run(
        &["--repository", &repository, "--offline", "setup", "--all"],
        None,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "npm: @acme/widgets",
        "crates-io: acme-widgets",
        "pypi: acme-widgets",
        "Dry run only",
    ] {
        assert!(stdout.contains(expected), "{expected} in {stdout}");
    }
    let output = run(
        &[
            "--repository",
            &repository,
            "--offline",
            "setup",
            "--all",
            "--registry",
            "npm",
        ],
        None,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("npm: @acme/widgets"));
    assert!(!stdout.contains("crates-io:"));
    let output = Command::new(env!("CARGO_BIN_EXE_package-registry-manager"))
        .args(["--offline", "setup", "--all", "--package", "tool"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn plan_reports_registry_state_in_json() {
    let registry = MockRegistry::start(|_| None);
    let repository = polyglot();
    let output = run(
        &["--repository", &repository, "--format", "json", "plan"],
        Some(&registry),
    );
    let plans: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("parse JSON output");
    let find = |registry: &str| {
        plans
            .iter()
            .find(|plan| plan["registry"] == registry)
            .expect("plan for registry")
    };
    let npm = find("npm");
    assert_eq!(npm["mode"], "bootstrap");
    assert_eq!(npm["package"]["exists_on_registry"], false);
    assert_eq!(npm["package"]["trusted_publishing"], false);
    let docker_hub = find("docker-hub");
    assert_eq!(docker_hub["mode"], "bootstrap");
    assert!(docker_hub["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .any(|step| step["id"] == "create-repository"));
}

#[test]
fn plan_package_keeps_only_the_named_package() {
    let repository = polyglot();
    let output = run(
        &[
            "--repository",
            &repository,
            "--format",
            "json",
            "plan",
            "--offline",
            "--package",
            "acme-widgets",
        ],
        None,
    );
    let plans: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("parse JSON output");
    let mut registries: Vec<&str> = plans
        .iter()
        .map(|plan| plan["registry"].as_str().expect("registry"))
        .collect();
    registries.sort_unstable();
    assert_eq!(registries, ["crates-io", "pypi"]);
    assert!(plans
        .iter()
        .all(|plan| plan["package"]["name"] == "acme-widgets"));
    let missing = Command::new(env!("CARGO_BIN_EXE_package-registry-manager"))
        .args([
            "--repository",
            &repository,
            "plan",
            "--offline",
            "--package",
            "missing",
        ])
        .output()
        .expect("run package-registry-manager");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("package 'missing' was not found"));
}

#[test]
fn dry_run_conflicts_with_execute() {
    let output = Command::new(env!("CARGO_BIN_EXE_package-registry-manager"))
        .args([
            "--repository",
            &polyglot(),
            "--offline",
            "setup",
            "--registry",
            "npm",
            "--dry-run",
            "--execute",
        ])
        .output()
        .expect("run package-registry-manager");
    assert!(!output.status.success());
}

#[test]
fn prints_the_package_version() {
    let output = run(&["--version"], None);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("package-registry-manager {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_lists_every_catalogue_import_and_launch_channel() {
    let output = run(&["setup", "--help"], None);
    let help = String::from_utf8_lossy(&output.stdout);
    for id in package_registry_manager::sign_in_import::import_sources()
        .into_iter()
        .chain(package_registry_manager::browser_catalogue::launch_channels())
    {
        assert!(help.contains(id), "{id} in\n{help}");
    }
}
