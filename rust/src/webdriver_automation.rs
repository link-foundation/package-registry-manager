//! Firefox-family form filling through Browser Commander's native `WebDriver` API.

use std::collections::HashSet;
use std::future::Future;

use anyhow::{bail, Context, Result};
use browser_commander::browser::real_browser::RealBrowserOptions;
use browser_commander::browser::system_browser::resolve_browser_executable;
use browser_commander::{
    launch_webdriver, read_browser_cookies, BrowserCookieReadOptions, ManagedWebDriver,
    StorageState, WebDriverBrowser, WebDriverOptions,
};
use serde_json::{json, Value};

/// True for a registry host and its subdomains, never a substring.
fn matches_domains(host: &str, domains: &[&str]) -> bool {
    let host = host.trim_start_matches('.').to_ascii_lowercase();
    domains
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

async fn launch_with<T, F, Fut>(options: &RealBrowserOptions, launcher: F) -> Result<T>
where
    F: FnOnce(WebDriverOptions) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    if !options.restrictions.is_empty()
        || options
            .preferences
            .as_object()
            .is_some_and(|prefs| !prefs.is_empty())
        || options.migrate_from.is_some()
            && !options
                .migrate_include
                .as_ref()
                .is_some_and(|include| include.len() == 1 && include[0] == "cookies")
    {
        bail!("unsupported Firefox launch options; use --browser-import-scope domains without preferences or restrictions");
    }
    let executable = if let Some(path) = &options.executable_path {
        path.clone()
    } else {
        resolve_browser_executable(&options.channel, None)?
    };
    if options.verbose {
        eprintln!(
            "launching {} with WebDriver BiDi ({})",
            options.channel,
            executable.display()
        );
    }
    launcher(WebDriverOptions {
        browser: WebDriverBrowser::Firefox,
        browser_executable: Some(executable),
        user_data_dir: options.user_data_dir.clone(),
        headless: options.headless,
        bidi: true,
        ..WebDriverOptions::default()
    })
    .await
}

pub async fn connect(options: RealBrowserOptions, domains: &[&str]) -> Result<ManagedWebDriver> {
    if options.migrate_from.is_some() && domains.is_empty() {
        bail!("a domains import needs the registry's sign-in domains");
    }
    let driver = launch_with(&options, launch_webdriver).await?;
    let initialized = async {
        if driver.bidi().is_none() {
            bail!("Firefox sign-in import and cleanup require WebDriver BiDi storage; update Firefox and geckodriver");
        }
        driver.bidi().expect("BiDi was checked").send("storage.getCookies", json!({})).await
            .context("Firefox lacks BiDi cookie storage; update Firefox and geckodriver")?;
        if let Some(source) = options.migrate_from {
            let filters = domains.iter().map(|domain| (*domain).to_owned()).collect::<Vec<_>>();
            let cookies = tokio::task::spawn_blocking(move || {
                let mut found = Vec::new();
                for domain in filters {
                    let mut read = BrowserCookieReadOptions::new(&source.browser);
                    read.profile.clone_from(&source.profile);
                    read.domain_filter = Some(domain);
                    read.cache = false;
                    found.extend(read_browser_cookies(read)?);
                }
                anyhow::Ok(found)
            }).await??;
            let mut seen = HashSet::new();
            let cookies = cookies.into_iter().filter(|cookie| {
                matches_domains(&cookie.domain, domains) &&
                    seen.insert((cookie.domain.clone(), cookie.path.clone(), cookie.name.clone()))
            }).map(serde_json::to_value).collect::<std::result::Result<Vec<Value>, _>>()?;
            driver.restore_state(StorageState { cookies, origins: Vec::new() }).await?;
        }
        anyhow::Ok(())
    }.await;
    if let Err(error) = initialized {
        let _ = driver.close().await;
        return Err(error).context("could not prepare Firefox sign-in cookies");
    }
    Ok(driver)
}

pub async fn clear_cookies(driver: &ManagedWebDriver, domains: &[&str]) -> Result<()> {
    let bidi = driver
        .bidi()
        .context("Firefox cookie cleanup requires WebDriver BiDi storage")?;
    let result = bidi.send("storage.getCookies", json!({})).await?;
    for cookie in result["cookies"]
        .as_array()
        .context("invalid BiDi cookie response")?
    {
        if matches_domains(cookie["domain"].as_str().unwrap_or_default(), domains) {
            bidi.send(
                "storage.deleteCookies",
                json!({ "filter": {
                    "name": cookie["name"], "domain": cookie["domain"], "path": cookie["path"],
                }}),
            )
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser_commander::EngineType;
    use std::path::PathBuf;

    #[tokio::test]
    async fn bidi_channel_uses_stubbed_native_launcher() {
        let options = RealBrowserOptions {
            engine: EngineType::Fantoccini,
            channel: "librewolf".to_owned(),
            executable_path: Some(PathBuf::from("stub-librewolf")),
            user_data_dir: Some(PathBuf::from("dedicated-profile")),
            ..RealBrowserOptions::default()
        };
        launch_with(&options, |request| async move {
            assert_eq!(request.browser, WebDriverBrowser::Firefox);
            assert_eq!(
                request.browser_executable,
                Some(PathBuf::from("stub-librewolf"))
            );
            assert_eq!(
                request.user_data_dir,
                Some(PathBuf::from("dedicated-profile"))
            );
            assert!(request.bidi);
            assert!(!request.headless);
            Ok(())
        })
        .await
        .unwrap();
    }

    #[test]
    fn sign_in_domains_do_not_match_lookalikes() {
        assert!(matches_domains(".npmjs.com", &["npmjs.com"]));
        assert!(matches_domains("login.npmjs.com", &["npmjs.com"]));
        assert!(!matches_domains("evilnpmjs.com", &["npmjs.com"]));
        assert!(!matches_domains("npmjs.com.evil.test", &["npmjs.com"]));
    }
}
