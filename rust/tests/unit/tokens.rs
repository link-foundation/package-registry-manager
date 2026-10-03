use std::ffi::OsString;
use std::path::{Path, PathBuf};

use package_registry_manager::tokens::{
    audit_token_secrets, cargo_home, read_cargo_token, registry_token, token_state,
    TokenSecretAudit, TokenState,
};
use package_registry_manager::{Package, Registry};
use serde_json::json;

pub fn crate_package(registry: Registry) -> Package {
    let mut package = Package::new(registry, "demo".to_owned(), None, "Cargo.toml".to_owned());
    package.exists_on_registry = Some(false);
    package.trusted_publishing = Some(false);
    package.workflow = Some("release.yml".to_owned());
    package
}

pub fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn splits_token_secrets_into_ones_still_read_and_leftovers() {
    let output = json!([
        { "name": "CARGO_TOKEN" },
        { "name": "CARGO_REGISTRY_TOKEN" },
        { "name": "NPM_TOKEN" },
        { "name": "DOCKERHUB_TOKEN" }
    ])
    .to_string();
    let mut package = crate_package(Registry::CratesIo);
    package.token_secrets = strings(&["CARGO_TOKEN"]);
    assert_eq!(
        audit_token_secrets(&output, &package).expect("audit"),
        TokenSecretAudit {
            in_use: strings(&["CARGO_TOKEN"]),
            leftover: strings(&["CARGO_REGISTRY_TOKEN"]),
        }
    );
    assert_eq!(
        audit_token_secrets(&output, &crate_package(Registry::Npm))
            .expect("audit")
            .leftover,
        strings(&["NPM_TOKEN"])
    );
    assert_eq!(
        audit_token_secrets("", &crate_package(Registry::PyPi)).expect("audit"),
        TokenSecretAudit::default()
    );
}

#[test]
fn reads_the_token_from_the_registry_table_of_cargo_credentials_only() {
    assert_eq!(
        registry_token(
            "[registries.other]\ntoken = \"other\"\n\n[registry] # crates.io\ntoken = \"cio-1\"\n"
        )
        .as_deref(),
        Some("cio-1")
    );
    assert_eq!(
        registry_token("[registry]\ntoken = 'cio-2'\r\n").as_deref(),
        Some("cio-2")
    );
    assert_eq!(registry_token("[registries.x]\ntoken = \"x\"\n"), None);
    assert_eq!(
        cargo_home(Some(OsString::from("/c")), Some(PathBuf::from("/h"))),
        Some(PathBuf::from("/c"))
    );
    assert_eq!(
        cargo_home(None, Some(PathBuf::from("/h"))),
        Some(Path::new("/h").join(".cargo"))
    );

    let directory = tempfile::tempdir().expect("temporary directory");
    assert_eq!(read_cargo_token(directory.path()), None);
    std::fs::write(
        directory.path().join("credentials"),
        "[registry]\ntoken = \"legacy\"\n",
    )
    .expect("write credentials");
    assert_eq!(
        read_cargo_token(directory.path()).as_deref(),
        Some("legacy")
    );
}

#[test]
fn classifies_crates_io_answers_to_a_token() {
    let detail = |text: &str| json!({ "errors": [{ "detail": text }] });
    assert_eq!(
        token_state(403, &detail("authentication failed")),
        Some(TokenState::Revoked)
    );
    assert_eq!(
        token_state(
            401,
            &detail("The given API token does not match the format")
        ),
        Some(TokenState::Revoked)
    );
    assert_eq!(
        token_state(
            403,
            &detail("this action can only be performed on the crates.io website")
        ),
        Some(TokenState::Active)
    );
    assert_eq!(token_state(200, &json!({})), Some(TokenState::Active));
    assert_eq!(token_state(500, &serde_json::Value::Null), None);
    assert_eq!(
        token_state(403, &detail("this action requires authentication")),
        None
    );
}
