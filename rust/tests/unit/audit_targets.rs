use package_registry_manager::{inspect_repository_with, InspectOptions};
use std::fs;
use tempfile::TempDir;

#[test]
fn audit_targets_preserve_aliases_workspace_versions_and_skip_non_artifacts() {
    let root = TempDir::new().unwrap();
    for (name, content) in [
        (
            "package.json",
            r#"{"name":"original","version":"1.2.3","publishAliases":["alias"],"scripts":{"prepare:npm":"node scripts/prepare-npm-packages.mjs"}}"#,
        ),
        ("scripts/prepare-npm-packages.mjs", "// generator"),
        (
            "rust/Cargo.toml",
            "[workspace]\nmembers = [\"member\"]\n[workspace.package]\nversion = \"0.5.0\"\n",
        ),
        (
            "rust/member/Cargo.toml",
            "[package]\nname = \"member\"\nversion.workspace = true\n",
        ),
        (
            "android/app/build.gradle",
            "plugins { id 'com.android.application' }",
        ),
        ("android/build.gradle", "buildscript { dependencies {} }"),
        (
            "dev/log/issues/1/package.json",
            r#"{"name":"template","version":"1.0.0"}"#,
        ),
    ] {
        let target = root.path().join(name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, content).unwrap();
    }
    let result = inspect_repository_with(
        root.path(),
        InspectOptions {
            include_skipped: true,
        },
    )
    .unwrap();
    assert_eq!(result.packages.len(), 3);
    assert_eq!(result.packages[0].name, "alias");
    assert_eq!(result.packages[2].version.as_deref(), Some("0.5.0"));
    assert_eq!(result.skipped.len(), 3);
}
