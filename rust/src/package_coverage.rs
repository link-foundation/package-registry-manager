//! Manifest-specific executable publishing steps for wrapper packages.
use crate::model::Package;
use crate::plan::package_directory;
use crate::publishers::{executable_lines, parse_workflow, script_references};
use crate::workflows::Workflow;
use regex::Regex;

fn working(lines: &[String]) -> Option<String> {
    let pattern =
        Regex::new(r#"^\s*working-directory\s*:\s*['"]?([\w./-]+)"#).expect("static pattern");
    lines
        .iter()
        .filter_map(|line| pattern.captures(line).map(|value| value[1].to_owned()))
        .next_back()
}

/// Keep executable steps for this manifest, preserving the job's OIDC identity.
#[must_use]
pub fn package_workflows(workflows: &[Workflow], package: &Package) -> Vec<Workflow> {
    let directory = package_directory(&package.manifest);
    let step = Regex::new(r"^\s*-\s+[\w-]+\s*:").expect("static pattern");
    let cd = Regex::new(r#"\bcd\s+['"]?([\w./-]+)"#).expect("static pattern");
    workflows
        .iter()
        .map(|workflow| {
            let parsed = parse_workflow(&workflow.contents);
            let jobs = parsed
                .jobs
                .iter()
                .filter_map(|job| {
                    let start = job
                        .lines
                        .iter()
                        .position(|line| line.trim_start().starts_with("steps:"));
                    let Some(start) = start else {
                        let called = job.lines.iter().any(|line| {
                            line.trim_start().starts_with("uses: ./.github/workflows/")
                                || line.trim_start().starts_with("uses: './.github/workflows/")
                                || line
                                    .trim_start()
                                    .starts_with("uses: \"./.github/workflows/")
                        });
                        let cwd = working(&job.lines)
                            .or_else(|| working(&parsed.header))
                            .unwrap_or_else(|| ".".into());
                        return (called
                            && cwd.trim_start_matches("./").trim_end_matches('/') == directory)
                            .then(|| format!("  {}:\n{}", job.name, job.lines.join("\n")));
                    };
                    let prefix = &job.lines[..=start];
                    let mut blocks: Vec<Vec<String>> = Vec::new();
                    for line in &job.lines[start + 1..] {
                        if step.is_match(line) {
                            blocks.push(Vec::new());
                        }
                        if let Some(block) = blocks.last_mut() {
                            block.push(line.clone());
                        }
                    }
                    let fallback = working(prefix)
                        .or_else(|| working(&parsed.header))
                        .unwrap_or_else(|| ".".into());
                    let matching: Vec<_> = blocks
                        .into_iter()
                        .filter(|block| {
                            let commands = executable_lines(block);
                            if commands.is_empty() {
                                return false;
                            }
                            let text = commands.join("\n");
                            if text.contains("--workspaces")
                                || text.contains("--recursive")
                                || text.contains("publish --all")
                                || text.contains(&format!("--workspace={}", package.name))
                                || text.contains(&format!("--workspace {}", package.name))
                            {
                                return true;
                            }
                            let script = script_references(&commands)
                                .iter()
                                .any(|script| {
                                    directory != "." && script.starts_with(&format!("{directory}/"))
                                })
                                .then(|| directory.to_owned());
                            let cwd = working(block)
                                .or_else(|| cd.captures(&text).map(|value| value[1].to_owned()))
                                .or(script)
                                .unwrap_or_else(|| fallback.clone());
                            cwd.trim_start_matches("./").trim_end_matches('/') == directory
                        })
                        .flatten()
                        .collect();
                    (!matching.is_empty()).then(|| {
                        format!(
                            "  {}:\n{}\n{}",
                            job.name,
                            prefix.join("\n"),
                            matching.join("\n")
                        )
                    })
                })
                .collect::<Vec<_>>();
            Workflow {
                name: workflow.name.clone(),
                contents: format!("{}\njobs:\n{}", parsed.header.join("\n"), jobs.join("\n")),
            }
        })
        .collect()
}

/// Warn when a second npm manifest stops tracking the package it re-exports.
pub fn audit_wrappers(root: &std::path::Path, packages: &mut [Package]) -> anyhow::Result<()> {
    let npm = packages
        .iter()
        .filter(|p| p.registry == crate::Registry::Npm)
        .cloned()
        .collect::<Vec<_>>();
    for wrapper in packages
        .iter_mut()
        .filter(|p| p.registry == crate::Registry::Npm)
    {
        let contents = std::fs::read_to_string(root.join(&wrapper.manifest))?;
        let data: serde_json::Value =
            serde_json::from_str(contents.trim_start_matches('\u{feff}'))?;
        for target in npm
            .iter()
            .filter(|p| p.name != wrapper.name && data["dependencies"].get(&p.name).is_some())
        {
            if wrapper.version != target.version {
                wrapper.warnings.push(format!("wrapper {}@{} differs from {}@{}; update both manifests and the wrapper dependency for every release",wrapper.name,wrapper.version.as_deref().unwrap_or_default(),target.name,target.version.as_deref().unwrap_or_default()));
            }
        }
    }
    Ok(())
}
