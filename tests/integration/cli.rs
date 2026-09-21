use std::path::Path;
use std::process::Command;

#[test]
fn inspect_emits_machine_readable_output() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/polyglot");
    let output = Command::new(env!("CARGO_BIN_EXE_package-registry-manager"))
        .args([
            "--repository",
            repository.to_str().expect("UTF-8 fixture path"),
            "--format",
            "json",
            "inspect",
        ])
        .output()
        .expect("run package-registry-manager");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inspection: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse JSON output");
    assert_eq!(inspection["schema_version"], 1);
    assert_eq!(inspection["packages"].as_array().map(Vec::len), Some(7));
    assert_eq!(inspection["repository"]["github_owner"], "acme");
    assert_eq!(inspection["repository"]["github_repository"], "polyglot");
}

#[test]
fn setup_is_safe_by_default() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/polyglot");
    let output = Command::new(env!("CARGO_BIN_EXE_package-registry-manager"))
        .args([
            "--repository",
            repository.to_str().expect("UTF-8 fixture path"),
            "setup",
            "--registry",
            "npm",
        ])
        .output()
        .expect("run package-registry-manager");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Dry run only"));
}
