use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use package_registry_manager::auth_urls::{node_options_with_shim, AuthUrlScanner};
use package_registry_manager::registry_state::Endpoints;
use package_registry_manager::{
    build_plans_with, inspect_repository, Package, PlanMode, PlanOptions, Registry, SetupPlan,
};
use regex::Regex;

use crate::pipeline_template::fixture;

pub fn npm_plan(state: impl FnOnce(&mut Package), verify_release: bool) -> SetupPlan {
    let (_temporary, root) = fixture();
    let mut inspection = inspect_repository(&root).expect("inspect fixture");
    state(
        inspection
            .packages
            .iter_mut()
            .find(|package| package.name == "pipeline-app")
            .expect("pipeline-app package"),
    );
    let options = PlanOptions {
        verify_release,
        endpoints: Endpoints::default(),
        ..PlanOptions::default()
    };
    build_plans_with(&inspection, &BTreeSet::from([Registry::Npm]), &options)
        .into_iter()
        .find(|plan| plan.package.name == "pipeline-app")
        .expect("pipeline-app plan")
}

pub fn registry_state(exists: bool, trusted: bool) -> impl FnOnce(&mut Package) {
    move |package| {
        package.exists_on_registry = Some(exists);
        package.trusted_publishing = Some(trusted);
    }
}

fn ids(plan: &SetupPlan) -> Vec<&str> {
    plan.steps.iter().map(|step| step.id.as_str()).collect()
}

fn argv(plan: &SetupPlan, id: &str) -> Vec<String> {
    let command = plan
        .steps
        .iter()
        .find(|step| step.id == id)
        .and_then(|step| step.command.as_ref())
        .unwrap_or_else(|| panic!("{id} has a command"));
    std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect()
}

fn words(text: &str) -> Vec<String> {
    text.split(' ').map(str::to_owned).collect()
}

#[test]
fn plans_a_bootstrap_for_a_package_missing_from_npm() {
    let plan = npm_plan(registry_state(false, false), false);
    assert_eq!(plan.mode, Some(PlanMode::Bootstrap));
    assert_eq!(
        ids(&plan),
        [
            "validate-package",
            "check-registry",
            "check-sign-in",
            "sign-in",
            "check-2fa",
            "enable-2fa",
            "verify-2fa",
            "fetch-default-branch",
            "prepare-worktree",
            "pack",
            "test-install",
            "verify-bins",
            "first-publish",
            "wait-for-registry",
            "check-trust",
            "inspect-release-run",
            "read-release-failure",
            "attach-trusted-publisher",
            "configure-trusted-publisher",
            "verify-trusted-publisher",
            "audit-token-secrets",
            "delete-token-secret",
            "rerun-release",
            "sign-out",
            "remove-worktree",
        ]
    );
    for (id, expected) in [
        ("sign-in", "npm login --auth-type=web --browser=false"),
        (
            "pack",
            "npm pack --ignore-scripts --json --pack-destination {pack_destination}",
        ),
        (
            "first-publish",
            "npm publish {tarball} --access public --auth-type=web --browser=false --provenance=false",
        ),
        (
            "attach-trusted-publisher",
            "npx -y npm@^11.10 trust github pipeline-app --repo acme/pipeline-app --file release.yml --allow-publish --yes --browser=false",
        ),
        (
            "check-trust",
            "npx -y npm@^11.10 trust list pipeline-app --json",
        ),
        ("sign-out", "npm logout"),
    ] {
        assert_eq!(argv(&plan, id), words(expected), "{id}");
    }
}

#[test]
fn plans_only_trusted_publisher_attachment_for_an_existing_package() {
    let plan = npm_plan(registry_state(true, false), false);
    assert_eq!(plan.mode, Some(PlanMode::Attach));
    let ids = ids(&plan);
    for id in ["first-publish", "pack", "prepare-worktree"] {
        assert!(!ids.contains(&id), "{id} must not be planned");
    }
    assert!(ids.contains(&"attach-trusted-publisher"));
}

#[test]
fn plans_nothing_once_trusted_publishing_is_in_use() {
    let plan = npm_plan(registry_state(true, true), false);
    assert_eq!(plan.mode, Some(PlanMode::Complete));
    assert_eq!(plan.steps, [] as [package_registry_manager::SetupStep; 0]);
}

#[test]
fn keeps_every_conditional_step_when_the_registry_state_is_unknown() {
    let plan = npm_plan(|_| {}, true);
    assert_eq!(plan.mode, None);
    let ids = ids(&plan);
    assert_eq!(
        ids[ids.len() - 6..ids.len() - 2],
        [
            "trigger-release",
            "find-release-run",
            "watch-release",
            "confirm-provenance",
        ]
    );
    assert_eq!(
        argv(&plan, "watch-release"),
        words("gh run watch {run_id} --repo acme/pipeline-app --exit-status")
    );
}

#[test]
fn finds_npm_web_authentication_urls_in_streamed_output() {
    let mut scanner = AuthUrlScanner::new();
    let mut urls = Vec::new();
    for chunk in [
        "npm notice Log in on https://registry.npmjs.org/\nLogin at:\n",
        "\u{1b}[1mhttps://www.npmjs.com/login?next=/login/cli/1\u{1b}[22m\nPress ENTER",
        " to open in the browser...\nAuthenticate your account at:\r\nhttps://www.npmjs.com/auth/cli/2\n",
        "Authenticate your account at: https://www.npmjs.com/auth/cli/2\n",
    ] {
        urls.extend(scanner.push(chunk));
    }
    assert_eq!(
        urls,
        [
            "https://www.npmjs.com/login?next=/login/cli/1",
            "https://www.npmjs.com/auth/cli/2",
        ]
    );
    assert_eq!(
        node_options_with_shim(Some("--max-old-space-size=64"), r#"C:\a "b"\shim.cjs"#),
        r#"--max-old-space-size=64 --require "C:\\a \"b\"\\shim.cjs""#
    );
}

#[test]
fn never_reads_writes_or_requests_an_npm_token() {
    let forbidden = [
        r"_authToken",
        r"NODE_AUTH_TOKEN",
        r"\bnpm token\b",
        r#""token",\s*"create""#,
        r"process\.env\.NPM_TOKEN",
        r#"env::var\("NPM_TOKEN"\)"#,
        r"\.npmrc",
    ]
    .map(|pattern| Regex::new(pattern).expect("pattern"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for directory in [root.join("src"), root.join("../js/src")] {
        for entry in fs::read_dir(&directory).expect("read sources") {
            let path = entry.expect("source entry").path();
            let contents = fs::read_to_string(&path).expect("read source");
            for pattern in &forbidden {
                assert!(
                    !pattern.is_match(&contents),
                    "{} matches {pattern}",
                    path.display()
                );
            }
        }
    }
    let token = Regex::new(r"(?i)\btoken\b|NPM_TOKEN.*set").expect("pattern");
    let plan = npm_plan(|_| {}, true);
    for step in &plan.steps {
        if let Some(command) = &step.command {
            let rendered = format!("{} {}", command.program, command.args.join(" "));
            assert!(!token.is_match(&rendered), "{}: {rendered}", step.id);
        }
    }
}

#[test]
fn trims_trailing_slashes_from_endpoint_overrides() {
    let variable = "PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY";
    let base = |value: &str| {
        Endpoints::default()
            .with(variable, value)
            .base(Registry::Npm)
    };
    assert_eq!(
        base("http://mirror.test/npm///").as_deref(),
        Some("http://mirror.test/npm")
    );
    let hostile = format!("{}x", "/".repeat(100_000));
    assert_eq!(base(&hostile), Some(hostile.clone()));
}
