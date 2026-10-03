//! GitHub Pages readiness.
//!
//! A workflow that deploys with `actions/deploy-pages` fails with "Get Pages
//! site failed ... Not Found" until Pages is enabled with GitHub Actions as
//! its source, which only a repository administrator can do.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::auth_urls::CommandOutput;
use crate::model::{RepositoryInfo, SetupStep, StepKind};
use crate::workflows::Workflow;

static PAGES_ACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^\s*(?:-\s*)?uses\s*:\s*['"]?actions/(?:configure-pages|deploy-pages|upload-pages-artifact)@"#,
    )
    .expect("valid Pages action pattern")
});
static NOT_FOUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bHTTP 404\b|"message"\s*:\s*"Not Found""#).expect("valid 404 pattern")
});

/// Step ids that change the Pages site; both need administrator rights.
pub const PAGES_CHANGES: [&str; 2] = ["enable-pages", "use-pages-workflow"];

/// What `gh api repos/{slug}/pages` reports about the Pages site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagesState {
    /// Pages deploys from GitHub Actions.
    Workflow,
    /// Pages builds from a branch, so a workflow's deployment is not served.
    Legacy,
    /// Pages is not enabled: the API answers 404.
    Missing,
}

/// The name of the first workflow that deploys to GitHub Pages.
#[must_use]
pub fn pages_workflow(workflows: &[Workflow]) -> Option<String> {
    workflows
        .iter()
        .find(|workflow| PAGES_ACTION.is_match(&workflow.contents))
        .map(|workflow| workflow.name.clone())
}

/// Where Pages is configured by hand: Settings -> Pages -> Source.
#[must_use]
pub fn pages_settings_url(repository: &RepositoryInfo) -> String {
    format!(
        "https://github.com/{}/{}/settings/pages",
        repository.github_owner.as_deref().unwrap_or_default(),
        repository.github_repository.as_deref().unwrap_or_default()
    )
}

/// Steps that check the repository's Pages site and, after a confirmation
/// each, enable it or switch it to GitHub Actions as its source.
#[must_use]
pub fn pages_steps(slug: &str, workflow: &str) -> [SetupStep; 3] {
    let site = format!("repos/{slug}/pages");
    [
        SetupStep::new(
            "check-pages",
            "Check that GitHub Pages deploys from GitHub Actions",
            StepKind::Check,
            format!("{workflow} deploys to GitHub Pages, which must be enabled with GitHub Actions as its source."),
        )
        .command("gh", &["api", &site])
        .cwd("."),
        SetupStep::new(
            "enable-pages",
            "Enable GitHub Pages with GitHub Actions as its source",
            StepKind::Command,
            "Create the Pages site so the workflow's deployment no longer fails with Not Found. This needs repository administrator rights.",
        )
        .command("gh", &["api", "-X", "POST", &site, "-f", "build_type=workflow"])
        .when("pages-missing")
        .cwd(".")
        .confirmed(),
        SetupStep::new(
            "use-pages-workflow",
            "Switch GitHub Pages to GitHub Actions as its source",
            StepKind::Command,
            "Pages builds from a branch, so the workflow's deployment is not served. This needs repository administrator rights.",
        )
        .command("gh", &["api", "-X", "PUT", &site, "-f", "build_type=workflow"])
        .when("pages-legacy")
        .cwd(".")
        .confirmed(),
    ]
}

/// Read `gh api repos/{slug}/pages`; `None` when the answer is unknown.
#[must_use]
pub fn pages_state(result: &CommandOutput) -> Option<PagesState> {
    if result.code == 0 {
        let site: Value = serde_json::from_str(&result.stdout).ok()?;
        return Some(if site["build_type"] == "workflow" {
            PagesState::Workflow
        } else {
            PagesState::Legacy
        });
    }
    (NOT_FOUND.is_match(&result.stdout) || NOT_FOUND.is_match(&result.stderr))
        .then_some(PagesState::Missing)
}

/// Print what the Pages check found and return the state.
#[must_use]
pub fn report_pages(result: &CommandOutput, repository: &RepositoryInfo) -> Option<PagesState> {
    let state = pages_state(result);
    match state {
        Some(PagesState::Workflow) => {
            println!("  GitHub Pages is enabled with GitHub Actions as its source.");
        }
        Some(PagesState::Legacy) => println!(
            "  GitHub Pages builds from a branch, so the workflow's deployment is not served."
        ),
        Some(PagesState::Missing) => println!(
            "  GitHub Pages is not enabled, so the workflow's deployment fails with Not Found."
        ),
        None => eprintln!(
            "warning: could not read the GitHub Pages site; check its source at {}",
            pages_settings_url(repository)
        ),
    }
    state
}

/// The warning printed when gh could not change the Pages site.
#[must_use]
pub fn pages_change_warning(repository: &RepositoryInfo) -> String {
    format!(
        "warning: could not change GitHub Pages (this needs repository administrator rights); set its source to GitHub Actions at {}",
        pages_settings_url(repository)
    )
}
