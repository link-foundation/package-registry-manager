//! Conservative replacement of a direct push, preserving validation and artifact identity.
use crate::{model::Package, Registry};
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::fmt::Write as _;

pub const NUGET_PREREQUISITE_EDIT: &str = "Split the NuGet publish step so one step runs only `dotnet nuget push <artifact glob> --api-key ... --source https://api.nuget.org/v3/index.json` (optionally behind a `should_publish` guard, `${A:-$B}` key selection, a key-presence check and `published=` outputs); pack, tests and any other command must stay in earlier steps.";

/// A migration that needs a reviewed workflow edit first; `code` is
/// `nuget-prerequisite-edit` and `prerequisite` the exact edit to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrerequisiteError {
    pub message: String,
    pub code: &'static str,
    pub prerequisite: &'static str,
}

impl std::fmt::Display for PrerequisiteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PrerequisiteError {}

fn prerequisite(reason: &str) -> anyhow::Error {
    PrerequisiteError {
        message: format!("{reason}. Prerequisite edit: {NUGET_PREREQUISITE_EDIT}"),
        code: "nuget-prerequisite-edit",
        prerequisite: NUGET_PREREQUISITE_EDIT,
    }
    .into()
}

/// The recognized parts of a guarded multiline nuget.org push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardedPush {
    pub guard: Option<String>,
    pub glob: String,
    pub options: String,
    pub start: usize,
    pub level: usize,
}

fn indent(line: &str) -> Option<usize> {
    line.find(|c: char| !c.is_whitespace())
}

const UNREVIEWED: &str = "NuGet script or multiline push requires a reviewed artifact-only publishing command; validation/build commands cannot be retried";

/// Recognize the guarded multiline push used by links-notation style workflows.
///
/// Every line must be the guard, key selection, presence check, notices,
/// `published=` outputs or one nuget.org push; anything else is not migrated.
pub fn guarded_nuget_push(step: &[String]) -> Result<Option<GuardedPush>> {
    let opener = Regex::new(r"^\s*-?\s*run:\s*\|[-+]?\s*$")?;
    let Some(start) = step.iter().position(|line| opener.is_match(line)) else {
        return Ok(None);
    };
    let level = indent(&step[start]).unwrap_or_default();
    let mut body = Vec::new();
    for line in &step[start + 1..] {
        if !line.trim().is_empty() && indent(line).is_some_and(|value| value <= level) {
            break;
        }
        body.push(line.trim());
    }
    let joined = Regex::new(r"\\\n\s*")?
        .replace_all(&body.join("\n"), " ")
        .into_owned();
    let guard_open = Regex::new(r#"^if \[ "\$\{\{\s*([\w.-]+)\s*\}\}" != "true" \]; then$"#)?;
    let missing_open = Regex::new(r#"^if \[ -z "\$\w+" \]; then$"#)?;
    let allowed = [
        Regex::new(r"^set -[euxo pipefail]+$")?,
        Regex::new(r#"^\w+="\$\{\w+:-\$\w+\}"$"#)?,
        Regex::new(r#"^echo "[^"`$]*(?:\$\{\{[^}]*\}\}[^"`$]*)*"(?: >> "\$GITHUB_OUTPUT")?$"#)?,
    ];
    let push_re = Regex::new(r#"^dotnet nuget push (?:"([^"]+)"|'([^']+)'|(\S+))(.*)$"#)?;
    let (mut guard, mut push, mut block) = (None, None, None);
    for line in joined
        .split('\n')
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        if let Some(opened) = guard_open.captures(line) {
            if block.is_none() && guard.is_none() {
                guard = Some(opened[1].to_owned());
                block = Some("guard");
                continue;
            }
        }
        if missing_open.is_match(line) && block.is_none() {
            block = Some("missing");
            continue;
        }
        if line == "fi" && block.is_some() {
            block = None;
            continue;
        }
        if (block == Some("guard") && line == "exit 0")
            || (block == Some("missing") && line == "exit 1")
            || allowed.iter().any(|re| re.is_match(line))
        {
            continue;
        }
        if let Some(command) = push_re.captures(line) {
            if push.is_none() && block.is_none() {
                let glob = (1..=3)
                    .find_map(|i| command.get(i))
                    .map_or("", |m| m.as_str());
                push = Some((glob.to_owned(), command[4].to_owned()));
                continue;
            }
        }
        return Err(prerequisite(&format!(
            "NuGet publish script contains an unreviewed command: {line}"
        )));
    }
    let Some((glob, options)) = push.filter(|_| block.is_none()) else {
        return Err(prerequisite(
            "NuGet multiline step has no single recognizable artifact push",
        ));
    };
    Ok(Some(GuardedPush {
        guard,
        glob,
        options,
        start,
        level,
    }))
}

/// `--source` followed by anything but nuget.org (the JS negative lookahead).
fn foreign_source(options: &str) -> bool {
    Regex::new(r"--source\s+")
        .expect("regex")
        .find_iter(options)
        .any(|found| {
            let rest = &options[found.end()..];
            let rest = rest.strip_prefix(['"', '\'']).unwrap_or(rest);
            !rest.starts_with("https://api.nuget.org/v3/index.json")
        })
}

pub fn convert_step(step: &[String], packages: &[Package], index: usize) -> Result<Vec<String>> {
    let packages: Vec<_> = packages
        .iter()
        .filter(|p| p.registry == Registry::NuGet)
        .collect();
    if packages.len() != 1 {
        bail!("NuGet migration requires a package-specific publishing step; ambiguous multi-package globs need explicit review");
    }
    if let Some(guarded) = guarded_nuget_push(step)? {
        return convert_guarded(step, packages[0], index, &guarded);
    }
    let run = step
        .iter()
        .find(|line| {
            regex::Regex::new(r"^\s*-?\s*run:")
                .expect("regex")
                .is_match(line)
        })
        .ok_or_else(|| prerequisite(UNREVIEWED))?;
    let re = regex::Regex::new(
        r#"^\s*-?\s*run:\s*dotnet\s+nuget\s+push\s+(?:"([^"\n]+)"|'([^'\n]+)'|([^\s]+))\s*(.*)$"#,
    )?;
    let push = re.captures(run).ok_or_else(|| prerequisite(UNREVIEWED))?;
    if [1, 2, 3, 4]
        .iter()
        .filter_map(|i| push.get(*i))
        .any(|m| m.as_str().contains([';', '\n', '`']) || m.as_str().contains("$("))
    {
        return Err(prerequisite(UNREVIEWED));
    }
    let glob = (1..=3)
        .find_map(|i| push.get(i))
        .context("artifact path")?
        .as_str();
    if !regex::Regex::new(r"^[\w./*${}-]+$")?.is_match(glob)
        || glob.contains("${{")
        || run.contains([';', '`'])
        || run.contains("$(")
    {
        bail!("NuGet artifact glob must resolve from job-local environment variables");
    }
    if push[4].contains("--source") && !push[4].contains("https://api.nuget.org/v3/index.json") {
        bail!("NuGet policy migration requires the explicit nuget.org destination");
    }
    let mut package = packages[0].clone();
    for secret in regex::Regex::new(r"secrets\.([\w]+)")?.captures_iter(&step.join("\n")) {
        package.token_secrets.push(secret[1].to_owned());
    }
    let prefix = " ".repeat(
        step[0]
            .find(|c: char| !c.is_whitespace())
            .unwrap_or_default(),
    );
    let mut lines: Vec<_> = crate::nuget_workflow::steps(&package)?
        .into_iter()
        .skip(5)
        .map(|line| {
            format!("{prefix}{}", line.get(6..).unwrap_or_default())
                .replace("prm_nuget_login", &format!("prm_nuget_login_{index}"))
        })
        .collect();
    let env = lines
        .iter()
        .position(|line| line.contains("PRM_PACKAGE_ID:"))
        .context("package env")?;
    lines.insert(env + 1, format!("{prefix}    PRM_ARTIFACT_GLOB: '{glob}'"));
    if let Some(condition) = step
        .iter()
        .find(|line| line.trim_start().starts_with("if:"))
    {
        let expression = condition
            .trim()
            .trim_start_matches("if:")
            .trim()
            .trim_start_matches("${{")
            .trim_end_matches("}}")
            .trim();
        let login = lines
            .iter_mut()
            .find(|line| line.contains("if: vars.NUGET_USER"))
            .context("login condition")?;
        write!(login, " && ({expression})")?;
        let publish = lines
            .iter()
            .position(|line| line.contains("- name: Publish validated NuGet"))
            .context("publish step")?;
        lines.insert(publish + 1, format!("{prefix}  if: {expression}"));
    }
    if let Some(directory) = step
        .iter()
        .find(|line| line.trim_start().starts_with("working-directory:"))
    {
        let publish = lines
            .iter()
            .position(|line| line.contains("- name: Publish validated NuGet"))
            .context("publish step")?;
        lines.insert(publish + 1, format!("{prefix}  {}", directory.trim()));
    }
    if let Some(position) = step
        .iter()
        .position(|line| line.trim_start().starts_with("env:"))
    {
        if step[position].trim() != "env:" {
            bail!("NuGet migration requires a resolved step environment mapping");
        }
        let level = step[position]
            .find(|c: char| !c.is_whitespace())
            .unwrap_or_default();
        let mut entries = Vec::new();
        for line in &step[position + 1..] {
            if !line.trim().is_empty()
                && line.find(|c: char| !c.is_whitespace()).unwrap_or_default() <= level
            {
                break;
            }
            if line.trim_start().starts_with("PRM_") {
                bail!("NuGet migration cannot overwrite a publication controller environment");
            }
            entries.push(line.clone());
        }
        let env = lines
            .iter()
            .position(|line| line.trim() == "env:")
            .context("publication environment")?;
        lines.splice((env + 1)..=env, entries);
    }
    Ok(lines)
}

/// Guard -> OIDC login -> controller push of the same artifacts -> original outputs.
fn convert_guarded(
    step: &[String],
    package: &Package,
    index: usize,
    guarded: &GuardedPush,
) -> Result<Vec<String>> {
    if foreign_source(&guarded.options) {
        return Err(prerequisite(
            "NuGet policy migration requires the explicit nuget.org destination",
        ));
    }
    if !Regex::new(r"^[\w./*-]+$")?.is_match(&guarded.glob) {
        return Err(prerequisite(
            "NuGet artifact glob must be a literal job-local path",
        ));
    }
    let prefix = " ".repeat(indent(&step[0]).unwrap_or_default());
    let header = &step[..guarded.start];
    let name = Regex::new(r"-\s*name:\s*(.+)$")?
        .captures(&step[0])
        .map_or_else(|| "Publish NuGet package".to_owned(), |m| m[1].to_owned());
    let id_pattern = Regex::new(r"^\s*id:\s*(\S+)")?;
    let id = header
        .iter()
        .find_map(|line| id_pattern.captures(line).map(|m| m[1].to_owned()));
    let condition_pattern = Regex::new(r"^\s*if:\s*(.+)$")?;
    let condition = header.iter().find_map(|line| {
        condition_pattern.captures(line).map(|m| {
            Regex::new(r"^\$\{\{\s*|\s*\}\}$")
                .expect("regex")
                .replace_all(&m[1], "")
                .into_owned()
        })
    });
    let mut package = package.clone();
    let mut secrets: Vec<String> = Regex::new(r"secrets\.(\w+)")?
        .captures_iter(&step.join("\n"))
        .map(|m| m[1].to_owned())
        .collect();
    secrets.append(&mut package.token_secrets);
    package.token_secrets = secrets;
    let gates = [
        condition.clone(),
        guarded
            .guard
            .as_ref()
            .map(|guard| format!("{guard} == 'true'")),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .map(|part| format!("({part})"))
    .collect::<Vec<_>>()
    .join(" && ");
    let publish_id = format!("prm_nuget_publish_{index}");
    let mut lines: Vec<String> = crate::nuget_workflow::steps(&package)?
        .into_iter()
        .skip(5)
        .map(|line| {
            format!("{prefix}{}", line.get(6..).unwrap_or_default())
                .replace("prm_nuget_login", &format!("prm_nuget_login_{index}"))
        })
        .collect();
    let env = lines
        .iter()
        .position(|line| line.contains("PRM_PACKAGE_ID:"))
        .context("package env")?;
    lines.insert(
        env + 1,
        format!(
            "{prefix}    PRM_ARTIFACT_GLOB: '{}'",
            guarded.glob.replace('\'', "''")
        ),
    );
    if !gates.is_empty() {
        let login = lines
            .iter_mut()
            .find(|line| line.contains("if: vars.NUGET_USER"))
            .context("login condition")?;
        write!(login, " && {gates}")?;
    }
    let publish = lines
        .iter()
        .position(|line| line.contains("- name: Publish validated NuGet"))
        .context("publish step")?;
    let mut inserted = vec![format!("{prefix}  id: {publish_id}")];
    if !gates.is_empty() {
        inserted.push(format!("{prefix}  if: {gates}"));
    }
    lines.splice(publish + 1..=publish, inserted);
    lines.push(format!("{prefix}- name: {name}"));
    if let Some(id) = id {
        lines.push(format!("{prefix}  id: {id}"));
    }
    lines.push(format!(
        "{prefix}  if: ${{{{ !cancelled(){} }}}}",
        condition.map_or_else(String::new, |condition| format!(" && ({condition})"))
    ));
    lines.push(format!("{prefix}  env:"));
    lines.push(format!(
        "{prefix}    PRM_SHOULD_PUBLISH: {}",
        guarded.guard.as_ref().map_or_else(
            || "'true'".to_owned(),
            |guard| format!("${{{{ {guard} }}}}")
        )
    ));
    lines.push(format!(
        "{prefix}    PRM_PUBLISH_OUTCOME: ${{{{ steps.{publish_id}.outcome }}}}"
    ));
    for line in [
        "  run: |",
        "    if [ \"$PRM_SHOULD_PUBLISH\" != \"true\" ]; then",
        "      echo \"published=skipped\" >> \"$GITHUB_OUTPUT\"",
        "      exit 0",
        "    fi",
        "    if [ \"$PRM_PUBLISH_OUTCOME\" = \"success\" ]; then",
        "      echo \"published=true\" >> \"$GITHUB_OUTPUT\"",
        "    else",
        "      echo \"published=failed\" >> \"$GITHUB_OUTPUT\"",
        "      exit 1",
        "    fi",
    ] {
        lines.push(format!("{prefix}{line}"));
    }
    Ok(lines)
}
