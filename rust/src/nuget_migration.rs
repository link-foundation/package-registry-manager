//! Conservative replacement of a direct push, preserving validation and artifact identity.
use crate::{model::Package, Registry};
use anyhow::{bail, Context, Result};
use std::fmt::Write as _;

pub fn convert_step(step: &[String], packages: &[Package], index: usize) -> Result<Vec<String>> {
    let packages: Vec<_> = packages
        .iter()
        .filter(|p| p.registry == Registry::NuGet)
        .collect();
    if packages.len() != 1 {
        bail!("NuGet migration requires a package-specific publishing step; ambiguous multi-package globs need explicit review");
    }
    let run = step
        .iter()
        .find(|line| {
            regex::Regex::new(r"^\s*-?\s*run:")
                .expect("regex")
                .is_match(line)
        })
        .context("NuGet script requires a reviewed artifact-only publishing command")?;
    let re = regex::Regex::new(
        r#"^\s*-?\s*run:\s*dotnet\s+nuget\s+push\s+(?:"([^"\n]+)"|'([^'\n]+)'|([^\s]+))\s*(.*)$"#,
    )?;
    let push = re.captures(run).context("NuGet script or multiline push requires a reviewed artifact-only publishing command; validation/build commands cannot be retried")?;
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
