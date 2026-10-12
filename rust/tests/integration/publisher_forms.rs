//! Verify the Rust-generated scripts against the current Warehouse-shaped DOM.
use anyhow::Result;
use package_registry_manager::automation::Automation;
use package_registry_manager::browser_options::BrowserOptions;
use package_registry_manager::model::TrustedPublisherPrefill;
use package_registry_manager::publisher_form::{confirmation_script, submit_form, wait_for_form};
use package_registry_manager::registry_state::encode_uri_component;
use std::time::Duration;

#[tokio::test]
async fn browser_publisher_forms_fill_submit_and_verify_github_identity() -> Result<()> {
    if std::env::var("PRM_BROWSER_SMOKE").as_deref() != Ok("1") {
        return Ok(());
    }
    let profile = tempfile::tempdir()?;
    let mut page =
        Automation::connect(&BrowserOptions::default(), profile.path(), true, &[]).await?;
    let result = async {
        let html = include_str!("../../../experiments/issue-54/publisher.html");
        page.goto(&format!("data:text/html,{}", encode_uri_component(html)))
            .await?;
        let mut config = TrustedPublisherPrefill {
            provider: "github".into(),
            organization: "acme".into(),
            repository: "example".into(),
            workflow: "release.yml".into(),
            environment: None,
            project: Some("prm-example".into()),
        };
        wait_for_form(
            &page,
            &config,
            true,
            Duration::from_millis(10),
            Duration::from_secs(5),
        )
        .await?;
        submit_form(
            &page,
            &config,
            true,
            true,
            Duration::from_millis(10),
            Duration::from_secs(5),
        )
        .await?;
        assert_eq!(
            page.evaluate("document.querySelector('#google_project').value")
                .await?,
            ""
        );
        config.organization = "wrong-owner".into();
        assert_eq!(
            page.evaluate(&confirmation_script(&config, true)?).await?["confirmed"],
            false
        );
        // Existing projects have no project_name field and no project column.
        let existing = html
            .replace("pending-github-publisher-form", "github-publisher-form")
            .replace("<label for=\"project_name\">PyPI Project Name</label><input id=\"project_name\" name=\"project_name\" required>", "");
        page.goto(&format!("data:text/html,{}", encode_uri_component(&existing)))
            .await?;
        config.organization = "acme".into();
        wait_for_form(&page, &config, false, Duration::from_millis(10), Duration::from_secs(5)).await?;
        submit_form(&page, &config, true, false, Duration::from_millis(10), Duration::from_secs(5)).await?;
        assert_eq!(page.evaluate("document.querySelector('#publishers td').textContent").await?, "GitHub");
        anyhow::Ok(())
    }
    .await;
    page.close().await;
    result
}
