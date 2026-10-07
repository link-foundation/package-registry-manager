//! GitHub Actions workflow discovery shared by package and container detection.

use std::fs;
use std::path::Path;

use anyhow::Result;
use regex::Regex;

use crate::model::Registry;
use crate::publishers::{executable_lines, parse_workflow};
use crate::source_code::{command_position, strip_comments};

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

fn build_push(lines: &[String]) -> bool {
    let step = Regex::new(r"^\s*-\s+[\w-]+\s*:").expect("static pattern");
    let start = lines
        .iter()
        .position(|line| line.trim_start().starts_with("steps:"));
    let step_indent = start
        .and_then(|index| lines[index + 1..].iter().find(|line| step.is_match(line)))
        .map(|line| line.len() - line.trim_start().len());
    let action = Regex::new(r#"^\s*-?\s*uses\s*:\s*['"]?docker/build-push-action@"#)
        .expect("static pattern");
    let push = Regex::new(r"^\s*push\s*:\s*true\b").expect("static pattern");
    let mut building = false;
    for line in lines {
        if Some(line.len() - line.trim_start().len()) == step_indent && step.is_match(line) {
            building = false;
        }
        if action.is_match(line) {
            building = true;
        }
        if building && push.is_match(line) {
            return true;
        }
    }
    false
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
    if ![Registry::DockerHub, Registry::Ghcr].contains(&registry) {
        return None;
    }
    workflows.iter().find(|workflow| {
        parse_workflow(&strip_comments(&workflow.contents, "workflow.yml"))
            .jobs
            .iter()
            .any(|job| {
                let text = job.lines.join("\n");
                let matches =
                    |pattern: &str| Regex::new(pattern).expect("static pattern").is_match(&text);
                let pushes = executable_lines(&job.lines).iter().any(|line| {
                    Regex::new(r"\bdocker\s+push\b")
                        .expect("static pattern")
                        .is_match(line)
                        && command_position(line.split("docker").next().unwrap_or_default())
                }) || build_push(&job.lines);
                if !pushes {
                    return false;
                }
                let ghcr = matches(r"\bghcr\.io\b");
                if registry == Registry::Ghcr {
                    ghcr
                } else {
                    matches(r"(?i)\bDOCKER_?HUB_|\bdocker\.io/|hub\.docker\.com")
                        || matches(r#"(?m)^\s*-?\s*uses\s*:\s*['"]?docker/login-action@"#) && !ghcr
                }
            })
    })
}

/// Report whether a workflow grants `packages: write` to its token.
#[must_use]
pub fn grants_packages_write(contents: &str) -> bool {
    let contents = strip_comments(contents, "workflow.yml");
    [
        r#"(?m)^\s*packages\s*:\s*['"]?write\b"#,
        r#"(?m)^\s*permissions\s*:\s*['"]?write-all\b"#,
    ]
    .iter()
    .any(|pattern| {
        Regex::new(pattern)
            .expect("static pattern must compile")
            .is_match(&contents)
    })
}
