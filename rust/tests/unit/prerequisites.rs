use package_registry_manager::browser_options::{AttachMode, ImportSource};
use package_registry_manager::prerequisites::{
    github_auth, plan_prerequisites, render_prerequisites, resolve_trust_npm, trust_npm_spec,
    two_factor_mode, BrowserDisplay, BrowserSummary, Environment, Github, GithubAuth, TwoFactor,
};
use serde_json::json;

use crate::npm_bootstrap::{npm_plan, registry_state};

#[test]
fn runs_npm_trust_with_npm_12_only_on_node_versions_npm_12_supports() {
    for version in ["v22.22.2", "v24.15.0", "v24.21.0", "v26.0.0"] {
        assert_eq!(trust_npm_spec(Some(version)), "npm@^12", "{version}");
    }
    for version in ["v20.19.4", "v22.22.1", "v24.14.9", "v25.9.0"] {
        assert_eq!(trust_npm_spec(Some(version)), "npm@^11.10", "{version}");
    }
    assert_eq!(trust_npm_spec(None), "npm@^11.10");
}

#[test]
fn resolves_the_trust_npm_from_the_matching_dist_tag() {
    let tags = json!({"latest": "12.2.0", "next-11": "11.21.0", "next-12": "12.2.0"});
    assert_eq!(
        resolve_trust_npm("npm@^12", Some(&tags)).as_deref(),
        Some("12.2.0")
    );
    assert_eq!(
        resolve_trust_npm("npm@^11.10", Some(&tags)).as_deref(),
        Some("11.21.0")
    );
    assert_eq!(
        resolve_trust_npm("npm@^11.10", Some(&json!({"latest": "12.2.0"}))),
        None
    );
    assert_eq!(resolve_trust_npm("npm@^12", None), None);
}

#[test]
fn reads_the_2fa_mode_from_npm_profile_get() {
    assert_eq!(
        two_factor_mode(r#"{"tfa":{"pending":false,"mode":"auth-and-writes"}}"#)
            .expect("valid JSON")
            .as_deref(),
        Some("auth-and-writes")
    );
    assert_eq!(
        two_factor_mode(r#"{"tfa":false}"#).expect("valid JSON"),
        None
    );
    assert_eq!(
        two_factor_mode(r#"{"tfa":{"pending":true,"mode":"auth-only"}}"#).expect("valid JSON"),
        None
    );
    assert!(two_factor_mode("not json").is_err());
}

#[test]
fn summarizes_gh_auth_status_without_credentials() {
    let output = json!({
        "hosts": {
            "github.com": [{
                "state": "success",
                "active": true,
                "login": "octo",
                "scopes": "gist, repo, workflow",
            }],
        },
    })
    .to_string();
    assert_eq!(
        github_auth(&output),
        Some(GithubAuth {
            login: Some("octo".to_owned()),
            scopes: vec!["gist".to_owned(), "repo".to_owned(), "workflow".to_owned()],
        })
    );
    assert_eq!(
        github_auth(r#"{"hosts":{}}"#),
        Some(GithubAuth {
            login: None,
            scopes: Vec::new(),
        })
    );
    assert_eq!(github_auth("gh: unknown flag --json"), None);
}

#[test]
fn lists_manual_prerequisites_before_an_npm_plan() {
    let plan = npm_plan(registry_state(false, false), false);
    let environment = Environment {
        offline: false,
        node: Some("v20.19.4".to_owned()),
        npm: Some("10.8.2".to_owned()),
        trust_npm: "npm@^11.10".to_owned(),
        trust_npm_version: Some("11.21.0".to_owned()),
        two_factor: TwoFactor::Off,
        github: Github::SignedIn {
            login: "octo".to_owned(),
            scopes: vec!["gist".to_owned()],
        },
    };
    let browser = BrowserDisplay {
        mode: BrowserSummary::None,
        ..BrowserDisplay::default()
    };
    let items = plan_prerequisites(&plan, Some(&environment), &browser);
    assert_eq!(
        items
            .iter()
            .map(|item| (item.id.as_str(), item.ok))
            .collect::<Vec<_>>(),
        [
            ("node", Some(true)),
            ("npm", Some(true)),
            ("npm-trust", Some(true)),
            ("npm-2fa", Some(false)),
            ("gh", Some(false)),
            ("browser", None),
        ]
    );
    assert_eq!(
        render_prerequisites(&items),
        [
            "  prerequisites:",
            "    - Node.js: v20.19.4; needs ^20.17.0 || >=22.9.0",
            "    - npm: 10.8.2; needs installed",
            "    - npm for npm trust: npm@^11.10, resolves to 11.21.0; needs npm 11.10 or newer; npm 12 only on Node.js ^22.22.2 || ^24.15.0 || >=26.0.0",
            "    - npm two-factor authentication: off; needs enabled at https://docs.npmjs.com/configuring-two-factor-authentication/; npm trust requires it [action needed]",
            "    - GitHub CLI: signed in as octo (scopes: gist); needs signed in with the repo scope, for gh secret and gh run [action needed]",
            "    - Browser: none; URLs are printed (--no-browser); needs signed in to the registry, or ready to sign in",
        ]
    );
    let mut empty = plan.clone();
    empty.steps.clear();
    assert!(plan_prerequisites(&empty, Some(&environment), &browser).is_empty());
    assert!(plan_prerequisites(&plan, None, &browser).is_empty());
}

#[test]
fn describes_where_browser_pages_open() {
    let plan = npm_plan(registry_state(false, false), false);
    let environment = Environment {
        offline: false,
        node: None,
        npm: None,
        trust_npm: "npm@^11.10".to_owned(),
        trust_npm_version: None,
        two_factor: TwoFactor::Unknown,
        github: Github::Missing,
    };
    let describe = |browser: BrowserDisplay| {
        render_prerequisites(&plan_prerequisites(&plan, Some(&environment), &browser))
            .pop()
            .unwrap_or_default()
    };
    assert_eq!(
        describe(BrowserDisplay {
            channel: "chrome".to_owned(),
            ..BrowserDisplay::default()
        }),
        "    - Browser: your default browser; forms open in the automated chrome profile; needs signed in to the registry, or ready to sign in"
    );
    assert_eq!(
        describe(BrowserDisplay {
            channel: "msedge".to_owned(),
            attach: Some(AttachMode::Snapshot {
                profile: Some("Work".to_owned()),
            }),
            ..BrowserDisplay::default()
        }),
        "    - Browser: your default browser; forms open in a temporary snapshot of your edge profile Work; needs signed in to the registry, or ready to sign in"
    );
    assert_eq!(
        describe(BrowserDisplay {
            mode: BrowserSummary::Automated,
            channel: "brave".to_owned(),
            profile: Some("/state/browser-profile".to_owned()),
            import: Some(ImportSource {
                browser: "chrome".to_owned(),
                profile: Some("Default".to_owned()),
            }),
            attach: None,
        }),
        "    - Browser: the automated brave profile at /state/browser-profile with data imported from chrome:Default; needs signed in to the registry, or ready to sign in"
    );
    assert_eq!(
        describe(BrowserDisplay {
            mode: BrowserSummary::Automated,
            attach: Some(AttachMode::Extension),
            ..BrowserDisplay::default()
        }),
        "    - Browser: your own browser through the Browser Commander extension; needs signed in to the registry, or ready to sign in"
    );
}
