use std::fs;

use package_registry_manager::skips::{ignored_by, test_directory};
use package_registry_manager::Skipped;
use package_registry_manager::{inspect_repository, inspect_repository_with, InspectOptions};
use tempfile::TempDir;

fn repository(files: &[(&str, &str)]) -> TempDir {
    let temporary = TempDir::new().expect("create temporary repository");
    for (name, contents) in files {
        let path = temporary.path().join(name);
        fs::create_dir_all(path.parent().expect("file has a parent")).expect("create directory");
        fs::write(path, contents).expect("write file");
    }
    temporary
}

fn npm_package(name: &str) -> String {
    format!("{{\"name\": \"{name}\", \"version\": \"1.0.0\"}}\n")
}

const VERBOSE: InspectOptions = InspectOptions {
    include_skipped: true,
};

fn names(inspection: &package_registry_manager::Inspection) -> Vec<&str> {
    inspection
        .packages
        .iter()
        .map(|package| package.name.as_str())
        .collect()
}

#[test]
fn skips_test_fixture_and_example_manifests() {
    let bom = format!("\u{feff}{}", npm_package("bom"));
    let (tool, fixture, test, snapshot, demo) = (
        npm_package("tool"),
        npm_package("fixture-app"),
        npm_package("test-app"),
        npm_package("snapshot"),
        npm_package("demo"),
    );
    let temporary = repository(&[
        ("package.json", &tool),
        ("tests/fixtures/app/package.json", &fixture),
        ("test/package.json", &test),
        ("src/__fixtures__/package.json", &snapshot),
        ("examples/demo/package.json", &demo),
        ("examples/broken/package.json", "{ not json"),
        ("packages/bom/package.json", &bom),
        ("tests/fixtures/app/Dockerfile", "FROM scratch\n"),
    ]);
    let inspection = inspect_repository(temporary.path()).expect("inspect repository");
    assert_eq!(names(&inspection), ["tool", "bom"]);
    assert!(inspection.skipped.is_empty());

    let verbose = inspect_repository_with(temporary.path(), VERBOSE).expect("inspect repository");
    assert_eq!(
        verbose
            .skipped
            .iter()
            .map(|item| item.manifest.as_str())
            .collect::<Vec<_>>(),
        [
            "examples/broken/package.json",
            "examples/demo/package.json",
            "src/__fixtures__/package.json",
            "test/package.json",
            "tests/fixtures/app/Dockerfile",
            "tests/fixtures/app/package.json",
        ]
    );
    assert!(verbose.skipped[1].reason.starts_with("under examples/"));
}

#[test]
fn keeps_an_example_that_a_workflow_publishes() {
    let demo = npm_package("demo");
    let temporary = repository(&[
        ("examples/demo/package.json", &demo),
        (
            ".github/workflows/release.yml",
            "on: push\njobs:\n  publish:\n    permissions:\n      id-token: write\n    steps:\n      - run: npm publish\n        working-directory: examples/demo\n",
        ),
    ]);
    let inspection = inspect_repository(temporary.path()).expect("inspect repository");
    assert_eq!(
        inspection
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package.workflow.as_deref()))
            .collect::<Vec<_>>(),
        [("demo", Some("release.yml"))]
    );
}

#[test]
fn honors_the_ignore_list() {
    let (tool, legacy, kept, site) = (
        npm_package("tool"),
        npm_package("legacy"),
        npm_package("kept"),
        npm_package("site"),
    );
    let temporary = repository(&[
        ("package.json", &tool),
        ("packages/legacy/package.json", &legacy),
        ("packages/kept/package.json", &kept),
        ("docs/site/package.json", &site),
        (
            ".package-registry-manager.json",
            r#"{"ignore": ["packages/legacy", "docs/**"]}"#,
        ),
    ]);
    let inspection =
        inspect_repository_with(temporary.path(), VERBOSE).expect("inspect repository");
    assert_eq!(names(&inspection), ["tool", "kept"]);
    assert_eq!(
        inspection.skipped,
        [
            Skipped {
                manifest: "docs/site/package.json".to_owned(),
                reason: "ignored by \"docs/**\" in .package-registry-manager.json".to_owned(),
            },
            Skipped {
                manifest: "packages/legacy/package.json".to_owned(),
                reason: "ignored by \"packages/legacy\" in .package-registry-manager.json"
                    .to_owned(),
            },
        ]
    );
}

#[test]
fn rejects_a_malformed_ignore_list() {
    let temporary = repository(&[(".package-registry-manager.json", r#"{"ignore": "tests"}"#)]);
    let error = inspect_repository(temporary.path()).expect_err("malformed ignore list");
    assert!(format!("{error:#}").contains("\"ignore\" must be an array of strings"));
}

#[test]
fn matches_globs_against_a_path_and_its_parents() {
    let patterns = |items: &[&str]| {
        items
            .iter()
            .map(|&item| item.to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ignored_by(&patterns(&["*/legacy"]), "packages/legacy/package.json"),
        Some("*/legacy")
    );
    assert_eq!(
        ignored_by(&patterns(&["**/package.json"]), "a/b/package.json"),
        Some("**/package.json")
    );
    assert_eq!(
        ignored_by(&patterns(&["**/legacy/**"]), "legacy/x/Cargo.toml"),
        Some("**/legacy/**")
    );
    assert_eq!(
        ignored_by(&patterns(&["pack?ges"]), "packages/a/package.json"),
        Some("pack?ges")
    );
    assert_eq!(
        ignored_by(&patterns(&["packages"]), "packages-extra/package.json"),
        None
    );
    assert_eq!(ignored_by(&patterns(&["*.json"]), "a/package.json"), None);
    assert_eq!(test_directory("tests/package.json"), Some("tests"));
    assert_eq!(
        test_directory("src/__tests__/package.json"),
        Some("__tests__")
    );
    assert_eq!(test_directory("package.json"), None);
    assert_eq!(test_directory("src/testing/package.json"), None);
}
