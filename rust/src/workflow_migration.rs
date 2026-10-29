//! Reviewable conversions of existing publishing jobs and reusable workflow callers.
use crate::{
    model::{Package, Registry},
    publishers::{
        executable_lines, job_publishes, parse_workflow, registry_token_secrets, Job,
        ParsedWorkflow,
    },
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
fn guard(step: &[String], level: usize) -> Vec<String> {
    step.iter()
        .position(|line| indent(line) == level + 2 && line.trim_start().starts_with("if:"))
        .map_or_else(Vec::new, |i| {
            step[i..end_of(step, i)]
                .iter()
                .filter(|line| !line.trim().is_empty())
                .cloned()
                .collect()
        })
}
fn consumer(lines: &[String], registry: Registry) -> bool {
    let names = keys(registry);
    lines.iter().any(|line| {
        names
            .iter()
            .any(|name| rx(&format!(r"^\s*{name}:")).is_match(line))
    }) || executable_lines(lines).iter().any(|line| {
        names.iter().any(|name| line.contains(name))
            || (registry == Registry::CratesIo
                && rx("preflight.*(?:crates|cargo)|(?:crates|cargo).*preflight").is_match(line))
    })
}
pub(crate) fn fallback_steps(
    step: &[String],
    registry: Registry,
    level: usize,
    index: usize,
) -> Vec<Vec<String>> {
    let prefix = " ".repeat(level + 2);
    let names = keys(registry);
    let id = step
        .iter()
        .find_map(|line| {
            rx(r"^\s*(?:-\s*)?id:\s*([\w-]+)")
                .captures(line)
                .map(|c| c[1].to_owned())
        })
        .unwrap_or_else(|| format!("prm_oidc_{}", index + 1));
    let mut remove = names.clone();
    if registry == Registry::PyPi {
        remove.extend(["password".into(), "user".into()]);
    }
    let mut oidc = remove_credentials(step, &remove);
    oidc.retain(|line| !line.trim_start().starts_with("continue-on-error:"));
    if !step
        .iter()
        .any(|line| rx(r"^\s*(?:-\s*)?id:").is_match(line))
    {
        oidc.push(format!("{prefix}id: {id}"));
    }
    oidc.push(format!("{prefix}continue-on-error: true"));
    if let Some(line) = oidc
        .iter_mut()
        .find(|line| rx(r"^\s*(?:-\s*)?name:").is_match(line))
    {
        append_title(line, "OIDC");
    } else {
        oidc.push(format!("{prefix}name: Publish through OIDC"));
    }
    let mut fallback = step.to_vec();
    if rx(r"^\s*-\s+(?:if|id|continue-on-error):").is_match(&fallback[0]) {
        fallback[0] = fallback[0].replacen("- ", "  ", 1);
        if let Some(next) = (1..fallback.len()).find(|&i| indent(&fallback[i]) == level + 2) {
            fallback[next] = format!("{}- {}", " ".repeat(level), fallback[next].trim_start());
        }
    }
    for i in (0..fallback.len()).rev() {
        if rx(r"^\s*(?:if|id|continue-on-error):").is_match(&fallback[i]) {
            let end = end_of(&fallback, i);
            fallback.drain(i..end);
        }
    }
    if let Some(line) = fallback
        .iter_mut()
        .find(|line| rx(r"^\s*(?:-\s*)?name:").is_match(line))
    {
        append_title(line, "token fallback");
    } else {
        fallback.push(format!("{prefix}name: Publish with token fallback"));
    }
    fallback.extend([
        format!("{prefix}if: steps.{id}.outcome == 'failure'"),
        format!("{prefix}id: {id}_fallback"),
    ]);
    if registry == Registry::PyPi {
        fallback = remove_credentials(
            &fallback,
            &BTreeSet::from(["password".into(), "user".into()]),
        );
        let password =
            format!("{prefix}  password: ${{{{ secrets.PYPI_API_TOKEN || secrets.PYPI_TOKEN }}}}");
        if let Some(i) = fallback.iter().position(|line| line.trim() == "with:") {
            fallback.insert(i + 1, password);
        } else {
            fallback.extend([format!("{prefix}with:"), password]);
        }
    } else {
        let mut env_names = names.clone();
        env_names.extend(["NODE_AUTH_TOKEN".into(), "NPM_CONFIG_PROVENANCE".into()]);
        oidc = remove_credentials(&oidc, &env_names);
        fallback = remove_credentials(&fallback, &env_names);
        let mut entries = names;
        entries.insert("NODE_AUTH_TOKEN".into());
        for name in entries {
            oidc = with_env(oidc, &name, "''", level);
            fallback = with_env(
                fallback,
                &name,
                "${{ secrets.NPM_TOKEN || secrets.NODE_AUTH_TOKEN || secrets.NPM_AUTH_TOKEN }}",
                level,
            );
        }
        oidc = with_env(oidc, "NPM_CONFIG_PROVENANCE", "'true'", level);
        fallback = with_env(fallback, "NPM_CONFIG_PROVENANCE", "'true'", level);
    }
    fallback.insert(0, String::new());
    vec![oidc, fallback]
}
fn append_title(line: &mut String, suffix: &str) {
    let (_, value) = line.split_once("name:").expect("step name");
    let value = value.trim();
    let quote = value.chars().next().unwrap_or_default();
    if matches!(quote, '\'' | '"') && value.ends_with(quote) {
        line.pop();
        line.push_str(" (");
        line.push_str(suffix);
        line.push(')');
        line.push(quote);
    } else {
        line.push_str(" (");
        line.push_str(suffix);
        line.push(')');
    }
}
fn convert_job(
    root: &Path,
    parsed: &ParsedWorkflow,
    lines: &[String],
    registry: Registry,
) -> Result<Vec<String>> {
    let text = parsed
        .header
        .iter()
        .chain(lines)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if crate::publishing_state::policy(
        &text,
        registry,
        rx(r"id-token:\s*write|permissions:\s*write-all").is_match(&text),
    ) == "oidc-with-token-fallback"
    {
        return Ok(lines.to_vec());
    }
    let names = keys(registry);
    if lines
        .iter()
        .any(|line| rx(r"^\s*env:\s*[^\s#]").is_match(line))
    {
        bail!("expand inline environment mappings before migration");
    }
    let level = lines
        .iter()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| indent(line))
        .min()
        .context("empty publishing job")?;
    if called(lines).is_some() {
        return grant_job(lines.to_vec(), level, parsed);
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
    let prefix = " ".repeat(step_level);
    let mut edits = Vec::new();
    let mut auths: Vec<(Vec<String>, String)> = Vec::new();
    let mut first = None;
    for (index, &position) in positions.iter().enumerate() {
        let step = &lines[position..positions.get(index + 1).copied().unwrap_or(end)];
        if registry == Registry::CratesIo
            && step
                .iter()
                .any(|line| line.contains("rust-lang/crates-io-auth-action@"))
        {
            continue;
        }
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
        let mut updated = step.to_vec();
        if publishes || consumer(step, registry) {
            first.get_or_insert(edits.len());
            if registry == Registry::CratesIo {
                let condition = guard(step, step_level);
                let id = if let Some((_, id)) = auths.iter().find(|(guard, _)| *guard == condition)
                {
                    id.clone()
                } else {
                    let id = if auths.is_empty() {
                        "prm_auth".into()
                    } else {
                        format!("prm_auth_{}", auths.len() + 1)
                    };
                    let mut auth = vec![
                        String::new(),
                        format!("{prefix}- name: Authenticate with crates.io (trusted publishing)"),
                        format!("{prefix}  uses: {CRATES_AUTH_ACTION} # v1.0.5"),
                        format!("{prefix}  id: {id}"),
                        format!("{prefix}  continue-on-error: true"),
                    ];
                    auth.extend(condition.clone());
                    auth.push(String::new());
                    edits.push(auth);
                    auths.push((condition, id.clone()));
                    id
                };
                let fallback = format!("${{{{ steps.{id}.outputs.token || secrets.CARGO_REGISTRY_TOKEN || secrets.CARGO_TOKEN }}}}");
                let mut used: BTreeSet<_> = names
                    .iter()
                    .filter(|name| {
                        parsed
                            .header
                            .iter()
                            .chain(&lines[..start])
                            .chain(step)
                            .any(|line| rx(&format!(r"\b(?:env\.)?{name}\b")).is_match(line))
                    })
                    .cloned()
                    .collect();
                used.insert("CARGO_REGISTRY_TOKEN".into());
                updated = remove_credentials(&updated,&names).into_iter().map(|line| rx(r#"\s+--token(?:=|\s+)(?:["']?\$\{\{\s*secrets\.[\w]+\s*\}\}["']?|["']?\$\{?(?:CARGO_TOKEN|CARGO_REGISTRY_TOKEN)\}?["']?)"#).replace_all(&line,"").into_owned()).collect();
                for name in &used {
                    updated = with_env(updated, name, &fallback, step_level);
                }
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
                let upload = run.and_then(|line|pattern.captures(line)).filter(|c| !c[2].split('/').any(|part|part=="..") && !step.iter().any(|line|line.trim_start().starts_with("working-directory:"))).context("PyPI migration requires a simple twine upload DIRECTORY/* without a working-directory, or a pypa publishing action")?;
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
                    format!("{prefix}  with:"),
                    format!("{prefix}    packages-dir: '{}/'", &upload[2]),
                ]);
            }
        }
        if publishes && matches!(registry, Registry::Npm | Registry::PyPi) {
            edits.extend(fallback_steps(&updated, registry, step_level, index));
        } else {
            edits.push(updated);
        }
    }
    let first = first.context("cannot locate publishing step for migration")?;
    if registry == Registry::Npm {
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
    let mut result = lines[..start].to_vec();
    result.push(lines[start].clone());
    result.extend(edits.into_iter().flatten());
    result.extend_from_slice(&lines[end..]);
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
        let token_check = registry == Registry::CratesIo
            && consumer(&job.lines, registry)
            && executable_lines(&job.lines)
                .iter()
                .any(|line| rx("preflight|CARGO_(?:REGISTRY_)?TOKEN").is_match(line));
        if publishes || token_check {
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
        let mut header = lines[..start].to_vec();
        for i in 0..header.len() {
            let key = rx(r"^\s*([\w-]+):\s*$")
                .captures(&header[i])
                .map(|c| c[1].to_owned());
            if key.is_some_and(|key| names.contains(&key)) {
                let end = end_of(&header, i);
                for line in &mut header[i + 1..end] {
                    *line = rx(r"required:\s*true")
                        .replace(line, "required: false")
                        .into_owned();
                }
            }
        }
        if let Some(index) = header.iter().position(|line| {
            rx(r#"^["']?on["']?:\s*(?:workflow_call|\[workflow_call\])\s*$"#).is_match(line)
        }) {
            header.splice(index..=index, ["on:".into(), "  workflow_call:".into()]);
        }
        if let Some(call) = header
            .iter()
            .position(|line| rx(r"^\s+workflow_call:\s*$").is_match(line))
        {
            let call_level = indent(&header[call]);
            let call_end = end_of(&header, call);
            let existing_secrets = (call + 1..call_end).find(|&i| {
                indent(&header[i]) == call_level + 2 && header[i].trim().starts_with("secrets:")
            });
            let secrets = existing_secrets.unwrap_or_else(|| {
                header.insert(call + 1, format!("{}secrets:", " ".repeat(call_level + 2)));
                call + 1
            });
            let existing = header[secrets + 1..end_of(&header, secrets)].to_vec();
            for name in &names {
                if !existing
                    .iter()
                    .any(|line| rx(&format!(r"^\s*{name}:")).is_match(line))
                {
                    let insertion = secrets + 1;
                    header.splice(
                        insertion..insertion,
                        [
                            format!("{}{name}:", " ".repeat(call_level + 4)),
                            format!("{}required: false", " ".repeat(call_level + 6)),
                        ],
                    );
                }
            }
        }
        let mut output = header;
        output.push(lines[start].clone());
        output.extend(body);
        output.extend_from_slice(&lines[end..]);
        let converted = output.join("\n");
        files.insert(workflow.name.clone(), format!("{}\n", converted.trim_end()));
    }
    if files.is_empty() {
        bail!("no publishing jobs were found for migration");
    }
    Ok(files)
}
