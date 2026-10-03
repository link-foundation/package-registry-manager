use chrono::NaiveTime;
use package_registry_manager::approvals::{
    approval_deadline, expired_approval, next_link, oidc_release_note, LinkKind, APPROVAL_ATTEMPTS,
};
use package_registry_manager::auth_urls::CommandOutput;
use package_registry_manager::default_browser::{
    browser_from_query, browser_name, default_browser_id, default_browser_query, open_with_command,
};

const fn output(code: i32, legacy_login: bool, approval_expired: bool) -> CommandOutput {
    CommandOutput {
        code,
        stdout: String::new(),
        stderr: String::new(),
        legacy_login,
        approval_expired,
    }
}

#[test]
fn prints_when_a_sign_in_or_approval_link_expires() {
    let now = NaiveTime::from_hms_opt(18, 58, 0).expect("time");
    assert_eq!(
        approval_deadline(LinkKind::Login, now),
        "Sign in within about 5 minutes (until 19:03)."
    );
    assert_eq!(
        approval_deadline(LinkKind::Approve, now),
        "Approve within about 5 minutes (until 19:03)."
    );
    let late = NaiveTime::from_hms_opt(23, 57, 0).expect("time");
    assert_eq!(
        approval_deadline(LinkKind::Login, late),
        "Sign in within about 5 minutes (until 00:02)."
    );
}

#[test]
fn recognizes_the_ways_an_npm_link_expires() {
    assert_eq!(
        expired_approval(&output(1, true, false)),
        Some("npm fell back to its legacy username prompt")
    );
    assert_eq!(
        expired_approval(&output(1, false, true)),
        Some("npm's approval session ended")
    );
    assert_eq!(expired_approval(&output(0, false, true)), None);
    assert_eq!(expired_approval(&output(1, false, false)), None);
}

#[test]
fn requests_fresh_links_a_bounded_number_of_times() {
    let expired = output(1, true, false);
    assert!(next_link(&output(0, false, false), 1, APPROVAL_ATTEMPTS)
        .expect("decision")
        .is_none());
    let message = next_link(&expired, 1, APPROVAL_ATTEMPTS)
        .expect("decision")
        .expect("fresh link");
    assert!(
        message.contains("requesting a fresh one (attempt 2 of 3)"),
        "{message}"
    );
    let error = next_link(
        &output(1, false, true),
        APPROVAL_ATTEMPTS,
        APPROVAL_ATTEMPTS,
    )
    .expect_err("gives up");
    assert!(
        error
            .to_string()
            .contains("the browser link expired 3 times (npm's approval session ended)"),
        "{error}"
    );
    assert_eq!(
        oidc_release_note("release.yml"),
        "Future releases publish from release.yml through trusted publishing; no login is needed."
    );
}

#[test]
fn names_the_default_browser_on_every_platform() {
    let mac = r#"(
    {
        LSHandlerContentType = "public.html";
        LSHandlerRoleAll = "com.google.chrome";
    },
    {
        LSHandlerPreferredVersions = { LSHandlerRoleAll = "-"; };
        LSHandlerRoleAll = "com.brave.browser";
        LSHandlerURLScheme = https;
    }
)"#;
    assert_eq!(
        default_browser_id(mac, "macos").as_deref(),
        Some("com.brave.browser")
    );
    assert_eq!(
        default_browser_id("(\n)", "macos").as_deref(),
        Some("com.apple.safari")
    );
    assert_eq!(
        default_browser_id(
            "\r\nHKEY_CURRENT_USER\\...\\UserChoice\r\n    ProgId    REG_SZ    MSEdgeHTM\r\n",
            "windows"
        )
        .as_deref(),
        Some("MSEdgeHTM")
    );
    assert_eq!(
        default_browser_id("firefox.desktop\n", "linux").as_deref(),
        Some("firefox.desktop")
    );
    assert_eq!(default_browser_id("", "linux"), None);
    assert_eq!(browser_name("com.brave.browser"), Some("Brave"));
    assert_eq!(browser_name("MSEdgeHTM"), Some("Microsoft Edge"));
    assert_eq!(browser_name("google-chrome.desktop"), Some("Google Chrome"));
    assert_eq!(browser_name("unknown.desktop"), None);
    let linux = default_browser_query("linux");
    assert_eq!(linux.program, "xdg-settings");
    assert_eq!(linux.args, ["get", "default-web-browser"]);
    assert_eq!(default_browser_query("macos").program, "defaults");
    assert_eq!(default_browser_query("windows").program, "reg");
    assert_eq!(browser_from_query(1, "", "macos"), Some("Safari"));
    assert_eq!(browser_from_query(1, "firefox.desktop", "linux"), None);
    assert_eq!(
        browser_from_query(0, "firefox.desktop\n", "linux"),
        Some("Firefox")
    );
}

#[test]
fn opens_a_link_in_a_chosen_application_without_a_shell() {
    let url = "https://www.npmjs.com/auth/cli/2";
    let mac = open_with_command(url, "Firefox", "macos").expect("command");
    assert_eq!(mac.program, "open");
    assert_eq!(mac.args, ["-a", "Firefox", url]);
    let linux = open_with_command(url, "firefox", "linux").expect("command");
    assert_eq!(linux.program, "firefox");
    assert_eq!(linux.args, [url]);
    let error = open_with_command("file:///etc/passwd", "firefox", "linux").expect_err("refused");
    assert!(error.to_string().contains("non-web URL"), "{error}");
}
