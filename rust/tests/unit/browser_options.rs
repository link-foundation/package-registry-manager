use std::path::{Path, PathBuf};

use browser_commander::browser::migration::MigrationSource;
use package_registry_manager::automation::{
    extension_instructions, launch_options, relay_extension_directory, snapshot_options,
};
use package_registry_manager::browser_catalogue::installed_description;
use package_registry_manager::browser_options::{
    automated_description, parse_browser_options, restriction_names, snapshot_browser, AttachMode,
    BrowserArgs, BrowserOptions, ImportSource,
};
use package_registry_manager::profile::default_browser_profile;
use package_registry_manager::sign_in_import::import_sources;
use serde_json::json;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn profile() -> PathBuf {
    std::path::absolute("automation-profile").expect("absolute profile")
}

#[test]
fn keeps_a_fresh_dedicated_profile_by_default() {
    let browser = parse_browser_options(&BrowserArgs::default()).expect("defaults");
    assert_eq!(browser, BrowserOptions::default());
    assert_eq!(browser.preferences, json!({}));
    let options = launch_options(&browser, &profile(), false, &[]);
    assert_eq!(options.channel, "chrome");
    assert!(!options.headless);
    assert!(!options.verbose);
    assert_eq!(options.user_data_dir, Some(profile()));
    assert_eq!(options.executable_path, None);
    assert_eq!(options.restrictions, [] as [String; 0]);
    assert_eq!(options.preferences, json!({}));
    assert_eq!(options.migrate_from, None);
    assert!(snapshot_options(&browser).is_none());
}

#[test]
fn passes_the_executable_preferences_and_restrictions_to_the_launch() {
    let browser = parse_browser_options(&BrowserArgs {
        channel: "msedge".to_owned(),
        executable: Some(PathBuf::from("bin/edge")),
        preferences: strings(&[
            "intl.accept_languages=en-US",
            "browser.show_home_button=true",
            "session.restore_on_startup=4",
            r#"download.default_directory="/tmp/x=y""#,
        ]),
        restrictions: strings(&["no-extensions", "legacy-defaults", "no-extensions"]),
        ..BrowserArgs::default()
    })
    .expect("valid options");
    let options = launch_options(&browser, &profile(), true, &[]);
    assert_eq!(options.channel, "msedge");
    assert!(options.verbose);
    assert_eq!(
        options.executable_path,
        Some(std::path::absolute("bin/edge").expect("absolute executable"))
    );
    assert_eq!(options.restrictions, ["no-extensions", "legacy-defaults"]);
    assert_eq!(
        options.preferences,
        json!({
            "intl": { "accept_languages": "en-US" },
            "browser": { "show_home_button": true },
            "session": { "restore_on_startup": 4 },
            "download": { "default_directory": "/tmp/x=y" },
        })
    );
    assert_eq!(options.user_data_dir, Some(profile()));
}

#[test]
fn imports_a_real_profile_into_the_dedicated_profile_on_request() {
    for (spec, browser, name) in [
        ("chrome", "chrome", None),
        ("edge:Profile 1", "edge", Some("Profile 1")),
        ("brave:Default", "brave", Some("Default")),
        (
            "firefox:abc.default-release",
            "firefox",
            Some("abc.default-release"),
        ),
    ] {
        let options = launch_options(
            &parse_browser_options(&BrowserArgs {
                import: Some(spec.to_owned()),
                ..BrowserArgs::default()
            })
            .expect(spec),
            &profile(),
            false,
            &[],
        );
        let mut expected = MigrationSource::new(browser);
        expected.profile = name.map(str::to_owned);
        assert_eq!(options.migrate_from, Some(expected), "{spec}");
        assert_eq!(options.user_data_dir, Some(profile()), "{spec}");
    }
}

#[test]
fn attaches_to_a_snapshot_of_the_users_own_profile() {
    for (channel, attach, browser, name) in [
        ("chrome", "snapshot", "chrome", "Default"),
        (
            "msedge-beta",
            "snapshot:Profile 2",
            "edge-beta",
            "Profile 2",
        ),
        ("brave", "snapshot", "brave", "Default"),
    ] {
        let parsed = parse_browser_options(&BrowserArgs {
            channel: channel.to_owned(),
            attach: Some(attach.to_owned()),
            ..BrowserArgs::default()
        })
        .expect(attach);
        let snapshot = snapshot_options(&parsed).expect("snapshot");
        assert_eq!(snapshot.browser, browser);
        assert_eq!(snapshot.profile, name);
        assert_eq!(snapshot.user_data_dir, None);
        let options = launch_options(&parsed, &profile(), false, &[]);
        assert_eq!(options.user_data_dir, None, "a snapshot is temporary");
    }
    assert_eq!(snapshot_browser("chromium"), "chromium");
    assert_eq!(snapshot_browser("msedge-canary"), "edge-canary");
}

#[test]
fn rejects_invalid_or_conflicting_browser_options() {
    let import_error = format!(
        "--browser-import must be <{}>[:profile], default, or auto. {}",
        import_sources().join("|"),
        installed_description()
    );
    let cases: Vec<(BrowserArgs, &str)> = vec![
        (
            BrowserArgs {
                import: Some("netscape".to_owned()),
                ..BrowserArgs::default()
            },
            &import_error,
        ),
        (
            BrowserArgs {
                import: Some("chrome:".to_owned()),
                ..BrowserArgs::default()
            },
            &import_error,
        ),
        (
            BrowserArgs {
                attach: Some("remote".to_owned()),
                ..BrowserArgs::default()
            },
            "--browser-attach must be 'snapshot', 'snapshot:<profile>', or 'extension'",
        ),
        (
            BrowserArgs {
                attach: Some("snapshot:".to_owned()),
                ..BrowserArgs::default()
            },
            "--browser-attach must be 'snapshot', 'snapshot:<profile>', or 'extension'",
        ),
        (
            BrowserArgs {
                preferences: strings(&["novalue"]),
                ..BrowserArgs::default()
            },
            "--browser-pref must be key=value, got 'novalue'",
        ),
        (
            BrowserArgs {
                preferences: strings(&["a..b=1"]),
                ..BrowserArgs::default()
            },
            "--browser-pref must be key=value, got 'a..b=1'",
        ),
        (
            BrowserArgs {
                preferences: strings(&["=1"]),
                ..BrowserArgs::default()
            },
            "--browser-pref must be key=value, got '=1'",
        ),
        (
            BrowserArgs {
                attach: Some("snapshot".to_owned()),
                import: Some("chrome".to_owned()),
                ..BrowserArgs::default()
            },
            "--browser-attach cannot be combined with --browser-import",
        ),
        (
            BrowserArgs {
                attach: Some("snapshot".to_owned()),
                profile_given: true,
                ..BrowserArgs::default()
            },
            "--browser-attach cannot be combined with --browser-profile",
        ),
        (
            BrowserArgs {
                attach: Some("extension".to_owned()),
                executable: Some(PathBuf::from("chrome")),
                ..BrowserArgs::default()
            },
            "--browser-attach extension cannot be combined with --browser-executable, --browser-pref, or --browser-restriction",
        ),
        (
            BrowserArgs {
                attach: Some("extension".to_owned()),
                restrictions: strings(&["no-sync"]),
                ..BrowserArgs::default()
            },
            "--browser-attach extension cannot be combined with --browser-executable, --browser-pref, or --browser-restriction",
        ),
        (
            BrowserArgs {
                attach: Some("extension".to_owned()),
                preferences: strings(&["a=1"]),
                ..BrowserArgs::default()
            },
            "--browser-attach extension cannot be combined with --browser-executable, --browser-pref, or --browser-restriction",
        ),
    ];
    for (args, message) in cases {
        let error = parse_browser_options(&args).expect_err(message);
        assert_eq!(error.to_string(), message);
    }
    let error = parse_browser_options(&BrowserArgs {
        restrictions: strings(&["nope"]),
        ..BrowserArgs::default()
    })
    .expect_err("unknown restriction");
    assert_eq!(
        error.to_string(),
        format!(
            "unknown --browser-restriction 'nope'; choose from {}",
            restriction_names().join(", ")
        )
    );
}

#[test]
fn accepts_every_restriction_and_preset_of_the_shared_catalogue() {
    let names = restriction_names();
    assert!(names.iter().any(|name| name == "no-extensions"));
    assert!(names.iter().any(|name| name == "legacy-defaults"));
    let parsed = parse_browser_options(&BrowserArgs {
        restrictions: names.clone(),
        ..BrowserArgs::default()
    })
    .expect("catalogue");
    assert_eq!(parsed.restrictions, names);
}

#[test]
fn later_preferences_override_earlier_ones() {
    let parsed = parse_browser_options(&BrowserArgs {
        preferences: strings(&["a=1", "a.b=2", "a.c=[1]", "d=null", "e=text", "f=1e999"]),
        ..BrowserArgs::default()
    })
    .expect("preferences");
    assert_eq!(
        parsed.preferences,
        json!({ "a": { "b": 2, "c": [1] }, "d": null, "e": "text", "f": "1e999" })
    );
}

#[test]
fn describes_where_forms_are_filled() {
    assert_eq!(
        automated_description("chrome", None, None, None),
        "the automated chrome profile"
    );
    assert_eq!(
        automated_description(
            "brave",
            Some("/p"),
            None,
            Some(&ImportSource {
                browser: "firefox".to_owned(),
                profile: None,
            }),
        ),
        "the automated brave profile at /p with data imported from firefox"
    );
    assert_eq!(
        automated_description(
            "chrome",
            None,
            Some(&AttachMode::Snapshot { profile: None }),
            None
        ),
        "a temporary snapshot of your chrome profile Default"
    );
    assert_eq!(
        automated_description("chrome", None, Some(&AttachMode::Extension), None),
        "your own browser through the Browser Commander extension"
    );
}

#[test]
fn writes_the_companion_extension_next_to_the_dedicated_profile() {
    let directory = relay_extension_directory().expect("relay directory");
    let profile = default_browser_profile().expect("profile");
    assert_eq!(directory.parent(), profile.parent());
    let instructions = extension_instructions(&directory);
    assert!(instructions.contains("chrome://extensions"));
    assert!(instructions.contains("Load unpacked"));
    assert!(instructions.ends_with(&format!("  {}", directory.display())));
    assert_eq!(
        extension_instructions(Path::new("/x")),
        "Waiting up to 5 minutes for the Browser Commander extension in your own browser.\n\
         If it is not installed, open chrome://extensions, turn on Developer mode, click \"Load unpacked\", and choose:\n  /x"
    );
}

#[test]
fn firefox_import_defaults_to_sign_in_domains_and_reports_missing_capabilities() {
    use package_registry_manager::browser_options::ImportScope;
    let browser = parse_browser_options(&BrowserArgs {
        channel: "librewolf".to_owned(),
        import: Some("chrome:Profile 1".to_owned()),
        ..BrowserArgs::default()
    })
    .unwrap();
    assert_eq!(browser.import_scope, ImportScope::Domains);
    let options = launch_options(&browser, &profile(), false, &["npmjs.com"]);
    assert_eq!(options.migrate_include, Some(vec!["cookies".to_owned()]));
    assert_eq!(options.migrate_domains, vec!["npmjs.com".to_owned()]);
    for (args, capability) in [
        (
            BrowserArgs {
                attach: Some("snapshot".to_owned()),
                ..BrowserArgs::default()
            },
            "snapshot",
        ),
        (
            BrowserArgs {
                preferences: strings(&["a=1"]),
                ..BrowserArgs::default()
            },
            "preferences",
        ),
        (
            BrowserArgs {
                restrictions: strings(&["no-extensions"]),
                ..BrowserArgs::default()
            },
            "restrictions",
        ),
        (
            BrowserArgs {
                import: Some("chrome".to_owned()),
                import_scope: Some("full".to_owned()),
                ..BrowserArgs::default()
            },
            "full-profile migration",
        ),
    ] {
        let error = parse_browser_options(&BrowserArgs {
            channel: "firefox".to_owned(),
            ..args
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains(capability), "{error}");
    }
}

#[test]
fn firefox_variants_have_separate_default_profiles() {
    use package_registry_manager::profile::default_browser_profile_for_channel;
    let chrome = default_browser_profile().unwrap();
    assert_eq!(
        default_browser_profile_for_channel("msedge").unwrap(),
        chrome
    );
    let firefox = default_browser_profile_for_channel("firefox").unwrap();
    assert_eq!(firefox.parent(), chrome.parent());
    assert_eq!(firefox.file_name().unwrap(), "browser-profile-firefox");
    assert_ne!(
        firefox,
        default_browser_profile_for_channel("librewolf").unwrap()
    );
}
