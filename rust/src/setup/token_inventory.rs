//! Read private token expiry without any credential or publication mutations.
use super::{BrowserMode, ExecuteOptions, Session};
use crate::{
    browser_options::BrowserOptions,
    crates_api::page_fetch_script,
    crates_tokens::{token_inventory, TOKEN_INVENTORY_PATH},
    model::{Package, Registry, RepositoryInfo, SetupPlan, SetupStep, StepKind},
};
use anyhow::Result;
use serde_json::Value;
use std::{path::Path, time::Duration};

/// Open the dedicated browser for a read-only crates.io token inventory.
#[allow(clippy::future_not_send)]
pub async fn read_account_token_inventory(root: &Path) -> Result<Vec<Value>> {
    let browser_options = BrowserOptions::default();
    let profile = crate::profile::default_browser_profile()?;
    let options = ExecuteOptions {
        repository: root,
        browser: BrowserMode::Automated,
        browser_profile: &profile,
        browser_options: &browser_options,
        execute: true,
        yes: false,
        no_browser: false,
        open_with: None,
        keep_session: true,
        delete_token_secrets: false,
        verbose: false,
        secret_name: None,
        quiet_browser: true,
        repository_batch: false,
        secret_repositories: None,
        endpoints: crate::registry_state::Endpoints::from_env(),
        poll_interval: Duration::from_secs(2),
        wait_timeout: Duration::from_secs(600),
    };
    let plan = SetupPlan {
        schema_version: 1,
        registry: Registry::CratesIo,
        credential_policy: crate::credential_cycle::credential_policy(Registry::CratesIo),
        package: Package::new(
            Registry::CratesIo,
            "token-inventory".into(),
            None,
            "Cargo.toml".into(),
        ),
        repository: RepositoryInfo {
            root: root.display().to_string(),
            github_owner: None,
            github_repository: None,
            release_workflow: None,
            pages_workflow: None,
        },
        steps: Vec::new(),
        mode: None,
        prerequisites: Vec::new(),
        oidc_publisher: None,
        skipped_reason: None,
    };
    let mut session = Session::new(&plan, &options);
    let step = SetupStep::new(
        "crates-sign-in",
        "Sign in to crates.io",
        StepKind::Api,
        "Read token metadata",
    );
    let result = async {
        session.api_step(&step).await?;
        token_inventory(
            &session
                .automated()
                .await?
                .evaluate(&page_fetch_script("GET", TOKEN_INVENTORY_PATH, None))
                .await?,
        )
    }
    .await;
    if let Some(browser) = session.browser.take() {
        browser.close().await;
    }
    result
}
