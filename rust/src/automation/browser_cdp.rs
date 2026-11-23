//! Locate the running browser by the endpoint recorded in its dedicated profile.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use browser_commander::browser::cdp_endpoint::fetch_cdp_version;
use chromiumoxide::cdp::browser_protocol::network::DeleteCookiesParams;
use chromiumoxide::cdp::browser_protocol::target::TargetId;
use chromiumoxide::error::CdpError;
use chromiumoxide::{Browser, Page};
use futures_util::StreamExt;
use reqwest::{redirect::Policy, Client, Url};
use serde::Deserialize;
use tokio::task::JoinHandle;

use super::{cookie_domain_matches, Automation};

mod process;

const ENDPOINT_FILE: &str = ".prm-cdp-endpoint";
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// A page in a dedicated Chromium profile owned by another process.
pub struct AttachedBrowser {
    pub(super) page: Page,
    browser: Browser,
    _handler: ConnectionHandler,
}

// Dropping the connection must leave the running browser and its tabs open.
struct ConnectionHandler(JoinHandle<()>);

impl Drop for ConnectionHandler {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl AttachedBrowser {
    async fn connect(socket: String, tab: Option<&Tab>) -> Result<Self> {
        let (mut browser, mut events) = Browser::connect(socket).await?;
        let handler = ConnectionHandler(tokio::spawn(async move {
            while events.next().await.is_some() {}
        }));
        // Browser Commander 0.22.1 skips existing target discovery when it
        // connects through Chromiumoxide, so it opens a new tab each time.
        browser.fetch_targets().await?;
        let page = if let Some(tab) = tab {
            let id = TargetId::new(&tab.id);
            loop {
                match browser.get_page(id.clone()).await {
                    Ok(page) => break page,
                    Err(CdpError::NotFound) => tokio::time::sleep(Duration::from_millis(20)).await,
                    Err(error) => return Err(error.into()),
                }
            }
        } else {
            browser.new_page("about:blank").await?
        };
        Ok(Self {
            page,
            browser,
            _handler: handler,
        })
    }

    pub(super) async fn clear_cookies(&self, domains: &[&str]) -> Result<()> {
        let cookies = self
            .browser
            .get_cookies()
            .await?
            .into_iter()
            .filter(|cookie| cookie_domain_matches(&cookie.domain, domains))
            .map(|cookie| DeleteCookiesParams {
                name: cookie.name,
                url: None,
                domain: Some(cookie.domain),
                path: Some(cookie.path),
                partition_key: cookie.partition_key,
            })
            .collect();
        self.page.delete_cookies(cookies).await?;
        Ok(())
    }
}

pub(super) fn configured_port() -> Result<Option<u16>> {
    std::env::var_os("PRM_BROWSER_CDP_PORT")
        .map(|value| {
            let value = value.to_str().unwrap_or_default();
            let port = value.parse::<u16>().ok().filter(|port| *port > 0);
            port.filter(|_| value.bytes().all(|byte| byte.is_ascii_digit()))
                .context("PRM_BROWSER_CDP_PORT must be an integer from 1 to 65535")
        })
        .transpose()
}

fn socket_port(socket: &str) -> Option<u16> {
    let (port, id) = socket
        .strip_prefix("ws://127.0.0.1:")?
        .split_once("/devtools/browser/")?;
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte)))
    .then(|| port.parse::<u16>().ok().filter(|port| *port != 0))
    .flatten()
}

async fn browser_socket(port: u16) -> Option<String> {
    fetch_cdp_version(port, PROBE_TIMEOUT)
        .await
        .filter(|socket| socket_port(socket) == Some(port))
}

pub(super) async fn remember_endpoint(profile: &Path, port: u16) -> Result<()> {
    if let Some(socket) = browser_socket(port).await {
        let mut file = tempfile::NamedTempFile::new_in(profile)?;
        writeln!(file, "{socket}")?;
        file.persist(profile.join(ENDPOINT_FILE))?;
    }
    Ok(())
}

async fn running_endpoint(profile: &Path, port: Option<u16>) -> Option<(u16, String)> {
    if let Some(port) = port {
        return Some((port, browser_socket(port).await?));
    }
    let recorded = std::fs::read_to_string(profile.join(ENDPOINT_FILE)).unwrap_or_default();
    let active = std::fs::read_to_string(profile.join("DevToolsActivePort")).unwrap_or_default();
    let mut lines = active.lines();
    let chrome = match (lines.next(), lines.next()) {
        (Some(port), Some(target)) => format!("ws://127.0.0.1:{port}{target}"),
        _ => String::new(),
    };
    for endpoint in [recorded.trim(), &chrome] {
        if let Some(port) = socket_port(endpoint) {
            if let Some(socket) = browser_socket(port)
                .await
                .filter(|socket| socket == endpoint)
            {
                return Some((port, socket));
            }
        }
    }
    // A browser launched with a fixed port may predate the recorded endpoint.
    let port = process::debugging_port(profile).await?;
    Some((port, browser_socket(port).await?))
}

#[derive(Deserialize)]
struct Tab {
    id: String,
    url: String,
    #[serde(rename = "type")]
    kind: String,
}

fn npm_page(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| {
        url.scheme() == "https"
            && url
                .host_str()
                .is_some_and(|host| host == "npmjs.com" || host.ends_with(".npmjs.com"))
    })
}

pub(super) async fn attach(
    profile: &Path,
    port: Option<u16>,
    verbose: bool,
) -> Result<Option<Automation>> {
    let Some((port, socket)) = running_endpoint(profile, port).await else {
        return Ok(None);
    };
    let client = cdp_client()?;
    let tabs = page_tabs(&client, port).await?;
    let target = tabs
        .iter()
        .find(|tab| npm_page(&tab.url))
        .or_else(|| tabs.first());
    let connection = tokio::time::timeout(ATTACH_TIMEOUT, AttachedBrowser::connect(socket, target))
        .await
        .context("timed out attaching to the running browser")?
        .context("could not attach to the running browser")?;
    close_duplicates(&client, port, tabs, connection.page.target_id().as_ref()).await;
    let _ = connection.page.bring_to_front().await;
    if verbose {
        eprintln!("Attached to the running browser for the dedicated profile");
    }
    Ok(Some(Automation::Attached(Box::new(connection))))
}

pub(super) async fn clean_launched(
    port: u16,
    page: &dyn browser_commander::EngineAdapter,
) -> Result<()> {
    let client = cdp_client()?;
    let selected = page.target_id().await?;
    if let Some(selected) = selected {
        let tabs = page_tabs(&client, port).await?;
        close_duplicates(&client, port, tabs, &selected).await;
    }
    Ok(())
}

fn cdp_client() -> Result<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .timeout(PROBE_TIMEOUT)
        .build()?)
}

async fn page_tabs(client: &Client, port: u16) -> Result<Vec<Tab>> {
    let response = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await?
        .error_for_status()?;
    let tabs: Vec<Tab> = serde_json::from_slice(&response.bytes().await?)?;
    Ok(tabs.into_iter().filter(|tab| tab.kind == "page").collect())
}

async fn close_duplicates(client: &Client, port: u16, tabs: Vec<Tab>, selected: &str) {
    for tab in tabs {
        if tab.id != selected
            && npm_page(&tab.url)
            && tab.id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            // The tab may have closed while the browser was attaching.
            let _ = client
                .get(format!("http://127.0.0.1:{port}/json/close/{}", tab.id))
                .send()
                .await;
        }
    }
}
