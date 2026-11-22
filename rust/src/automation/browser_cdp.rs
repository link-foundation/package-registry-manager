//! Locate the running browser by the endpoint recorded in its dedicated profile.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use browser_commander::browser::cdp_endpoint::fetch_cdp_version;
use browser_commander::{connect_browser, ConnectOptions};
use reqwest::{redirect::Policy, Client, Url};
use serde::Deserialize;

use super::Automation;

const ENDPOINT_FILE: &str = ".prm-cdp-endpoint";
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);

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
    None
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
    let npm_tabs = npm_tabs(&client, port).await?;
    let target_id = npm_tabs.first().map(|tab| tab.id.clone());
    let connection = connect_browser(ConnectOptions {
        ws_endpoint: Some(socket),
        target_id,
        fallback: true,
        verbose,
        timeout: Some(Duration::from_secs(10)),
        ..ConnectOptions::default()
    })
    .await
    .context("could not attach to the running browser")?;
    let selected = connection.page.target_id().await?;
    ensure!(
        selected.is_some(),
        "the attached browser did not report a working tab"
    );
    close_duplicates(&client, port, npm_tabs, selected.as_deref()).await;
    let _ = connection.page.bring_to_front().await;
    Ok(Some(Automation::Attached(connection.page)))
}

pub(super) async fn clean_launched(
    port: u16,
    page: &dyn browser_commander::EngineAdapter,
) -> Result<()> {
    let client = cdp_client()?;
    let selected = page.target_id().await?;
    if let Some(selected) = selected {
        let tabs = npm_tabs(&client, port).await?;
        close_duplicates(&client, port, tabs, Some(&selected)).await;
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

async fn npm_tabs(client: &Client, port: u16) -> Result<Vec<Tab>> {
    let response = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await?
        .error_for_status()?;
    let tabs: Vec<Tab> = serde_json::from_slice(&response.bytes().await?)?;
    Ok(tabs
        .into_iter()
        .filter(|tab| tab.kind == "page" && npm_page(&tab.url))
        .collect())
}

async fn close_duplicates(client: &Client, port: u16, tabs: Vec<Tab>, selected: Option<&str>) {
    for tab in tabs {
        if Some(tab.id.as_str()) != selected
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
