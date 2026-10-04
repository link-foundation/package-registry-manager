use browser_commander::browser::real_browser::launch_real_browser;
use browser_commander::{
    connect_browser, measure_session_parity, ConnectOptions, MeasureParityOptions,
};
use package_registry_manager::automation::launch_options;
use package_registry_manager::browser_options::BrowserOptions;
use serde_json::Value;

/// Launches a real browser, so it runs only when `PRM_BROWSER_SMOKE=1` (CI
/// runs it under `xvfb-run` with Google Chrome installed).
#[tokio::test]
async fn launches_the_automated_profile_like_a_browser_started_by_hand() -> anyhow::Result<()> {
    if std::env::var("PRM_BROWSER_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipped: set PRM_BROWSER_SMOKE=1 to launch Chrome");
        return Ok(());
    }
    let profile = tempfile::tempdir()?;
    let launched = Box::pin(launch_real_browser(launch_options(
        &BrowserOptions::default(),
        profile.path(),
        false,
        &[],
    )))
    .await?;
    let measured = tokio::time::timeout(
        std::time::Duration::from_secs(180),
        Box::pin(async {
            launched.page.goto("about:blank").await?;
            let webdriver = launched.page.evaluate("navigator.webdriver").await?;
            // The parity probe needs a `LaunchResult`, so it attaches to the very
            // browser the CLI launched rather than launching one of its own.
            let mut connection = ConnectOptions::chromiumoxide();
            connection.cdp_endpoint = Some(launched.cdp_endpoint.clone());
            let session = connect_browser(connection).await?;
            let report = Box::pin(measure_session_parity(
                &session,
                MeasureParityOptions {
                    attached: true,
                    ..MeasureParityOptions::default()
                },
            ))
            .await?;
            anyhow::Ok((webdriver, report))
        }),
    )
    .await;
    launched.close().await?;
    let (webdriver, report) = measured??;
    assert_eq!(webdriver, Value::Bool(false));
    assert!(
        report.command_line.comparison.extra.is_empty(),
        "extra switches in {}",
        report.command_line.launched
    );
    assert!(report.unlisted.is_empty(), "{:?}", report.unlisted);
    assert!(report.ok);
    Ok(())
}
