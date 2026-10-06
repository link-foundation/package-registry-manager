use std::collections::HashMap;
use std::path::Path;

use browser_commander::{browser_sources, find_browser_source};
use package_registry_manager::automation::launch_options;
use package_registry_manager::browser_catalogue::{installed_browsers_with, launch_channels};
use package_registry_manager::browser_options::{
    parse_browser_options, parse_import, snapshot_browser, BrowserArgs,
};
use package_registry_manager::default_browser::browser_name;
use package_registry_manager::sign_in_import::{import_sources, source_id, source_name};

#[test]
fn accepts_and_names_extended_browser_sources() {
    for (id, name, identifier) in [
        ("vivaldi", "Vivaldi", "vivaldi.desktop"),
        ("opera-gx", "Opera GX", "OperaGXStable"),
        ("yandex", "Yandex", "YandexHTML"),
        ("librewolf", "LibreWolf", "librewolf.desktop"),
        ("waterfox", "Waterfox", "waterfox.desktop"),
        ("zen", "Zen", "zen-browser.desktop"),
        ("floorp", "Floorp", "floorp.desktop"),
    ] {
        assert_eq!(parse_import(id).unwrap().browser, id);
        assert_eq!(source_name(id), name);
        assert_eq!(source_id(name), Some(id));
        assert_eq!(browser_name(identifier), Some(name));
    }
    for id in ["vivaldi", "opera", "opera-gx", "yandex", "arc"] {
        assert_eq!(snapshot_browser(id), id);
    }
}

#[test]
fn imports_and_names_every_catalogue_entry_and_alias() {
    assert_eq!(import_sources().len(), browser_sources().len());
    for source in browser_sources() {
        let name = source_name(&source.id);
        assert_eq!(source_id(name), Some(source.id.as_str()), "{name}");
        for id in std::iter::once(&source.id).chain(&source.aliases) {
            assert_eq!(parse_import(id).unwrap().browser, source.id);
            let parsed = parse_import(&format!("{id}:Profile 1")).unwrap();
            assert_eq!(parsed.browser, source.id);
            assert_eq!(parsed.profile.as_deref(), Some("Profile 1"));
        }
        for identifier in source.default.values().flatten() {
            assert_eq!(browser_name(identifier), Some(name), "{identifier}");
            assert_eq!(browser_name(&identifier.to_uppercase()), Some(name));
        }
    }
}

#[test]
fn preserves_legacy_and_packaged_default_browser_identifiers() {
    for (identifier, name) in [
        ("FirefoxURL308046B0AF4A39CB", "Firefox"),
        ("VivaldiHTM.123", "Vivaldi"),
        ("ChromeHTML-hash", "Google Chrome"),
        ("brave-browser_brave.desktop", "Brave"),
        ("SafariURL", "Safari"),
        ("ChromiumHTM.123", "Chromium"),
        ("IE.HTTP", "Internet Explorer"),
    ] {
        assert_eq!(browser_name(identifier), Some(name), "{identifier}");
    }
}

#[test]
fn launches_and_snapshots_every_supported_channel_and_alias() {
    for channel in launch_channels() {
        let options = parse_browser_options(&BrowserArgs {
            channel: channel.to_owned(),
            import: Some("firefox".to_owned()),
            ..BrowserArgs::default()
        })
        .unwrap();
        let launch = launch_options(&options, Path::new("/profile"), false, &[]);
        assert_eq!(launch.channel, channel);
        assert_eq!(launch.migrate_from.unwrap().browser, "firefox");
        assert_eq!(
            snapshot_browser(channel),
            find_browser_source(channel).unwrap().id
        );
    }
    for channel in ["firefox", "librewolf", "safari", "duckduckgo", "netscape"] {
        let error = parse_browser_options(&BrowserArgs {
            channel: channel.to_owned(),
            ..BrowserArgs::default()
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains(if channel == "netscape" {
            "unknown --browser-channel"
        } else {
            "real launcher does not support yet"
        }));
        assert!(error.contains("Installed browsers found:"));
    }
}

#[test]
fn invalid_imports_report_every_id_and_installed_discovery() {
    let error = parse_import("netscape").unwrap_err().to_string();
    for id in import_sources() {
        assert!(error.contains(id), "{id}");
    }
    assert!(error.contains("Installed browsers found:"));
}

#[test]
fn extension_attachment_does_not_require_a_real_launcher_channel() {
    let options = parse_browser_options(&BrowserArgs {
        channel: "firefox".to_owned(),
        attach: Some("extension".to_owned()),
        ..BrowserArgs::default()
    })
    .unwrap();
    assert_eq!(options.channel, "firefox");
}

#[test]
fn discovers_executables_and_profile_roots_on_each_platform() {
    let environment = HashMap::from([
        ("XDG_CONFIG_HOME".to_owned(), "/profiles".to_owned()),
        ("PATH".to_owned(), "/browsers".to_owned()),
    ]);
    assert_eq!(
        installed_browsers_with("linux", "/users/test", &environment, |candidate| {
            [
                Path::new("/profiles/vivaldi"),
                Path::new("/browsers/yandex-browser"),
            ]
            .contains(&candidate)
        }),
        ["vivaldi", "yandex"]
    );
    assert_eq!(
        installed_browsers_with("macos", "/users/test", &HashMap::new(), |candidate| {
            candidate == Path::new("/users/test/Library/Containers/com.apple.Safari/Data/Library")
        }),
        ["safari"]
    );
    assert_eq!(
        installed_browsers_with(
            "windows",
            r"C:\Users\test",
            &HashMap::from([("APPDATA".to_owned(), r"D:\Roaming".to_owned()),]),
            |candidate| { candidate == Path::new(r"D:\Roaming\Opera Software\Opera GX Stable") }
        ),
        ["opera-gx"]
    );
    assert!(installed_browsers_with("linux", "/users/test", &environment, |_| false).is_empty());
    assert_eq!(
        installed_browsers_with(
            "windows",
            r"C:\Users\test",
            &HashMap::from([("PROGRAMFILES".to_owned(), r"D:\Programs".to_owned()),]),
            |candidate| {
                [
                    Path::new(r"D:\Programs\Naver\Naver Whale\Application\whale.exe"),
                    Path::new(r"D:\Programs\360\360se6\360se.exe"),
                    Path::new(r"D:\Programs\Tencent\QQBrowser\QQBrowser.exe"),
                ]
                .contains(&candidate)
            }
        ),
        ["whale", "360se", "qq"]
    );
}
