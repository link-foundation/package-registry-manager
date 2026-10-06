use anyhow::Result;
use package_registry_manager::automation::Automation;
use package_registry_manager::browser::npm_prefill_script;
use package_registry_manager::browser_options::{parse_browser_options, BrowserArgs};
use package_registry_manager::crates_api::page_fetch_script;
use package_registry_manager::model::TrustedPublisherPrefill;
use serde_json::json;

/// Local fixture only: no account, registry write, or personal profile.
#[tokio::test]
async fn firefox_forms_and_async_page_scripts() -> Result<()> {
    if std::env::var("PRM_FIREFOX_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipped: set PRM_FIREFOX_SMOKE=1 with Firefox and geckodriver installed");
        return Ok(());
    }
    let profile = tempfile::tempdir()?;
    let browser = parse_browser_options(&BrowserArgs {
        channel: "firefox".to_owned(),
        executable: std::env::var_os("PRM_FIREFOX_EXECUTABLE").map(Into::into),
        ..BrowserArgs::default()
    })?;
    let mut automation = Automation::connect(&browser, profile.path(), true, &[]).await?;
    let checked = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        if let Automation::WebDriver(driver) = &automation {
            driver.restore_state(browser_commander::StorageState {
                cookies: vec![
                    json!({ "name": "session", "value": "synthetic", "domain": ".npmjs.com", "path": "/", "httpOnly": true, "secure": true, "expires": -1 }),
                    json!({ "name": "unrelated", "value": "synthetic", "domain": "example.test", "path": "/", "expires": -1 }),
                    json!({ "name": "lookalike", "value": "synthetic", "domain": "evilnpmjs.com", "path": "/", "expires": -1 }),
                ], origins: Vec::new(),
            }).await?;
        }
        automation.goto("data:text/html,<label>Organization<input name='organization'></label><label>Repository<input name='repository'></label><label>Workflow<input name='workflow'></label>").await?;
        let publisher = TrustedPublisherPrefill {
            provider: "github".to_owned(), organization: "owner".to_owned(),
            repository: "repo".to_owned(), workflow: "release.yml".to_owned(),
            environment: None, project: None,
        };
        let form = automation.evaluate(&npm_prefill_script(&publisher, false)?).await?;
        assert_eq!(form["filled"].as_array().unwrap().len(), 3);
        assert_eq!(automation.evaluate(&page_fetch_script("GET", "data:application/json,%7B%22login%22%3A%22fixture%22%7D", None)).await?,
            json!({ "status": 200, "body": { "login": "fixture" } }));
        assert!(automation.clear_cookies(&["npmjs.com"]).await?);
        if let Automation::WebDriver(driver) = &automation {
            let state = driver.bidi().unwrap().send("storage.getCookies", json!({})).await?;
            let cookies = state["cookies"].as_array().unwrap();
            assert!(!cookies.iter().any(|cookie| cookie["name"] == "session"));
            assert!(cookies.iter().any(|cookie| cookie["name"] == "unrelated"));
            assert!(cookies.iter().any(|cookie| cookie["name"] == "lookalike"));
        }
        anyhow::Ok(())
    }).await;
    automation.close().await;
    checked?
}
