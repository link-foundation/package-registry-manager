//! GitHub Actions workflow discovery shared by package and container detection.

use std::fs;
use std::path::Path;

use anyhow::Result;
use regex::Regex;

use crate::model::Registry;

/// A workflow file under `.github/workflows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    /// File name, such as `release.yml`.
    pub name: String,
    /// Raw YAML contents.
    pub contents: String,
}

fn is_yaml(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
    })
}

const fn publish_pattern(registry: Registry) -> Option<&'static str> {
    match registry {
        Registry::Npm => Some(r"\bnpm (?:stage )?publish\b|\bchangeset publish\b"),
        Registry::CratesIo => Some(r"\bcargo publish\b|rust-lang/crates-io-auth-action"),
        Registry::PyPi => {
            Some(r"pypa/gh-action-pypi-publish|\btwine upload\b|\buv publish\b|\bpoetry publish\b")
        }
        Registry::DockerHub => Some(r"(?i)\bDOCKER_?HUB_|\bdocker\.io/|hub\.docker\.com"),
        Registry::Ghcr => Some(r"\bghcr\.io\b"),
        _ => None,
    }
}

/// Read `.yml` and `.yaml` workflow files sorted by file name.
pub fn read_workflows(root: &Path) -> Result<Vec<Workflow>> {
    let Ok(entries) = fs::read_dir(root.join(".github/workflows")) else {
        return Ok(Vec::new());
    };
    let mut workflows = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_file() && is_yaml(&name) {
            let contents = fs::read_to_string(entry.path())?;
            workflows.push(Workflow { name, contents });
        }
    }
    workflows.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(workflows)
}

/// Return the first workflow that publishes to the registry, if any.
#[must_use]
pub fn publishing_workflow(workflows: &[Workflow], registry: Registry) -> Option<&Workflow> {
    let pattern = Regex::new(publish_pattern(registry)?).expect("static pattern must compile");
    workflows
        .iter()
        .find(|workflow| pattern.is_match(&workflow.contents))
}

/// Keep the historical npm-first release workflow heuristic.
#[must_use]
pub fn release_workflow(workflows: &[Workflow]) -> Option<String> {
    publishing_workflow(workflows, Registry::Npm)
        .or_else(|| {
            workflows
                .iter()
                .find(|workflow| workflow.name.contains("release"))
        })
        .map(|workflow| workflow.name.clone())
}

/// Report whether a workflow grants `packages: write` to its token.
#[must_use]
pub fn grants_packages_write(contents: &str) -> bool {
    [
        r#"(?m)^\s*packages\s*:\s*['"]?write\b"#,
        r#"(?m)^\s*permissions\s*:\s*['"]?write-all\b"#,
    ]
    .iter()
    .any(|pattern| {
        Regex::new(pattern)
            .expect("static pattern must compile")
            .is_match(contents)
    })
}
