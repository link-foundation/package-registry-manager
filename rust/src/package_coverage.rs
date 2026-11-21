//! Manifest-specific executable publishing steps for wrapper packages.
use crate::model::{Package, Registry};
use crate::plan::package_directory;
use crate::publishers::{executable_lines, parse_workflow, script_references};
use crate::source_code::command_position;
use crate::workflows::Workflow;

fn working(lines: &[String]) -> Option<String> {
    let pattern =
        crate::publishers::regex(r#"^\s*(?:-\s+)?working-directory\s*:\s*['"]?([\w./-]+)"#);
    lines
        .iter()
        .filter_map(|line| pattern.captures(line).map(|value| value[1].to_owned()))
        .next_back()
}

fn ancestor(cwd: &str, child: &str) -> bool {
    child != cwd && (cwd == "." || child.starts_with(&format!("{cwd}/")))
}

/// A step running in a parent directory publishes this package only when it
/// names it, as in `cargo publish -p name` or a shell function invoked with the
/// name, or reads its manifest by path.
fn names_package(package: &Package, text: &str, cwd: &str) -> bool {
    let relative = if cwd == "." {
        package.manifest.as_str()
    } else {
        package.manifest.get(cwd.len() + 1..).unwrap_or_default()
    };
    let before = |c: char| c.is_ascii_alphanumeric() || c == '_' || matches!(c, '.' | '/' | '-');
    let after = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    [package.name.as_str(), relative]
        .into_iter()
        .filter(|token| !token.is_empty())
        .any(|token| {
            text.match_indices(token).any(|(start, _)| {
                !text[..start].chars().next_back().is_some_and(before)
                    && !text[start + token.len()..]
                        .chars()
                        .next()
                        .is_some_and(after)
            })
        })
}

fn tarball_match(package: &Package, commands: &[String]) -> Option<bool> {
    let expression = crate::publishers::regex(r"\$\{\{[^}]+\}\}");
    let publish = crate::publishers::regex(r#"\bnpm\s+publish\s+("[^"]*"|'[^']*'|[^\s;&|]+)"#);
    let version = crate::publishers::regex(r"^(?:v?\d|\$)");
    let expected = package.name.trim_start_matches('@').replace('/', "-");
    let mut matched = None;
    for line in commands {
        let text = expression.replace_all(line, "$$VERSION");
        for target in publish.captures_iter(&text) {
            let start = target.get(0).expect("publish command").start();
            if !command_position(&text[..start]) {
                continue;
            }
            let Some(stem) = target[1]
                .trim_matches(['"', '\''])
                .rsplit('/')
                .next()
                .and_then(|file| file.strip_suffix(".tgz"))
            else {
                continue;
            };
            // npm pack flattens @scope/name to scope-name. An optional
            // version suffix must not mistake foo-extra for foo.
            if stem == expected
                || stem
                    .strip_prefix(&expected)
                    .and_then(|value| value.strip_prefix('-'))
                    .is_some_and(|suffix| version.is_match(suffix))
            {
                return Some(true);
            }
            matched = Some(false);
        }
    }
    matched
}

/// Keep executable steps for this manifest, preserving the job's OIDC identity.
#[must_use]
pub fn package_workflows(workflows: &[Workflow], package: &Package) -> Vec<Workflow> {
    scoped_workflows(workflows, package, true)
}

pub(crate) fn scoped_workflows(
    workflows: &[Workflow],
    package: &Package,
    match_directories: bool,
) -> Vec<Workflow> {
    let directory = package_directory(&package.manifest);
    let step = crate::publishers::regex(r"^\s*-\s+[\w-]+\s*:");
    let cd = crate::publishers::regex(r#"\bcd\s+['"]?([\w./-]+)"#);
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
                        // A reusable-workflow caller has no steps or working directory of
                        // its own; it carries the OIDC identity of whatever its callee
                        // publishes, and the callee is scoped separately.
                        return called
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
                    let matching: Vec<bool> = blocks
                        .iter()
                        .map(|block| {
                            let commands = executable_lines(block);
                            if commands.is_empty() {
                                return false;
                            }
                            if package.registry == Registry::Npm {
                                if let Some(matched) = tarball_match(package, &commands) {
                                    return matched;
                                }
                            }
                            if !match_directories {
                                return true;
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
                            let cwd = cwd.trim_start_matches("./").trim_end_matches('/');
                            let cwd = if cwd.is_empty() { "." } else { cwd };
                            cwd == directory
                                || (ancestor(cwd, directory) && names_package(package, &text, cwd))
                        })
                        .collect();
                    // Action-only steps (authentication exchanges such as
                    // crates-io-auth-action) belong to the job that publishes this package.
                    let run = crate::publishers::regex(r"^\s*-?\s*run\s*:");
                    let kept = blocks
                        .iter()
                        .zip(&matching)
                        .filter(|(block, matched)| {
                            **matched || !block.iter().any(|line| run.is_match(line))
                        })
                        .flat_map(|(block, _)| block.iter().cloned())
                        .collect::<Vec<_>>();
                    matching.contains(&true).then(|| {
                        format!(
                            "  {}:\n{}\n{}",
                            job.name,
                            prefix.join("\n"),
                            kept.join("\n")
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
