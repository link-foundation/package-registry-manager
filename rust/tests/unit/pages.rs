use package_registry_manager::auth_urls::CommandOutput;
use package_registry_manager::pages::{
    pages_settings_url, pages_state, pages_steps, pages_workflow, PagesState,
};
use package_registry_manager::workflows::Workflow;
use package_registry_manager::RepositoryInfo;

const DOCS_WORKFLOW: &str = "jobs:\n  deploy:\n    steps:\n      - uses: actions/configure-pages@v6\n      - uses: actions/deploy-pages@v5\n";

fn workflow(name: &str, contents: &str) -> Workflow {
    Workflow {
        name: name.to_owned(),
        contents: contents.to_owned(),
    }
}

fn output(code: i32, stdout: &str, stderr: &str) -> CommandOutput {
    CommandOutput {
        code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        legacy_login: false,
        approval_expired: false,
    }
}

#[test]
fn finds_the_workflow_that_deploys_to_github_pages() {
    assert_eq!(
        pages_workflow(&[
            workflow("ci.yml", "steps:\n  - uses: actions/checkout@v6\n"),
            workflow("docs.yml", DOCS_WORKFLOW),
        ])
        .as_deref(),
        Some("docs.yml")
    );
    assert_eq!(
        pages_workflow(&[workflow("ci.yml", "# actions/deploy-pages is not used\n")]),
        None
    );
}

#[test]
fn reads_the_pages_site_from_gh_api() {
    assert_eq!(
        pages_state(&output(0, r#"{"build_type":"workflow"}"#, "")),
        Some(PagesState::Workflow)
    );
    assert_eq!(
        pages_state(&output(0, r#"{"build_type":"legacy"}"#, "")),
        Some(PagesState::Legacy)
    );
    assert_eq!(
        pages_state(&output(
            1,
            r#"{"message":"Not Found","status":"404"}"#,
            "gh: Not Found (HTTP 404)"
        )),
        Some(PagesState::Missing)
    );
    assert_eq!(
        pages_state(&output(1, "", "gh: Forbidden (HTTP 403)")),
        None
    );
}

#[test]
fn enables_or_switches_pages_only_after_a_confirmation() {
    let [check, enable, switch] = pages_steps("acme/demo", "docs.yml");
    assert_eq!(check.id, "check-pages");
    assert_eq!(
        check.command.expect("check command").args,
        ["api", "repos/acme/demo/pages"]
    );
    assert_eq!(enable.when.as_deref(), Some("pages-missing"));
    assert!(enable.confirm);
    assert_eq!(
        enable.command.expect("enable command").args,
        [
            "api",
            "-X",
            "POST",
            "repos/acme/demo/pages",
            "-f",
            "build_type=workflow"
        ]
    );
    assert_eq!(switch.when.as_deref(), Some("pages-legacy"));
    assert!(switch.confirm);
    assert_eq!(switch.command.expect("switch command").args[2], "PUT");
    let repository = RepositoryInfo {
        root: ".".to_owned(),
        github_owner: Some("acme".to_owned()),
        github_repository: Some("demo".to_owned()),
        release_workflow: None,
        pages_workflow: Some("docs.yml".to_owned()),
    };
    assert_eq!(
        pages_settings_url(&repository),
        "https://github.com/acme/demo/settings/pages"
    );
}
