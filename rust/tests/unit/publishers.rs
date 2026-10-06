use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use package_registry_manager::publishers::{detect_publisher, parse_workflow};
use package_registry_manager::workflows::{read_workflows, Workflow};
use package_registry_manager::{build_plans_for, build_plans_with, inspect_repository};
use package_registry_manager::{PlanOptions, Registry, SetupStep};
use tempfile::TempDir;

fn repository(files: &[(&str, &str)]) -> TempDir {
    let temporary = TempDir::new().expect("create temporary repository");
    let git = (
        ".git/config",
        "[remote \"origin\"]\n\turl = https://github.com/acme/tool.git\n",
    );
    for (name, contents) in std::iter::once(&git).chain(files) {
        let path = temporary.path().join(name);
        fs::create_dir_all(path.parent().expect("file has a parent")).expect("create directory");
        fs::write(path, contents).expect("write file");
    }
    temporary
}

fn workflow(name: &str, contents: &str) -> Vec<Workflow> {
    vec![Workflow {
        name: name.to_owned(),
        contents: contents.to_owned(),
    }]
}

/// The layout of package-registry-manager itself (issue #16): npm publishes
/// from release.yml through a script, and desktop-release.yml sorts first.
const SELF_LAYOUT: [(&str, &str); 7] = [
    ("js/package.json", "{\"name\": \"tool\", \"version\": \"1.0.0\"}\n"),
    (
        "rust/Cargo.toml",
        "[package]\nname = \"tool\"\nversion = \"1.0.0\"\n",
    ),
    (
        "js/scripts/publish-to-npm.mjs",
        "import { spawnSync } from \"node:child_process\";\nfunction npm(args) {\n  return spawnSync(\"npm\", args, { stdio: \"inherit\" });\n}\nnpm([\"publish\", \"--access\", \"public\", \"--provenance\"]);\n",
    ),
    (
        "rust/scripts/publish-crate.rs",
        "// `cargo publish` is mentioned here only in a comment.\nlet mut cmd = Command::new(\"cargo\");\ncmd.arg(\"publish\").arg(\"--allow-dirty\");\n",
    ),
    (
        "rust/scripts/preflight.sh",
        "#!/usr/bin/env bash\n# cargo publish would fail without a token\necho 'no credential -- cargo publish would fail with 401'\n",
    ),
    (".github/workflows/desktop-release.yml", DESKTOP_RELEASE),
    (
        ".github/workflows/release.yml",
        "name: Release
on:
  push:
    branches: [main]
permissions:
  contents: read
jobs:
  # npm publish is not run by the preflight
  release-preflight:
    runs-on: ubuntu-latest
    steps:
      - run: bash rust/scripts/preflight.sh
  auto-release:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      id-token: write
    steps:
      - run: rust-script rust/scripts/publish-crate.rs
  javascript-release:
    runs-on: ubuntu-latest
    environment: npm
    permissions: { contents: read, id-token: write }
    steps:
      - name: Publish and verify npm package
        run: node js/scripts/publish-to-npm.mjs
",
    ),
];

const DESKTOP_RELEASE: &str = "name: Desktop release
on: workflow_dispatch
jobs:
  finalize:
    runs-on: ubuntu-latest
    permissions:
      id-token: write
    steps:
      - run: gh release upload v1 app.dmg
";

fn npm_plans(root: &Path, options: &PlanOptions) -> Vec<package_registry_manager::SetupPlan> {
    let inspection = inspect_repository(root).expect("inspect repository");
    build_plans_with(&inspection, &BTreeSet::from([Registry::Npm]), options)
}

#[test]
fn finds_the_job_that_publishes_to_npm_through_a_script() {
    let temporary = repository(&SELF_LAYOUT);
    let inspection = inspect_repository(temporary.path()).expect("inspect repository");
    let npm = inspection
        .packages
        .iter()
        .find(|package| package.registry == Registry::Npm)
        .expect("npm package");
    assert_eq!(npm.workflow.as_deref(), Some("release.yml"));
    assert_eq!(npm.workflow_jobs, ["javascript-release"]);
    assert_eq!(npm.environment.as_deref(), Some("npm"));
    let crate_package = inspection
        .packages
        .iter()
        .find(|package| package.registry == Registry::CratesIo)
        .expect("crate package");
    assert_eq!(crate_package.workflow.as_deref(), Some("release.yml"));
    assert_eq!(crate_package.workflow_jobs, ["auto-release"]);
    assert_eq!(
        inspection.repository.release_workflow.as_deref(),
        Some("release.yml")
    );

    let plans = build_plans_for(&inspection, &BTreeSet::from([Registry::Npm]));
    let publisher = plans[0].oidc_publisher.as_ref().expect("publisher");
    assert_eq!(
        (
            publisher.provider.as_str(),
            publisher.organization.as_str(),
            publisher.repository.as_str(),
            publisher.workflow.as_str(),
            publisher.environment.as_deref(),
        ),
        ("github-actions", "acme", "tool", "release.yml", Some("npm"))
    );
    let attach = plans[0]
        .steps
        .iter()
        .find(|step| step.id == "attach-trusted-publisher")
        .and_then(|step| step.command.as_ref())
        .expect("attach command");
    let start = attach
        .args
        .iter()
        .position(|arg| arg == "--file")
        .expect("--file");
    let end = attach
        .args
        .iter()
        .position(|arg| arg == "--allow-publish")
        .expect("--allow-publish");
    assert_eq!(
        attach.args[start..end],
        ["--file", "release.yml", "--env", "npm"]
    );
}

#[test]
fn never_falls_back_to_a_workflow_named_like_a_release() {
    let temporary = repository(&[
        (
            "package.json",
            "{\"name\": \"tool\", \"version\": \"1.0.0\"}\n",
        ),
        (".github/workflows/desktop-release.yml", DESKTOP_RELEASE),
    ]);
    let inspection = inspect_repository(temporary.path()).expect("inspect repository");
    assert_eq!(inspection.repository.release_workflow, None);
    assert_eq!(inspection.packages[0].workflow, None);
    let plans = build_plans_for(&inspection, &BTreeSet::from([Registry::Npm]));
    assert!(plans[0].oidc_publisher.is_none());
}

#[test]
fn recognizes_the_common_npm_publish_commands() {
    for command in [
        "npm publish --provenance",
        "npm stage publish",
        "pnpm -r publish --no-git-checks",
        "yarn npm publish",
        "npx changeset publish",
    ] {
        let workflows = workflow(
            "ci.yml",
            &format!("on: push\npermissions:\n  id-token: write\njobs:\n  ship:\n    runs-on: ubuntu-latest\n    steps:\n      - run: {command}\n"),
        );
        let result = detect_publisher(Path::new("/nonexistent"), &workflows, Registry::Npm);
        assert_eq!(result.workflow.as_deref(), Some("ci.yml"), "{command}");
        assert_eq!(result.jobs, ["ship"], "{command}");
    }
    let dry_run = detect_publisher(
        Path::new("/nonexistent"),
        &workflow(
            "ci.yml",
            "on: push\njobs:\n  check:\n    steps:\n      - run: npm publish --dry-run\n",
        ),
        Registry::Npm,
    );
    assert_eq!(dry_run.workflow, None);
}

#[test]
fn follows_changesets_action_and_package_scripts() {
    let temporary = repository(&[
        (
            "package.json",
            "{\"name\": \"tool\", \"version\": \"1.0.0\", \"scripts\": {\"release\": \"changeset publish\"}}\n",
        ),
        (
            ".github/workflows/version.yml",
            "on: push\njobs:\n  version:\n    permissions:\n      id-token: write\n    steps:\n      - uses: changesets/action@v1\n        with:\n          publish: pnpm release\n",
        ),
    ]);
    let workflows = read_workflows(temporary.path()).expect("read workflows");
    let result = detect_publisher(temporary.path(), &workflows, Registry::Npm);
    assert_eq!(result.workflow.as_deref(), Some("version.yml"));
    assert_eq!(result.jobs, ["version"]);
}

#[test]
fn follows_a_package_script_that_a_release_script_runs() {
    // The layout of the link-foundation pipeline templates, e.g. browser-commander.
    let temporary = repository(&[
        (
            "js/package.json",
            "{\"name\": \"tool\", \"version\": \"1.0.0\", \"scripts\": {\"changeset:publish\": \"changeset publish\"}}\n",
        ),
        (
            "js/scripts/publish-to-npm.mjs",
            "import { $ } from \"command-stream\";\n// npm run changeset:publish --dry-run is not run\nawait $`npm run changeset:publish`;\n",
        ),
        (
            ".github/workflows/js.yml",
            "on: push\ndefaults:\n  run:\n    working-directory: js\njobs:\n  release:\n    permissions:\n      id-token: write\n    steps:\n      - run: node scripts/publish-to-npm.mjs --should-pull\n",
        ),
    ]);
    let workflows = read_workflows(temporary.path()).expect("read workflows");
    let result = detect_publisher(temporary.path(), &workflows, Registry::Npm);
    assert_eq!(result.workflow.as_deref(), Some("js.yml"));
    assert_eq!(result.jobs, ["release"]);
}

#[cfg(unix)]
#[test]
fn follows_scripts_when_the_checkout_is_reached_through_a_symlink() {
    // macOS's temporary directory is /var -> /private/var, and Windows may
    // name it with an 8.3 short name; the canonical path differs from the one given.
    let temporary = repository(&[
        ("package.json", "{\"name\": \"tool\", \"version\": \"1.0.0\"}\n"),
        ("scripts/publish.mjs", "await $`npm publish`;\n"),
        (
            ".github/workflows/release.yml",
            "on: push\njobs:\n  release:\n    permissions:\n      id-token: write\n    steps:\n      - run: node scripts/publish.mjs\n",
        ),
    ]);
    let links = TempDir::new().expect("create link directory");
    let root = links.path().join("checkout");
    std::os::unix::fs::symlink(temporary.path(), &root).expect("link the checkout");
    let workflows = read_workflows(&root).expect("read workflows");
    let result = detect_publisher(&root, &workflows, Registry::Npm);
    assert_eq!(result.workflow.as_deref(), Some("release.yml"));
}

#[test]
fn recognizes_the_changesets_action_publish_sub_action() {
    let result = detect_publisher(
        Path::new("/nonexistent"),
        &workflow(
            "publish.yml",
            "on: push\njobs:\n  publish:\n    permissions:\n      id-token: write\n    steps:\n      - uses: changesets/action/publish@ae32849d5ba541f9ae29e40e22a623bc13562f51 # v2.1.2\n",
        ),
        Registry::Npm,
    );
    assert_eq!(result.workflow.as_deref(), Some("publish.yml"));
    assert_eq!(result.jobs, ["publish"]);
}

#[test]
fn names_the_caller_of_a_reusable_publishing_workflow() {
    let temporary = repository(&[
        ("package.json", "{\"name\": \"tool\", \"version\": \"1.0.0\"}\n"),
        (
            ".github/workflows/publish.yml",
            "on:\n  workflow_call:\njobs:\n  publish:\n    permissions:\n      id-token: write\n    steps:\n      - run: npm publish\n",
        ),
        (
            ".github/workflows/release.yml",
            "on:\n  push:\njobs:\n  npm:\n    permissions:\n      id-token: write\n    uses: ./.github/workflows/publish.yml\n",
        ),
    ]);
    let workflows = read_workflows(temporary.path()).expect("read workflows");
    let result = detect_publisher(temporary.path(), &workflows, Registry::Npm);
    assert_eq!(result.workflow.as_deref(), Some("release.yml"));
    assert_eq!(result.jobs, ["npm"]);
}

#[test]
fn stops_and_asks_when_several_workflows_publish() {
    let publish = |job: &str| {
        format!("on: push\njobs:\n  {job}:\n    permissions:\n      id-token: write\n    steps:\n      - run: npm publish\n")
    };
    let (one, two) = (publish("one"), publish("two"));
    let temporary = repository(&[
        (
            "package.json",
            "{\"name\": \"tool\", \"version\": \"1.0.0\"}\n",
        ),
        (".github/workflows/a.yml", &one),
        (".github/workflows/b.yml", &two),
    ]);
    let inspection = inspect_repository(temporary.path()).expect("inspect repository");
    let npm = &inspection.packages[0];
    assert_eq!(npm.workflow, None);
    assert_eq!(npm.workflow_candidates, ["a.yml", "b.yml"]);
    assert!(npm.warnings[0].contains("pass --workflow <file>"));
    assert_eq!(inspection.repository.release_workflow, None);

    let skipped = &npm_plans(temporary.path(), &PlanOptions::default())[0];
    assert_eq!(skipped.steps, [] as [SetupStep; 0]);
    assert!(skipped
        .skipped_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("several workflows publish to npm")));

    let chosen = &npm_plans(
        temporary.path(),
        &PlanOptions {
            workflow: Some("b.yml".to_owned()),
            publisher_environment: Some("release".to_owned()),
            ..PlanOptions::default()
        },
    )[0];
    assert_eq!(chosen.skipped_reason, None);
    let publisher = chosen.oidc_publisher.as_ref().expect("publisher");
    assert_eq!(publisher.workflow, "b.yml");
    assert_eq!(publisher.environment.as_deref(), Some("release"));
}

#[test]
fn warns_when_the_publishing_job_cannot_mint_an_oidc_token() {
    let result = detect_publisher(
        Path::new("/nonexistent"),
        &workflow(
            "release.yml",
            "on: push\npermissions:\n  contents: write\njobs:\n  publish:\n    steps:\n      - run: npm publish\n",
        ),
        Registry::Npm,
    );
    assert_eq!(result.workflow.as_deref(), Some("release.yml"));
    assert!(result.warnings[0].contains("without `id-token: write`"));
}

#[test]
fn warns_about_long_lived_registry_token_secrets() {
    let mut files = SELF_LAYOUT.to_vec();
    files.push((
        ".github/workflows/token.yml",
        "on: push\njobs:\n  publish:\n    steps:\n      - run: cargo publish --dry-run\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_TOKEN }}\n",
    ));
    let temporary = repository(&files);
    let inspection = inspect_repository(temporary.path()).expect("inspect repository");
    let crate_package = inspection
        .packages
        .iter()
        .find(|package| package.registry == Registry::CratesIo)
        .expect("crate package");
    assert_eq!(crate_package.token_secrets, ["CARGO_TOKEN"]);
    assert!(crate_package.warnings.join("\n").contains(
        "token.yml reads secrets.CARGO_TOKEN; publish with crates.io trusted publishing"
    ));
}

#[test]
fn splits_jobs_by_indentation_and_ignores_comments() {
    let parsed = parse_workflow(
        "on: push\njobs:\n    # a comment\n    first:\n        steps: []\n    second-job:\n        runs-on: x\nenv:\n  A: b\n",
    );
    assert_eq!(
        parsed
            .jobs
            .iter()
            .map(|job| job.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second-job"]
    );
    assert_eq!(parsed.header, ["on: push", "env:", "  A: b"]);
}

#[test]
fn follows_scripts_running_python_module_twine_issue_33() {
    for (runner, script, contents) in [
        (
            "node",
            "scripts/publish-to-pypi.mjs",
            "await $`cd python && python -m twine upload dist/*`;",
        ),
        (
            "bash",
            "scripts/publish.sh",
            "python -m twine upload dist/*",
        ),
        (
            "python",
            "scripts/publish.py",
            "subprocess.run([\"python\", \"-m\", \"twine\", \"upload\", \"dist/*\"])",
        ),
    ] {
        let yaml = format!("on: workflow_dispatch\njobs:\n  python-release:\n    permissions: {{id-token: write}}\n    steps:\n      - run: {runner} {script}\n");
        let root = repository(&[
            (
                "python/pyproject.toml",
                "[project]\nname=\"tool\"\nversion=\"1.0.0\"\n",
            ),
            (script, contents),
            (".github/workflows/release.yml", &yaml),
        ]);
        let inspection = inspect_repository(root.path()).unwrap();
        assert_eq!(
            inspection.packages[0].workflow.as_deref(),
            Some("release.yml"),
            "{runner}"
        );
        assert_eq!(inspection.packages[0].workflow_jobs, ["python-release"]);
    }
}

#[test]
fn blocks_setup_without_a_publishing_job_issue_33() {
    let root = repository(&[("package.json", "{\"name\":\"tool\"}")]);
    let inspection = inspect_repository(root.path()).unwrap();
    assert!(inspection.packages[0]
        .warnings
        .join("\n")
        .contains("no workflow publishes tool to npm; CI releases will not reach it"));
    let plans = build_plans_for(&inspection, &BTreeSet::new());
    assert!(plans[0]
        .skipped_reason
        .as_deref()
        .unwrap()
        .contains("publishing job"));
    assert!(plans[0].oidc_publisher.is_none());
    assert_eq!(
        plans[0]
            .steps
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["add-publishing-workflow"]
    );
}
