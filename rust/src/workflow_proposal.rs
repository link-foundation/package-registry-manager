//! Reviewable workflow jobs and a branch/PR offer for missing CI publication.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use command_stream::StreamingRunner;

use crate::auth_urls::resolve_program;
use crate::model::{Inspection, Package, Registry, SetupPlan};
use crate::plan::package_directory;
use crate::publishers::parse_workflow;
use crate::workflows::{read_workflows, Workflow};

/// Proposed contents, generated before any repository mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowProposal {
    /// Filename in .github/workflows.
    pub workflow: String,
    /// Complete updated YAML for review.
    pub contents: String,
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn job(
    package: &Package,
    index: usize,
    environment: Option<&str>,
    producer: Option<&str>,
) -> Result<String> {
    if package.manifest.split('/').any(|part| {
        part.is_empty()
            || part == "."
            || part == ".."
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "_@.-".contains(ch))
    }) {
        bail!("unsafe manifest path for publishing job");
    }
    let directory = package_directory(&package.manifest);
    let mut lines = vec![
        format!("  prm-publish-{}-{}:", package.registry, index+1),
        "    if: github.event_name == 'release' || ((github.event_name == 'push' || github.event_name == 'workflow_dispatch') && github.ref == format('refs/heads/{0}', github.event.repository.default_branch))".into(),
        "    runs-on: ubuntu-latest".into(), "    timeout-minutes: 15".into(),
        "    permissions:".into(), "      contents: read".into(), "      id-token: write".into(),
    ];
    if let Some(producer) = producer {
        lines[1] = format!(
            "    if: ({}) && needs.{producer}.outputs.published_version != ''",
            lines[1].trim_start_matches("    if: ")
        );
        lines.extend([
            format!("    needs: {producer}"),
            "    env:".into(),
            format!("      RELEASE_VERSION: ${{{{ needs.{producer}.outputs.published_version }}}}"),
        ]);
    }
    lines.extend([
        "    concurrency:".into(),
        format!(
            "      group: prm-publish-{}-{}-${{{{ github.ref }}}}",
            package.registry,
            index + 1
        ),
        "      cancel-in-progress: false".into(),
    ]);
    if let Some(environment) = environment {
        lines.push(format!("    environment: {}", quote(environment)));
    }
    lines.extend([
        "    defaults:".into(),
        "      run:".into(),
        format!("        working-directory: {}", quote(directory)),
        "    steps:".into(),
        "      - uses: actions/checkout@v6".into(),
        "        with:".into(),
        "          persist-credentials: false".into(),
    ]);
    if producer.is_some() {
        lines.push("          ref: ${{ github.event.repository.default_branch }}".into());
    }
    let mut steps: Vec<String> = match package.registry {
        Registry::Npm => [
            "      - uses: actions/setup-node@v6",
            "        with:",
            "          node-version: '22'",
            "      - run: npm install --global npm@^11",
            "      - run: npm install",
            "      - run: npm run build --if-present",
            "      - run: npm publish --provenance --access public",
            "        if: steps.version-check.outputs.publish == 'true'",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        Registry::CratesIo => [
            "      - uses: dtolnay/rust-toolchain@stable",
            "      - uses: rust-lang/crates-io-auth-action@v1",
            "        if: steps.version-check.outputs.publish == 'true'",
            "        id: auth",
            "      - run: cargo publish",
            "        if: steps.version-check.outputs.publish == 'true'",
            "        env:",
            "          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        Registry::PyPi => vec![
            "      - uses: actions/setup-python@v6".into(),
            "        with:".into(),
            if package.manifest.ends_with("pyproject.toml") && package.requires_python.is_some() {
                format!(
                    "          python-version-file: {}",
                    quote(&package.manifest)
                )
            } else {
                "          python-version: '3.x'".into()
            },
            "      - run: python -m pip install build".into(),
            "      - run: python -m build".into(),
            "      - uses: pypa/gh-action-pypi-publish@release/v1".into(),
            "        with:".into(),
            "          skip-existing: true".into(),
            format!(
                "          packages-dir: {}",
                quote(&format!(
                    "{}dist/",
                    if directory == "." {
                        String::new()
                    } else {
                        format!("{directory}/")
                    }
                ))
            ),
        ],
        registry => bail!("no publishing job template for {registry}"),
    };
    if matches!(package.registry, Registry::Npm | Registry::CratesIo) {
        let before = steps
            .iter()
            .position(|line| {
                line.contains(if package.registry == Registry::Npm {
                    "- run: npm publish"
                } else {
                    "- uses: rust-lang/crates-io-auth-action"
                })
            })
            .expect("publish step");
        steps.splice(
            before..before,
            crate::version_guard::version_guard(package.registry == Registry::Npm),
        );
    }
    lines.extend(steps);
    Ok(format!("{}\n", lines.join("\n")))
}

/// Generates a proposal, preserving unrelated jobs and YAML.
pub fn workflow_proposal(
    inspection: &Inspection,
    packages: &[Package],
    workflows: &[Workflow],
    override_file: Option<&str>,
    chosen_environment: Option<&str>,
) -> Result<WorkflowProposal> {
    let known: BTreeSet<_> = inspection
        .packages
        .iter()
        .filter(|package| {
            package.publishable && crate::publishers::TRUSTED_REGISTRIES.contains(&package.registry)
        })
        .filter_map(|package| package.workflow.as_deref())
        .collect();
    if override_file.is_none() && known.len() > 1 {
        bail!(
            "publishing uses several files; pass --add-publish-job --workflow <file> to choose the shared workflow"
        );
    }
    let workflow = override_file
        .or_else(|| known.first().copied())
        .or(inspection.repository.release_workflow.as_deref())
        .unwrap_or("release.yml");
    if !regex::Regex::new(r"^[\w.-]+\.ya?ml$")
        .expect("valid regex")
        .is_match(workflow)
    {
        bail!("invalid publishing workflow name");
    }
    let contents = workflows.iter().find(|item| item.name == workflow).map_or("name: Package release\non:\n  workflow_dispatch:\n  release:\n    types: [published]\npermissions:\n  contents: read\njobs:\n", |item| item.contents.as_str());
    let parsed = parse_workflow(contents);
    let producers: Vec<_> = parsed
        .jobs
        .iter()
        .filter(|job| {
            job.lines
                .iter()
                .any(|line| line.trim_start().starts_with("published_version:"))
        })
        .collect();
    if producers.len() > 1 {
        bail!("several jobs expose published_version; choose the release dependency manually");
    }
    let producer = producers.first().map(|job| job.name.as_str());
    let environments: BTreeSet<_> = inspection
        .packages
        .iter()
        .filter(|package| package.workflow.as_deref() == Some(workflow))
        .map(|package| package.environment.as_deref())
        .collect();
    if chosen_environment.is_none() && environments.len() > 1 {
        bail!("publishing jobs use different environments; choose a shared environment with --environment");
    }
    let mut addition = packages
        .iter()
        .enumerate()
        .map(|(index, package)| {
            job(
                package,
                index,
                chosen_environment.or_else(|| environments.first().copied().flatten()),
                producer,
            )
        })
        .collect::<Result<Vec<_>>>()?
        .join("\n");
    let extra = parse_workflow(&format!("jobs:\n{addition}"));
    for item in extra.jobs {
        if parsed
            .jobs
            .iter()
            .any(|existing| existing.name == item.name)
        {
            bail!(
                "publishing job {} already exists; inspect the workflow before adding it again",
                item.name
            );
        }
    }
    let lines: Vec<_> = contents.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim_end() == "jobs:")
        .context("workflow has no editable jobs block; add a publishing job manually")?;
    let end = lines
        .iter()
        .enumerate()
        .find(|(index, line)| *index > start && line.starts_with(|ch: char| !ch.is_whitespace()))
        .map_or(lines.len(), |(index, _)| index);
    let indent = lines[start + 1..end]
        .iter()
        .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map_or(2, |line| line.len() - line.trim_start().len());
    if indent != 2 {
        if indent < 2 {
            bail!("unsupported jobs indentation");
        }
        addition = addition
            .lines()
            .map(|line| {
                if line.is_empty() {
                    String::new()
                } else {
                    format!("{}{line}", " ".repeat(indent - 2))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
    }
    let suffix = if end < lines.len() {
        format!("{}\n", lines[end..].join("\n"))
    } else {
        String::new()
    };
    Ok(WorkflowProposal {
        workflow: workflow.to_owned(),
        contents: format!(
            "{}\n\n{addition}{suffix}",
            lines[..end].join("\n").trim_end()
        ),
    })
}

pub(crate) async fn run(
    repository: &Path,
    program: &str,
    args: &[&str],
    verbose: bool,
) -> Result<String> {
    if verbose {
        eprintln!("+ {program} {}", args.join(" "));
    }
    let output = StreamingRunner::from_argv(resolve_program(program), args)
        .cwd(repository)
        .collect()
        .await?;
    if output.code != 0 {
        bail!("{program} failed: {}{}", output.stdout, output.stderr);
    }
    Ok(output.stdout.to_string().trim().to_owned())
}

/// Explicit choices for the proposed workflow.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkflowOptions<'a> {
    /// Target file, overriding detection.
    pub workflow: Option<&'a str>,
    /// Shared GitHub environment, overriding detected environments.
    pub environment: Option<&'a str>,
}

/// Offers a draft PR, then stops setup until the reviewed workflow is merged.
/// Returns its URL, or None when the user declines.
pub async fn offer_workflow(
    inspection: &Inspection,
    plans: &[&SetupPlan],
    repository: &Path,
    yes: bool,
    verbose: bool,
    options: WorkflowOptions<'_>,
) -> Result<Option<String>> {
    let packages: Vec<_> = plans.iter().map(|plan| plan.package.clone()).collect();
    let proposal = workflow_proposal(
        inspection,
        &packages,
        &read_workflows(repository)?,
        options.workflow,
        options.environment,
    )?;
    println!(
        "Proposed .github/workflows/{}:\n{}",
        proposal.workflow, proposal.contents
    );
    if !yes
        && !crate::setup::is_yes(&crate::setup::prompt(
            "Create a branch and draft pull request with these publishing jobs? [y/N] ",
        )?)
    {
        return Ok(None);
    }
    let owner = inspection
        .repository
        .github_owner
        .as_deref()
        .context("a GitHub repository is required to propose a workflow")?;
    let name = inspection
        .repository
        .github_repository
        .as_deref()
        .context("a GitHub repository is required to propose a workflow")?;
    let slug = format!("{owner}/{name}");
    let remote = run(repository, "git", &["remote", "get-url", "origin"], verbose).await?;
    if ![
        "https://github.com/",
        "git@github.com:",
        "ssh://git@github.com/",
    ]
    .iter()
    .any(|prefix| {
        remote
            .strip_prefix(prefix)
            .is_some_and(|tail| tail == slug || tail == format!("{slug}.git"))
    }) {
        bail!("origin must match the inspected GitHub repository before proposing a workflow");
    }
    let base = run(
        repository,
        "gh",
        &[
            "repo",
            "view",
            &slug,
            "--json",
            "defaultBranchRef",
            "--jq",
            ".defaultBranchRef.name",
        ],
        verbose,
    )
    .await?;
    run(repository, "git", &["fetch", "origin", &base], verbose).await?;
    let branch = format!("prm/publish-jobs-{}", chrono::Utc::now().timestamp_millis());
    let temporary = tempfile::tempdir()?;
    let checkout = temporary.path().join("checkout");
    let checkout_text = checkout.to_string_lossy();
    run(
        repository,
        "git",
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &checkout_text,
            &format!("origin/{base}"),
        ],
        verbose,
    )
    .await?;
    let result = async {
        let current = workflow_proposal(inspection, &packages, &read_workflows(&checkout)?, Some(&proposal.workflow), options.environment)?;
        if current.contents != proposal.contents {
            println!("Updated proposal from {base}:\n{}", current.contents);
            if !yes && !crate::setup::is_yes(&crate::setup::prompt("Use this updated proposal? [y/N] ")?) {
                return Ok(None);
            }
        }
        let file = format!(".github/workflows/{}", current.workflow);
        let target = checkout.join(&file);
        fs::create_dir_all(target.parent().context("workflow directory")?)?;
        let parent = target.parent().context("workflow directory")?.canonicalize()?;
        if !parent.starts_with(checkout.canonicalize()?) || fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            bail!("workflow must stay inside the proposal worktree without symlinks");
        }
        fs::write(target, current.contents)?;
        run(&checkout, "git", &["add", "--", &file], verbose).await?;
        run(&checkout, "git", &["commit", "-m", "Add missing trusted publishing jobs"], verbose).await?;
        run(&checkout, "git", &["push", "origin", &format!("HEAD:refs/heads/{branch}")], verbose).await?;
        let names = plans.iter().map(|plan| format!("{}: {}", plan.registry, plan.package.name)).collect::<Vec<_>>().join(", ");
        let body = format!("Add missing publishing jobs for {names} in {}. Review the build, versioning, triggers and job dependencies before merging. Then re-run setup to attach the trusted publishers to this file.", current.workflow);
        let url = run(&checkout, "gh", &["pr", "create", "--repo", &slug, "--base", &base, "--head", &branch, "--draft", "--title", "Add missing trusted publishing jobs", "--body", &body], verbose).await?;
        println!("Review {url}, merge it, then re-run setup. Trusted publishers have not been attached.");
        Ok(Some(url))
    }.await;
    run(
        repository,
        "git",
        &["worktree", "remove", "--force", &checkout_text],
        verbose,
    )
    .await?;
    result
}
