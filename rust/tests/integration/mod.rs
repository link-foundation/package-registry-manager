mod browser_parity;
mod cli;
mod firefox_forms;
mod mock_registry;
mod npm_bootstrap;
#[cfg(unix)]
mod npm_bootstrap_e2e;
#[cfg(unix)]
mod npm_session_e2e;
mod organization_scan;
mod repository_transfer;
#[cfg(unix)]
mod repository_transfer_e2e;

mod npm_similarity;
mod publisher_forms;

mod scan_reliability;
