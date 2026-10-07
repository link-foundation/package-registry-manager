//! Job-aware detection of the workflow that publishes to a registry, so the
//! trusted publisher names the workflow file CI actually publishes from.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value as JsonValue;

use crate::model::Registry;
use crate::source_code::{command_position, strip_comments};
use crate::workflows::Workflow;

/// Registries whose trusted publisher is bound to a workflow file.
pub const TRUSTED_REGISTRIES: [Registry; 6] = [
    Registry::Npm,
    Registry::CratesIo,
    Registry::PyPi,
    Registry::RubyGems,
    Registry::NuGet,
    Registry::Jsr,
];

// Scripts usually pass the subcommand as a separate argument, as in
// `spawnSync("npm", ["publish"])` or `Command::new("cargo")` followed by
// `.arg("publish")` within the next few lines.
const SCRIPT_WINDOW: usize = 3;
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static pattern must compile")
}

static NPM_JOB: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"\b(?:npm|pnpm)\s+(?:-r\s+|--recursive\s+)?(?:stage\s+)?publish\b|\byarn\s+npm\s+publish\b|\bchangeset\s+publish\b|changesets/action/publish@|JS-DevTools/npm-publish",
    )
});
static CRATES_JOB: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"\bcargo\s+(?:\+\S+\s+)?(?:workspaces\s+|ws\s+)?publish\b|katyo/publish-crates")
});
static PYPI_JOB: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"pypa/gh-action-pypi-publish|\b(?:python(?:3(?:\.\d+)?)?\s+-m\s+)?twine\s+upload\b|\b(?:uv|poetry|hatch|flit|pdm)\s+publish\b",
    )
});
static NPM_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:npm|pnpm|yarn)\b"));
static CARGO_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\bcargo\b"));
static PYPI_PROGRAM: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\b(?:twine|uv|poetry|hatch|flit|pdm)\b"));
static PUBLISH_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| regex(r#"["'`]publish["'`]"#));
static UPLOAD_ARGUMENT: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"["'`](?:upload|publish)["'`]"#));
static SCRIPT_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"^(?:\.{1,2}/)?[\w@.-]+(?:/[\w@.-]+)*\.(?:mjs|cjs|js|mts|cts|ts|rs|sh|bash|py)$")
});
static REFERENCE_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| regex(r#"[\s"'=;()|&]+"#));
static RUN_SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"\bnpm\s+run(?:-script)?\s+([\w:.-]+)|\b(?:pnpm|yarn|bun)\s+(?:run\s+)?([\w:.-]+)")
});
// `changesets/action` runs its `publish` (v1) or `publish-script` (v2) input.
static CHANGESETS_PUBLISH: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s*publish(?:-script)?\s*:\s*(.+?)\s*$"));
static LOCAL_WORKFLOW: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"^\s*uses\s*:\s*["']?\./\.github/workflows/([\w.-]+\.ya?ml)"#));
static JOB_HEADER: LazyLock<Regex> = LazyLock::new(|| regex(r#"^\s*["']?([\w-]+)["']?\s*:\s*$"#));
static WORKING_DIRECTORY: LazyLock<Regex> =
    LazyLock::new(|| regex(r"^\s*working-directory\s*:\s*(.+?)\s*$"));
static INLINE_ID_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"\bid-token\s*:\s*['"]?write\b"#));
static NESTED_ID_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"^\s*id-token\s*:\s*['"]?write\b"#));
static INLINE_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"\bname\s*:\s*([^,}]+)"));
static NESTED_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*name\s*:\s*(.+)$"));
static TRAILING_COMMENT: LazyLock<Regex> = LazyLock::new(|| regex(r"\s+#.*$"));

static RUBYGEMS_JOB: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\bgem\s+push\b|rubygems/release-gem"));
static NUGET_JOB: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\bdotnet\s+nuget\s+push\b|\bnuget\s+push\b"));
static JSR_JOB: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:deno|jsr)\s+publish\b"));
static MAVEN_JOB: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\bmvn\s+(?:--?[^\s]+\s+)*deploy\b|\bgradle(?:w)?\s+publish\b"));
static VSCE_JOB: LazyLock<Regex> = LazyLock::new(|| regex(r"\bvsce\s+publish\b"));
static OVSX_JOB: LazyLock<Regex> = LazyLock::new(|| regex(r"\bovsx\s+publish\b"));
static CHROME_JOB: LazyLock<Regex> =
    LazyLock::new(|| regex(r"chrome-webstore-upload|chrome-web-store|chromewebstore"));
static DOCKER_JOB: LazyLock<Regex> =
    LazyLock::new(|| regex(r"docker/login-action|\bdocker\s+(?:login|push)\b"));
static GEM_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\bgem\b"));
static NUGET_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:dotnet|nuget)\b"));
static JSR_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:deno|jsr)\b"));
static VSCE_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\bvsce\b"));
static OVSX_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\bovsx\b"));
static MAVEN_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:mvn|gradle)\b"));
static PUSH_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| regex(r#"["'`]push["'`]"#));
static DEPLOY_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| regex(r#"["'`]deploy["'`]"#));

fn job_pattern(registry: Registry) -> Option<&'static Regex> {
    match registry {
        Registry::Npm => Some(&NPM_JOB),
        Registry::CratesIo => Some(&CRATES_JOB),
        Registry::PyPi => Some(&PYPI_JOB),
        Registry::RubyGems => Some(&RUBYGEMS_JOB),
        Registry::NuGet => Some(&NUGET_JOB),
        Registry::Jsr => Some(&JSR_JOB),
        Registry::MavenCentral => Some(&MAVEN_JOB),
        Registry::VsCodeMarketplace => Some(&VSCE_JOB),
        Registry::OpenVsx => Some(&OVSX_JOB),
        Registry::ChromeWebStore => Some(&CHROME_JOB),
        Registry::DockerHub => Some(&DOCKER_JOB),
        _ => None,
    }
}

fn script_pattern(registry: Registry) -> Option<(&'static Regex, &'static Regex)> {
    match registry {
        Registry::Npm => Some((&NPM_PROGRAM, &PUBLISH_ARGUMENT)),
        Registry::CratesIo => Some((&CARGO_PROGRAM, &PUBLISH_ARGUMENT)),
        Registry::PyPi => Some((&PYPI_PROGRAM, &UPLOAD_ARGUMENT)),
        Registry::RubyGems => Some((&GEM_PROGRAM, &PUSH_ARGUMENT)),
        Registry::NuGet => Some((&NUGET_PROGRAM, &PUSH_ARGUMENT)),
        Registry::Jsr => Some((&JSR_PROGRAM, &PUBLISH_ARGUMENT)),
        Registry::MavenCentral => Some((&MAVEN_PROGRAM, &DEPLOY_ARGUMENT)),
        Registry::VsCodeMarketplace => Some((&VSCE_PROGRAM, &PUBLISH_ARGUMENT)),
        Registry::OpenVsx => Some((&OVSX_PROGRAM, &PUBLISH_ARGUMENT)),
        _ => None,
    }
}

/// Names of the long-lived token secrets that trusted publishing replaces.
#[must_use]
pub const fn registry_token_secrets(registry: Registry) -> &'static [&'static str] {
    match registry {
        Registry::Npm => &["NPM_TOKEN", "NPM_AUTH_TOKEN"],
        Registry::CratesIo => &[
            "CARGO_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "CRATES_IO_TOKEN",
            "CRATES_TOKEN",
        ],
        Registry::PyPi => &[
            "PYPI_TOKEN",
            "PYPI_API_TOKEN",
            "PYPI_PASSWORD",
            "TWINE_PASSWORD",
        ],
        Registry::RubyGems => &["GEM_HOST_API_KEY", "RUBYGEMS_API_KEY"],
        Registry::NuGet => &["NUGET_API_KEY", "NUGET_TOKEN"],
        Registry::Jsr => &["JSR_TOKEN"],
        Registry::DockerHub => &["DOCKERHUB_TOKEN", "DOCKER_HUB_TOKEN", "DOCKER_PASSWORD"],
        Registry::MavenCentral => &[
            "MAVEN_CENTRAL_TOKEN",
            "MAVEN_CENTRAL_PASSWORD",
            "OSSRH_TOKEN",
            "OSSRH_PASSWORD",
            "SONATYPE_TOKEN",
            "CENTRAL_TOKEN",
        ],
        Registry::VsCodeMarketplace => &["VSCE_PAT", "VSCE_TOKEN"],
        Registry::OpenVsx => &["OVSX_PAT", "OVSX_TOKEN"],
        Registry::ChromeWebStore => &["CHROME_WEB_STORE_REFRESH_TOKEN", "CHROME_REFRESH_TOKEN"],
        _ => &[],
    }
}

const fn trusted_publishing_hint(registry: Registry) -> &'static str {
    match registry {
        Registry::Npm => "npm trusted publishing (`id-token: write` and npm 11.5.1 or newer)",
        Registry::CratesIo => {
            "crates.io trusted publishing (`rust-lang/crates-io-auth-action` with `id-token: write`)"
        }
        Registry::RubyGems => "RubyGems trusted publishing (id-token: write)", Registry::NuGet => "NuGet/login short-lived OIDC API key (id-token: write)", Registry::Jsr => "JSR repository-linked OIDC (id-token: write)",
        _ => "PyPI trusted publishing (`pypa/gh-action-pypi-publish` with `id-token: write`)",
    }
}

/// One job of a workflow, with the lines below its header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The job id, such as `javascript-release`.
    pub name: String,
    /// The job body, without full-line comments.
    pub lines: Vec<String>,
}

/// A workflow split into its top-level keys other than `jobs`, and its jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWorkflow {
    /// Every line outside the `jobs` block, such as `on` and `permissions`.
    pub header: Vec<String>,
    /// The jobs in file order.
    pub jobs: Vec<Job>,
}

/// The workflow that publishes to a registry, as far as detection can tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Publisher {
    /// The workflow file to trust, when exactly one publishes.
    pub workflow: Option<String>,
    /// The jobs of `workflow` that publish.
    pub jobs: Vec<String>,
    /// The GitHub environment shared by those jobs, if any.
    pub environment: Option<String>,
    /// Workflow files that all publish, when detection cannot choose one.
    pub candidates: Vec<String>,
    /// Findings to show next to the package, such as a missing `id-token`.
    pub warnings: Vec<String>,
}

/// A long-lived registry token secret that a workflow reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSecret {
    /// The workflow file name.
    pub workflow: String,
    /// The secret name, such as `NPM_TOKEN`.
    pub secret: String,
}

/// Split a GitHub Actions workflow into its jobs by indentation, since no YAML
/// parser is bundled. Full-line comments are dropped.
#[must_use]
pub fn parse_workflow(contents: &str) -> ParsedWorkflow {
    let stripped = strip_comments(contents, "workflow.yml");
    let lines = stripped
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let Some(start) = lines
        .iter()
        .position(|line| line.starts_with("jobs") && line[4..].trim_start().trim_end() == ":")
    else {
        return ParsedWorkflow {
            header: lines,
            jobs: Vec::new(),
        };
    };
    let end = lines
        .iter()
        .skip(start + 1)
        .position(|line| {
            line.chars()
                .next()
                .is_some_and(|first| !first.is_whitespace())
        })
        .map_or(lines.len(), |offset| start + 1 + offset);
    let body = &lines[start + 1..end];
    let job_indent = body
        .iter()
        .find(|line| !line.trim().is_empty())
        .map_or(0, |line| indent_of(line));
    let mut jobs: Vec<Job> = Vec::new();
    for line in body {
        let header = (!line.trim().is_empty() && indent_of(line) == job_indent)
            .then(|| JOB_HEADER.captures(line))
            .flatten();
        if let Some(header) = header {
            jobs.push(Job {
                name: header[1].to_owned(),
                lines: Vec::new(),
            });
        } else if let Some(job) = jobs.last_mut() {
            job.lines.push(line.clone());
        }
    }
    let mut header = lines[..start].to_vec();
    header.extend_from_slice(&lines[end..]);
    ParsedWorkflow { header, jobs }
}

struct Found {
    workflow: String,
    job: String,
    id_token: bool,
    environment: Option<String>,
}

/// Find the workflow jobs that publish to a registry with an OIDC token, so
/// the trusted publisher names the workflow CI actually publishes from.
#[must_use]
pub fn detect_publisher(root: &Path, workflows: &[Workflow], registry: Registry) -> Publisher {
    let parsed_workflows = workflows
        .iter()
        .map(|workflow| (workflow.name.as_str(), parse_workflow(&workflow.contents)))
        .collect::<BTreeMap<_, _>>();
    let mut found = Vec::new();
    for workflow in workflows {
        let parsed = &parsed_workflows[workflow.name.as_str()];
        // Registries check the caller's identity, so a workflow that only runs
        // when another workflow calls it can never be the trusted publisher.
        if only_called(&parsed.header) {
            continue;
        }
        let workflow_grant =
            grants_id_token(child_block(&parsed.header, 0, "permissions").as_ref());
        for job in &parsed.jobs {
            let callee = job
                .lines
                .iter()
                .find_map(|line| LOCAL_WORKFLOW.captures(line))
                .and_then(|captures| parsed_workflows.get(&captures[1]));
            let publishes = callee.map_or_else(
                || job_publishes(root, parsed, job, registry),
                |callee| {
                    callee
                        .jobs
                        .iter()
                        .any(|job| job_publishes(root, callee, job, registry))
                },
            );
            if !publishes {
                continue;
            }
            let indent = child_indent(&job.lines);
            found.push(Found {
                workflow: workflow.name.clone(),
                job: job.name.clone(),
                id_token: grants_id_token(child_block(&job.lines, indent, "permissions").as_ref())
                    .or(workflow_grant)
                    .unwrap_or(false),
                environment: environment_name(
                    child_block(&job.lines, indent, "environment").as_ref(),
                ),
            });
        }
    }
    let trusted = found
        .iter()
        .filter(|item| !TRUSTED_REGISTRIES.contains(&registry) || item.id_token)
        .collect::<Vec<_>>();
    let files = unique(trusted.iter().map(|item| item.workflow.clone()));
    if files.len() > 1 {
        return Publisher {
            warnings: vec![format!(
                "several workflows publish to {registry} with id-token: write ({}); pass --workflow <file> to choose the trusted publisher",
                files.join(", ")
            )],
            candidates: files,
            ..Publisher::default()
        };
    }
    if let [file] = files.as_slice() {
        return described(file, &trusted);
    }
    let all = found.iter().collect::<Vec<_>>();
    let untrusted = unique(found.iter().map(|item| item.workflow.clone()));
    if let [file] = untrusted.as_slice() {
        let mut result = described(file, &all);
        let jobs = unique(found.iter().map(|item| format!("job {}", item.job)));
        result.warnings.insert(
            0,
            format!(
                "{file} publishes to {registry} from {} without `id-token: write`; add it so trusted publishing can mint a token",
                jobs.join(", ")
            ),
        );
        return result;
    }
    Publisher {
        warnings: if untrusted.len() > 1 {
            vec![format!(
                "several workflows publish to {registry} ({}) and none grants id-token: write; pass --workflow <file> to choose the trusted publisher",
                untrusted.join(", ")
            )]
        } else {
            Vec::new()
        },
        candidates: untrusted,
        ..Publisher::default()
    }
}

fn described(workflow: &str, found: &[&Found]) -> Publisher {
    let jobs = found
        .iter()
        .filter(|item| item.workflow == workflow)
        .collect::<Vec<_>>();
    let environments = unique(jobs.iter().map(|item| item.environment.clone()));
    let mut warnings = Vec::new();
    if environments.len() > 1 {
        warnings.push(format!(
            "the publishing jobs in {workflow} use different environments ({}); pass --environment <name> to choose one",
            environments
                .iter()
                .map(|item| item.as_deref().unwrap_or("none"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Publisher {
        workflow: Some(workflow.to_owned()),
        jobs: unique(jobs.iter().map(|item| item.job.clone())),
        environment: if environments.len() == 1 {
            environments.into_iter().next().flatten()
        } else {
            None
        },
        candidates: Vec::new(),
        warnings,
    }
}

fn unique<T: PartialEq>(items: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut result = Vec::new();
    for item in items {
        if !result.contains(&item) {
            result.push(item);
        }
    }
    result
}

/// List the long-lived registry token secrets that workflows read, which
/// trusted publishing makes unnecessary.
#[must_use]
pub fn token_secrets(workflows: &[Workflow], registry: Registry) -> Vec<TokenSecret> {
    let mut found = Vec::new();
    for workflow in workflows {
        for name in registry_token_secrets(registry) {
            let pattern = regex(&format!(r"\bsecrets\.{name}\b"));
            if strip_comments(&workflow.contents, "workflow.yml")
                .lines()
                .any(|line| !line.trim_start().starts_with('#') && pattern.is_match(line))
            {
                found.push(TokenSecret {
                    workflow: workflow.name.clone(),
                    secret: (*name).to_owned(),
                });
            }
        }
    }
    found
}

/// Explain how to replace a long-lived token secret with trusted publishing.
#[must_use]
pub fn token_secret_warning(registry: Registry, secret: &TokenSecret) -> String {
    if !TRUSTED_REGISTRIES.contains(&registry) {
        return format!(
            "{} reads secrets.{}; ensure a scoped expiring publishing token through gh-manager",
            secret.workflow, secret.secret
        );
    }
    format!(
        "{} reads secrets.{}; publish with {} and delete the long-lived token after verification",
        secret.workflow,
        secret.secret,
        trusted_publishing_hint(registry)
    )
}

fn job_publishes(root: &Path, parsed: &ParsedWorkflow, job: &Job, registry: Registry) -> bool {
    let Some(pattern) = job_pattern(registry) else {
        return false;
    };
    let executable = executable_lines(&job.lines);
    if publishing_line(&executable, pattern) {
        return true;
    }
    let directories = working_directories(parsed.header.iter().chain(&job.lines));
    let mut scripts = Vec::new();
    for script in script_references(&executable) {
        let lines = read_inside(root, &directories, &script)
            .map(|contents| code_lines(&contents, &script))
            .unwrap_or_default();
        if script_publishes(&lines, registry) {
            return true;
        }
        scripts.extend(lines);
    }
    let commands = if job
        .lines
        .iter()
        .any(|line| line.contains("changesets/action@"))
    {
        job.lines
            .iter()
            .filter_map(|line| CHANGESETS_PUBLISH.captures(line))
            .map(|captures| unquote(&captures[1]).to_owned())
            .collect()
    } else {
        Vec::new()
    };
    if publishing_line(&commands, pattern) {
        return true;
    }
    // A package script may run from the job, its changesets command, or a
    // script, as in `$`npm run changeset:publish``.
    for line in executable.iter().chain(&commands).chain(&scripts) {
        for captures in RUN_SCRIPT.captures_iter(line) {
            if !command_position(&line[..captures.get(0).expect("match").start()]) {
                continue;
            }
            let name = captures
                .get(1)
                .or_else(|| captures.get(2))
                .map_or("", |name| name.as_str());
            let command = read_inside(root, &directories, "package.json")
                .and_then(|manifest| package_script(&manifest, name));
            if command.is_some_and(|command| publishing_line(&[command], pattern)) {
                return true;
            }
        }
    }
    false
}

fn only_called(header: &[String]) -> bool {
    let Some(block) = child_block(header, 0, r#"["']?on["']?"#) else {
        return false;
    };
    if !block.inline.is_empty() {
        let inline = unquote(&block.inline);
        let inline = inline.strip_prefix('[').unwrap_or(inline);
        let inline = inline.strip_suffix(']').unwrap_or(inline);
        return inline.trim() == "workflow_call";
    }
    let indent = child_indent(&block.nested);
    let triggers = block
        .nested
        .iter()
        .filter(|line| !line.trim().is_empty() && indent_of(line) == indent)
        .map(|line| {
            let trigger = line.trim();
            let trigger = trigger.strip_prefix('-').map_or(trigger, str::trim_start);
            trigger.split(':').next().unwrap_or_default().trim_end()
        })
        .collect::<Vec<_>>();
    !triggers.is_empty() && triggers.iter().all(|trigger| *trigger == "workflow_call")
}

fn publishing_line(lines: &[String], pattern: &Regex) -> bool {
    lines.iter().any(|line| {
        !line.contains("--dry-run")
            && pattern.find_iter(line).any(|found| {
                command_position(&line[..found.start()])
                    || regex(r#"^\s*-?\s*uses\s*:\s*['"]?[\w./-]*$"#)
                        .is_match(&line[..found.start()])
            })
    })
}

/// Extract executable run blocks and action references; names and env values are inert.
#[must_use]
pub fn executable_lines(lines: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut block_indent = None;
    let run = regex(r"^\s*-?\s*run\s*:\s*(.*)$");
    for line in lines {
        let indent = indent_of(line);
        if block_indent.is_some_and(|level| line.trim().is_empty() || indent > level) {
            result.push(line.clone());
            continue;
        }
        block_indent = None;
        if regex(r"^\s*-?\s*uses\s*:").is_match(line) {
            result.push(line.clone());
        }
        let Some(captures) = run.captures(line) else {
            continue;
        };
        if regex(r"^[|>][-+\d]*$").is_match(&captures[1]) {
            block_indent = Some(
                indent
                    + if line.trim_start().starts_with('-') {
                        2
                    } else {
                        0
                    },
            );
        } else {
            result.push(unquote(&captures[1]).to_owned());
        }
    }
    result
}

fn code_lines(contents: &str, script: &str) -> Vec<String> {
    strip_comments(contents, script)
        .lines()
        .filter(|line| !line.contains("--dry-run"))
        .map(str::to_owned)
        .collect()
}

fn script_publishes(lines: &[String], registry: Registry) -> bool {
    let (Some(pattern), Some((program, subcommand))) =
        (job_pattern(registry), script_pattern(registry))
    else {
        return false;
    };
    if lines.iter().any(|line| {
        pattern
            .find_iter(line)
            .any(|found| command_position(&line[..found.start()]))
    }) {
        return true;
    }
    lines.iter().enumerate().any(|(index, line)| {
        program.is_match(line)
            && regex(
                r"(?:exec|spawn|run|Command::new|subprocess|system|(?:npm|pnpm|yarn)\s*\(\s*\[)",
            )
            .is_match(line)
            && lines[index..(index + SCRIPT_WINDOW).min(lines.len())]
                .iter()
                .any(|nearby| subcommand.is_match(nearby))
    })
}

fn package_script(contents: &str, name: &str) -> Option<String> {
    serde_json::from_str::<JsonValue>(contents)
        .ok()?
        .get("scripts")?
        .get(name)?
        .as_str()
        .map(str::to_owned)
}

/// List repository script paths that a job's lines run, such as `node x.mjs`.
#[must_use]
pub fn script_references(lines: &[String]) -> Vec<String> {
    unique(
        lines
            .iter()
            .flat_map(|line| REFERENCE_SEPARATOR.split(line))
            .filter(|token| SCRIPT_REFERENCE.is_match(token))
            .map(str::to_owned),
    )
}

fn working_directories<'a>(lines: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut directories = vec![String::new()];
    for line in lines {
        let Some(captures) = WORKING_DIRECTORY.captures(line) else {
            continue;
        };
        let directory = unquote(&captures[1]);
        if !directory.is_empty()
            && !directory.contains("${{")
            && !directories.iter().any(|item| item == directory)
        {
            directories.push(directory.to_owned());
        }
    }
    directories
}

fn read_inside(root: &Path, directories: &[String], file: &str) -> Option<String> {
    let canonical_root = root.canonicalize().ok()?;
    directories.iter().find_map(|directory| {
        let candidate = normalize(&root.join(directory).join(file));
        if !is_inside(root, &candidate) {
            return None;
        }
        let resolved = candidate.canonicalize().ok()?;
        if !is_inside(&canonical_root, &resolved) {
            return None;
        }
        let metadata = fs::metadata(&resolved).ok()?;
        if metadata.is_file() && metadata.len() <= MAX_SCRIPT_BYTES {
            fs::read_to_string(&resolved).ok()
        } else {
            None
        }
    })
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

fn is_inside(root: &Path, candidate: &Path) -> bool {
    candidate != root && candidate.starts_with(root)
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn child_indent(lines: &[String]) -> usize {
    lines
        .iter()
        .find(|line| !line.trim().is_empty())
        .map_or(0, |line| indent_of(line))
}

struct Block {
    inline: String,
    nested: Vec<String>,
}

fn child_block(lines: &[String], indent: usize, key: &str) -> Option<Block> {
    let expression = regex(&format!(r"^\s*{key}\s*:\s*(.*)$"));
    let start = lines
        .iter()
        .position(|line| indent_of(line) == indent && expression.is_match(line))?;
    let nested = lines[start + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || indent_of(line) > indent)
        .cloned()
        .collect();
    let inline = expression.captures(&lines[start]).map_or("", |captures| {
        captures.get(1).map_or("", |value| value.as_str())
    });
    Some(Block {
        inline: strip_comment(inline),
        nested,
    })
}

fn grants_id_token(block: Option<&Block>) -> Option<bool> {
    let block = block?;
    let inline = unquote(&block.inline);
    if !inline.is_empty() {
        return Some(
            inline == "write-all" || (inline.starts_with('{') && INLINE_ID_TOKEN.is_match(inline)),
        );
    }
    Some(
        block
            .nested
            .iter()
            .any(|line| NESTED_ID_TOKEN.is_match(line)),
    )
}

fn environment_name(block: Option<&Block>) -> Option<String> {
    let block = block?;
    let value = if block.inline.starts_with('{') {
        INLINE_NAME
            .captures(&block.inline)
            .map_or_else(String::new, |captures| captures[1].to_owned())
    } else if block.inline.is_empty() {
        block
            .nested
            .iter()
            .find_map(|line| NESTED_NAME.captures(line))
            .map_or_else(String::new, |captures| captures[1].to_owned())
    } else {
        block.inline.clone()
    };
    let value = unquote(&strip_comment(&value)).to_owned();
    (!value.is_empty() && !value.contains("${{")).then_some(value)
}

fn strip_comment(value: &str) -> String {
    TRAILING_COMMENT.replace(value, "").trim().to_owned()
}

fn unquote(value: &str) -> &str {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}
