use package_registry_manager::automation::launch_options;
use package_registry_manager::browser_options::{BrowserOptions, ImportScope};
use package_registry_manager::npm_policy::policy_refusal;
use package_registry_manager::npm_similarity::similar_name;
use std::path::Path;

#[test]
fn catches_popular_typos_and_arbitrary_punctuation() {
    for (name, other) in [
        ("gh-upload", "rc-upload"),
        ("lodahs", "lodash"),
        ("jsonstream", "js-on-stream"),
    ] {
        assert!(similar_name(name, other));
    }
    assert!(!similar_name("@acme/gh-upload", "rc-upload"));
    assert!(!similar_name("unique-project-name", "lodash"));
}

#[test]
fn authorization_errors_are_not_naming_errors() {
    assert!(!policy_refusal("E403 403 Forbidden - permission denied"));
    assert!(policy_refusal(
        "403 Forbidden - Package name too similar to existing package rc-upload"
    ));
}

#[test]
fn saved_login_fallback_is_scoped_to_registry_domains() {
    let browser = BrowserOptions {
        import: Some(package_registry_manager::browser_options::ImportSource {
            browser: "chrome".into(),
            profile: Some("Default".into()),
        }),
        import_scope: ImportScope::SavedLogins,
        ..BrowserOptions::default()
    };
    let options = launch_options(&browser, Path::new("/tmp/dedicated"), false, &["pypi.org"]);
    assert_eq!(
        options.migrate_include.unwrap(),
        ["cookies", "passwords", "passkeys"]
    );
    assert_eq!(options.migrate_domains, ["pypi.org"]);
}

#[test]
fn discovers_scoped_saved_passwords_when_installed_profiles_have_no_cookies() {
    use browser_commander::BrowserProfile;
    use package_registry_manager::sign_in_import::{
        find_saved_login_sources, find_sign_in_sources, CookieStores,
    };
    struct Stores;
    impl CookieStores for Stores {
        fn profiles(&self, browser: &str) -> anyhow::Result<Vec<BrowserProfile>> {
            Ok(if matches!(browser, "chrome" | "firefox") {
                vec![BrowserProfile {
                    browser: browser.into(),
                    name: "Default".into(),
                    display_name: String::new(),
                    path: format!("/{browser}/Default").into(),
                    is_default: true,
                    error: None,
                }]
            } else {
                vec![]
            })
        }
        fn count(&self, _: &str, _: &str, _: &str) -> anyhow::Result<usize> {
            Ok(0)
        }
        fn saved_logins(
            &self,
            browser: &str,
            profile: &str,
            domains: &[&str],
        ) -> anyhow::Result<usize> {
            assert_eq!(profile, "Default");
            assert_eq!(domains, ["pypi.org"]);
            Ok(usize::from(browser == "chrome"))
        }
    }
    assert_eq!(
        find_sign_in_sources(&Stores, &["pypi.org"], Some("firefox"), false),
        Vec::new()
    );
    let sources = find_saved_login_sources(&Stores, &["pypi.org"], Some("firefox"), false);
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].browser, "chrome");
    assert_eq!(sources[0].domains, ["pypi.org"]);
}

use anyhow::Result;
use package_registry_manager::model::TrustedPublisherPrefill;
use package_registry_manager::publisher_form::{submit_form, wait_for_form, PublisherPage};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct LoginThenForm {
    polls: AtomicUsize,
    rejected: bool,
}
impl PublisherPage for LoginThenForm {
    async fn evaluate(&self, script: &str) -> Result<Value> {
        tokio::task::yield_now().await;
        if script.contains("shouldSubmit = true") {
            return Ok(json!({"submitted":true}));
        }
        if script.contains("shouldSubmit = false") {
            self.polls.fetch_add(1, Ordering::Relaxed);
            return Ok(if self.polls.load(Ordering::Relaxed) > 2 {
                json!({"filled":["project","organization","repository","workflow"]})
            } else {
                json!({"filled":[]})
            });
        }
        Ok(if self.rejected {
            json!({"error":"project name is already registered"})
        } else {
            json!({"confirmed":true})
        })
    }
}
fn publisher() -> TrustedPublisherPrefill {
    TrustedPublisherPrefill {
        provider: "github".into(),
        organization: "acme".into(),
        repository: "demo".into(),
        workflow: "release.yml".into(),
        environment: None,
        project: Some("demo".into()),
    }
}
#[tokio::test]
async fn pypi_waits_for_sign_in_and_verifies_submission_without_stdin() {
    let page = LoginThenForm {
        polls: AtomicUsize::new(0),
        rejected: false,
    };
    wait_for_form(
        &page,
        &publisher(),
        true,
        Duration::from_millis(1),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert!(page.polls.load(Ordering::Relaxed) > 2);
    submit_form(
        &page,
        &publisher(),
        true,
        true,
        Duration::from_millis(1),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn pypi_server_rejection_stops_the_release() {
    let page = LoginThenForm {
        polls: AtomicUsize::new(0),
        rejected: true,
    };
    let error = submit_form(
        &page,
        &publisher(),
        true,
        true,
        Duration::from_millis(1),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("PyPI refused the pending publisher"));
}
#[tokio::test]
async fn missing_pypi_form_has_a_finite_timeout() {
    let page = LoginThenForm {
        polls: AtomicUsize::new(0),
        rejected: false,
    };
    let error = wait_for_form(
        &page,
        &publisher(),
        true,
        Duration::from_millis(1),
        Duration::ZERO,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("nothing submitted"));
}
#[test]
fn safari_denial_is_not_an_absent_cookie_store() {
    use package_registry_manager::sign_in_import::safari_access_denied;
    assert!(safari_access_denied(
        "safari",
        "Operation not permitted (os error 1)"
    ));
    assert!(safari_access_denied("safari", "EACCES"));
    assert!(!safari_access_denied("safari", "No such file"));
}
