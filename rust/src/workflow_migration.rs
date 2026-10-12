//! Reviewable conversions of existing publishing jobs and reusable workflow callers.
use crate::{
    model::{Package, Registry},
    publishers::{job_publishes, parse_workflow, registry_token_secrets, Job, ParsedWorkflow},
    workflows::Workflow,
};
use anyhow::{bail, Context, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Reviewed v1.0.5 commit, shared with JavaScript.
pub const CRATES_AUTH_ACTION: &str =
    "rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18";
type Targets = BTreeMap<String, BTreeMap<String, BTreeSet<Registry>>>;
fn rx(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("valid pattern")
}
fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}
fn called(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        rx(r#"^\s*uses:\s*["']?\./\.github/workflows/([\w.-]+\.ya?ml)"#)
            .captures(line)
            .map(|c| c[1].into())
    })
}
fn keys(registry: Registry) -> BTreeSet<String> {
    let mut names: BTreeSet<_> = registry_token_secrets(registry)
        .iter()
        .map(|name| (*name).into())
        .collect();
    if registry == Registry::Npm {
        names.insert("NODE_AUTH_TOKEN".into());
    }
    if registry == Registry::PyPi {
        names.extend(["TWINE_PASSWORD".into(), "TWINE_USERNAME".into()]);
    }
    names
}
fn end_of(lines: &[String], start: usize) -> usize {
    let level = indent(&lines[start]);
    (start + 1..lines.len())
        .find(|&i| {
            !lines[i].trim().is_empty()
                && !lines[i].trim_start().starts_with('#')
                && indent(&lines[i]) <= level
        })
        .unwrap_or(lines.len())
}
fn remove_credentials(lines: &[String], names: &BTreeSet<String>) -> Vec<String> {
    let mut result = lines.to_vec();
    for i in (0..result.len()).rev() {
        let key = rx(r"^\s*([\w-]+):")
            .captures(&result[i])
            .map(|c| c[1].to_owned());
        if key.is_some_and(|key| names.contains(&key)) {
            let end = end_of(&result, i);
            result.drain(i..end);
        }
    }
    for i in (0..result.len()).rev() {
        if rx(r"^\s*(?:env|secrets|with):\s*$").is_match(&result[i])
            && result[i + 1..end_of(&result, i)]
                .iter()
                .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
        {
            result.remove(i);
        }
    }
    result
}
fn grant(lines: Vec<String>, level: usize) -> Result<Vec<String>> {
    let mut result = lines;
    let Some(start) = result
        .iter()
        .position(|line| indent(line) == level && line.trim_start().starts_with("permissions:"))
    else {
        let mut prefix = vec![
            format!("{}permissions:", " ".repeat(level)),
            format!("{}contents: read", " ".repeat(level + 2)),
            format!("{}id-token: write", " ".repeat(level + 2)),
        ];
        prefix.extend(result);
        return Ok(prefix);
    };
    if result[start].trim() == "permissions: write-all" {
        return Ok(result);
    }
    if !rx(r"permissions:\s*(?:#.*)?$").is_match(&result[start]) {
        bail!("migration requires a mapping permissions block; expand inline permissions first");
    }
    let end = end_of(&result, start);
    let token = (start + 1..end).find(|&i| result[i].trim_start().starts_with("id-token:"));
    let child = result[start + 1..end]
        .iter()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map_or(level + 2, |line| indent(line));
    let entry = format!("{}id-token: write", " ".repeat(child));
    if let Some(index) = token {
        result[index] = entry;
    } else {
        result.insert(start + 1, entry);
    }
    Ok(result)
}
fn with_env(mut lines: Vec<String>, key: &str, value: &str, step_level: usize) -> Vec<String> {
    let level = step_level + 2;
    let start = lines
        .iter()
        .position(|line| indent(line) == level && line.trim() == "env:");
    let entry = format!("{}{key}: {value}", " ".repeat(level + 2));
    if let Some(start) = start {
        lines.insert(start + 1, entry);
    } else {
        lines.extend([format!("{}env:", " ".repeat(level)), entry]);
    }
    lines
}
fn grant_job(lines: Vec<String>, level: usize, parsed: &ParsedWorkflow) -> Result<Vec<String>> {
    if !lines
        .iter()
        .any(|line| indent(line) == level && line.trim_start().starts_with("permissions:"))
    {
        if let Some(start) = parsed
            .header
            .iter()
            .position(|line| line.starts_with("permissions:"))
        {
            if parsed.header[start].trim() == "permissions: write-all" {
                return Ok(lines);
            }
            let mut inherited: Vec<_> = parsed.header[start..end_of(&parsed.header, start)]
                .iter()
                .map(|line| format!("{}{line}", " ".repeat(level)))
                .collect();
            inherited.extend(lines);
            return grant(inherited, level);
        }
    }
    grant(lines, level)
}
fn convert_job(
    root: &Path,
    parsed: &ParsedWorkflow,
    lines: &[String],
    registry: Registry,
) -> Result<Vec<String>> {
    let names = keys(registry);
    if lines
        .iter()
        .any(|line| rx(r"^\s*(?:env|secrets):\s*[^\s#]").is_match(line))
    {
        bail!("expand inline environment and secret mappings before migration");
    }
    let level = lines
        .iter()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| indent(line))
        .min()
        .context("empty publishing job")?;
    if called(lines).is_some() {
        return grant_job(remove_credentials(lines, &names), level, parsed);
    }
    let start = lines
        .iter()
        .position(|line| indent(line) == level && line.trim() == "steps:")
        .context("publishing job has no editable steps block")?;
    let end = end_of(lines, start);
    let positions: Vec<_> = (start + 1..end)
        .filter(|&i| rx(r"^\s*-\s+(?:name|run|uses|id):").is_match(&lines[i]))
        .collect();
    let step_level = indent(
        &lines[*positions
            .first()
            .context("publishing job has no editable steps")?],
    );
    let mut edits = Vec::new();
    let mut guards = Vec::new();
    let mut first = None;
    for (index, &position) in positions.iter().enumerate() {
        let step = &lines[position..positions.get(index + 1).copied().unwrap_or(end)];
        let mut job_lines = vec![format!("{}steps:", " ".repeat(level))];
        job_lines.extend_from_slice(step);
        let publishes = job_publishes(
            root,
            parsed,
            &Job {
                name: String::new(),
                lines: job_lines,
            },
            registry,
        );
        let credential = step.iter().any(|line| {
            names
                .iter()
                .any(|name| rx(&format!(r"^\s*{name}:")).is_match(line))
        });
        let mut updated = remove_credentials(step, &names);
        if publishes || credential {
            first.get_or_insert(index);
            if registry == Registry::CratesIo {
                let guard = step.iter().position(|line| {
                    indent(line) == step_level + 2 && line.trim_start().starts_with("if:")
                });
                guards.push(
                    guard.map_or_else(Vec::new, |start| step[start..end_of(step, start)].to_vec()),
                );
                updated = with_env(
                    updated,
                    "CARGO_REGISTRY_TOKEN",
                    "${{ steps.prm_auth.outputs.token }}",
                    step_level,
                );
            }
            if registry == Registry::Npm && publishes {
                updated = with_env(updated, "NPM_CONFIG_PROVENANCE", "'true'", step_level);
            }
            if registry == Registry::PyPi
                && publishes
                && step
                    .iter()
                    .any(|line| line.contains("pypa/gh-action-pypi-publish@"))
            {
                let mut action_keys = names.clone();
                action_keys.extend(["password".into(), "user".into()]);
                updated = remove_credentials(&updated, &action_keys);
            }
            if registry == Registry::PyPi
                && publishes
                && !step
                    .iter()
                    .any(|line| line.contains("pypa/gh-action-pypi-publish@"))
            {
                let run = step.iter().find(|line| rx(r"^\s*-?\s*run:").is_match(line));
                let pattern =
                    rx(r"^\s*(-?\s*)run:\s*(?:python(?:3)? -m )?twine upload ([\w./-]+)\/\*\s*$");
                let upload = run.and_then(|line| pattern.captures(line));
                let upload = upload.filter(|upload| !upload[2].split('/').any(|part| part == "..") && !step.iter().any(|line| line.trim_start().starts_with("working-directory:"))).context("PyPI migration requires a simple twine upload DIRECTORY/* without a working-directory, or a pypa publishing action")?;
                updated = updated
                    .into_iter()
                    .filter(|line| !line.trim_start().starts_with("shell:"))
                    .map(|line| {
                        if Some(&line) == run {
                            rx("run:.*")
                                .replace(&line, "uses: pypa/gh-action-pypi-publish@release/v1")
                                .into_owned()
                        } else {
                            line
                        }
                    })
                    .collect();
                updated.extend([
                    format!("{}with:", " ".repeat(step_level + 2)),
                    format!(
                        "{}packages-dir: '{}/'",
                        " ".repeat(step_level + 4),
                        &upload[2]
                    ),
                ]);
            }
        }
        edits.push(updated);
    }
    let first = first.context("cannot locate publishing step for migration")?;
    let prefix = " ".repeat(step_level);
    if registry == Registry::CratesIo {
        if guards.iter().any(|guard| guard != &guards[0]) {
            bail!("different publishing conditions require separate authentication steps; review this job before migration");
        }
        if lines
            .iter()
            .any(|line| rx(r"\bid:\s*prm_auth\b|rust-lang/crates-io-auth-action@").is_match(line))
        {
            bail!("existing crates.io auth step needs review before converting token wiring");
        }
        let mut auth = vec![
            format!("{prefix}- name: Authenticate with crates.io (trusted publishing)"),
            format!("{prefix}  uses: {CRATES_AUTH_ACTION}"),
            format!("{prefix}  id: prm_auth"),
        ];
        auth.extend_from_slice(&guards[0]);
        edits.insert(first, auth);
    } else if registry == Registry::Npm {
        edits.insert(
            first,
            vec![
                format!("{prefix}- uses: actions/setup-node@v6"),
                format!("{prefix}  with:"),
                format!("{prefix}    node-version: '22'"),
                format!("{prefix}- run: npm install --global npm@^11"),
            ],
        );
    }
    let mut result = remove_credentials(&lines[..start], &names);
    result.push(lines[start].clone());
    result.extend(edits.into_iter().flatten());
    result.extend(remove_credentials(&lines[end..], &names));
    grant_job(result, level, parsed)
}
fn visit(
    root: &Path,
    workflows: &BTreeMap<String, ParsedWorkflow>,
    name: &str,
    registry: Registry,
    seen: &mut BTreeSet<String>,
    targets: &mut Targets,
) -> Result<bool> {
    if !seen.insert(name.into()) {
        bail!("cyclic reusable workflows cannot be migrated");
    }
    let parsed = workflows
        .get(name)
        .with_context(|| format!("missing reusable workflow {name}"))?;
    let mut found = false;
    for job in &parsed.jobs {
        let publishes = if let Some(called) = called(&job.lines) {
            visit(root, workflows, &called, registry, seen, targets)?
        } else {
            job_publishes(root, parsed, job, registry)
        };
        if publishes {
            found = true;
            targets
                .entry(name.into())
                .or_default()
                .entry(job.name.clone())
                .or_default()
                .insert(registry);
        }
    }
    seen.remove(name);
    Ok(found)
}

/// Generate all workflow edits without mutating a checkout or registry.
pub fn workflow_migration_proposal(
    root: &Path,
    packages: &[Package],
    workflows: &[Workflow],
) -> Result<BTreeMap<String, String>> {
    let parsed: BTreeMap<_, _> = workflows
        .iter()
        .map(|workflow| (workflow.name.clone(), parse_workflow(&workflow.contents)))
        .collect();
    let mut targets = Targets::new();
    let mut registries = BTreeSet::new();
    for registry in packages.iter().map(|package| package.registry) {
        if !registries.insert(registry) {
            continue;
        }
        if !matches!(
            registry,
            Registry::CratesIo | Registry::Npm | Registry::PyPi
        ) {
            bail!("workflow conversion is unavailable for {registry}");
        }
        for workflow in workflows {
            visit(
                root,
                &parsed,
                &workflow.name,
                registry,
                &mut BTreeSet::new(),
                &mut targets,
            )?;
        }
    }
    let mut files = BTreeMap::new();
    for workflow in workflows {
        let Some(jobs) = targets.get(&workflow.name) else {
            continue;
        };
        let lines: Vec<String> = workflow.contents.lines().map(str::to_owned).collect();
        let start = lines
            .iter()
            .position(|line| line.trim_end() == "jobs:")
            .context("workflow has no jobs block")?;
        let end = end_of(&lines, start);
        let job_level = lines[start + 1..end]
            .iter()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .map(|line| indent(line))
            .min()
            .context("empty jobs block")?;
        let positions: Vec<_> = (start + 1..end)
            .filter(|&i| {
                indent(&lines[i]) == job_level
                    && rx(r#"^\s*["']?[\w-]+["']?:\s*$"#).is_match(&lines[i])
            })
            .collect();
        let mut body = Vec::new();
        for (index, &position) in positions.iter().enumerate() {
            let name = rx(r#"^\s*["']?([\w-]+)["']?:"#)
                .captures(&lines[position])
                .context("invalid job name")?[1]
                .to_owned();
            let mut content =
                lines[position + 1..positions.get(index + 1).copied().unwrap_or(end)].to_vec();
            for &registry in jobs.get(&name).into_iter().flatten() {
                content = convert_job(root, &parsed[&workflow.name], &content, registry)?;
            }
            body.push(lines[position].clone());
            body.extend(content);
        }
        let names: BTreeSet<_> = jobs
            .values()
            .flat_map(|registries| registries.iter().flat_map(|&registry| keys(registry)))
            .collect();
        for job in parsed[&workflow.name]
            .jobs
            .iter()
            .filter(|job| !jobs.contains_key(&job.name))
        {
            if job.lines.iter().any(|line| {
                names
                    .iter()
                    .any(|name| rx(&format!(r"\b(?:secrets|env)\.{name}\b")).is_match(line))
            }) {
                bail!("token is also read by a non-publishing job; preserve that consumer before migration");
            }
        }
        let mut output = remove_credentials(&lines[..start], &names);
        output.push(lines[start].clone());
        output.extend(body);
        output.extend_from_slice(&lines[end..]);
        let converted = output.join("\n");
        if names
            .iter()
            .any(|name| rx(&format!(r"\bsecrets\.{name}\b")).is_match(&converted))
            || rx(r"\bsecrets\s*\[|\bsecrets:\s*inherit\b").is_match(&converted)
        {
            bail!(
                "remaining legacy or inherited secret references require review before migration"
            );
        }
        files.insert(workflow.name.clone(), format!("{}\n", converted.trim_end()));
    }
    if files.is_empty() {
        bail!("no publishing jobs were found for migration");
    }
    Ok(files)
}
