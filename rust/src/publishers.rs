//! Job-aware detection of the workflow that publishes to a registry, so the
//! trusted publisher names the workflow file CI actually publishes from.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value as JsonValue;

use crate::model::Registry;
use crate::publishing_state::{publishing_policies, AuthenticationGate};
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
const SCRIPT_WINDOW: usize = 12;
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

/// Compile a static (or repeatedly built) pattern once per process; compiled
/// regexes share their program, so clones are cheap.
pub(crate) fn regex(pattern: &str) -> Regex {
    static CACHE: LazyLock<std::sync::Mutex<std::collections::HashMap<String, Regex>>> =
        LazyLock::new(Default::default);
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(pattern.to_owned())
        .or_insert_with(|| Regex::new(pattern).expect("static pattern must compile"))
        .clone()
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
static NUGET_JOB: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"\bdotnet\s+nuget\s+push\b|PRM_NUGET_OIDC|\bnuget\s+push\b|PRM_NUGET_OIDC")
});
static JSR_JOB: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:deno|jsr)\s+publish\b"));
static MAVEN_JOB: LazyLock<Regex> = LazyLock::new(|| {
    regex(
        r"(?:\./|\b)mvnw?\s+(?:-\S+\s+)*deploy\b|(?:\./|\b)gradlew?\s+(?:-\S+\s+)*publish(?:ToMavenCentral|AndReleaseToMavenCentral|AllPublicationsTo\w+|ToSonatype)?\b",
    )
});
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
static MAVEN_PROGRAM: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:mvnw?|gradlew?)\b"));
static PUSH_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| regex(r#"["'`]push["'`]"#));
static DEPLOY_ARGUMENT: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"["'`](?:deploy|publish\w*)["'`]"#));

static PACKAGIST_EVIDENCE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"packagist\.org/api/(?:update|create)-package"));
static GO_EVIDENCE: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"\bgh\s+release\s+create\b|\bgit\s+push\b[^\n]*\btags?\b|proxy\.golang\.org")
});
static CMAKE_EVIDENCE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"\bgh\s+release\s+(?:create|upload)\b"));

/// Registries without an upload command.
///
/// A release is a tag, a GitHub release
/// asset or a registry update hook. The evidence is matched anywhere in the
/// job's executable lines, as the URL is usually an argument.
#[must_use]
pub fn release_evidence(registry: Registry) -> Option<&'static Regex> {
    match registry {
        Registry::Packagist => Some(&PACKAGIST_EVIDENCE),
        Registry::GoModules => Some(&GO_EVIDENCE),
        Registry::Cmake => Some(&CMAKE_EVIDENCE),
        _ => None,
    }
}

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
    /// Reusable-workflow chains ending in each callee job that publishes.
    pub callees: Vec<String>,
    /// Authentication policy of the publishing jobs, for OIDC registries.
    pub policy: Option<String>,
    /// Repository variable that selects OIDC, when one gates it.
    pub gate: Option<AuthenticationGate>,
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
    callees: Vec<String>,
    id_token: bool,
    environment: Option<String>,
}

/// Find the workflow jobs that publish to a registry with an OIDC token, so
/// the trusted publisher names the workflow CI actually publishes from.
#[must_use]
pub fn detect_publisher(root: &Path, workflows: &[Workflow], registry: Registry) -> Publisher {
    let mut result = detect_publisher_details(root, workflows, registry);
    if matches!(
        registry,
        Registry::CratesIo | Registry::Npm | Registry::PyPi | Registry::NuGet
    ) && (result.workflow.is_some() || !result.candidates.is_empty())
    {
        let (policy, gates) = publishing_policies(root, workflows, registry);
        if let Some(first) = gates.first() {
            result.gate.clone_from(&first.gate);
            if gates.iter().any(|facts| facts.password_always_token) {
                let variable = result
                    .gate
                    .as_ref()
                    .map_or("", |gate| gate.variable.as_str());
                result.warnings.push(format!(
                    "the publish password expression `vars.{variable} == 'true' && '' || secrets.*` always selects the token ('' is falsy in GitHub expressions); the variable cannot switch to OIDC"
                ));
            }
        }
        if policy == "variable-gated-oidc-with-token-fallback" {
            let variable = result
                .gate
                .as_ref()
                .map_or("gate", |gate| gate.variable.as_str());
            result.warnings.push(format!(
                "publishing selects OIDC only when the repository variable {variable} is 'true' and otherwise uses the token; register the trusted publisher, verify it, then set the variable"
            ));
        } else if policy != "oidc-with-token-fallback" {
            result.warnings.push(format!(
                "publishing authentication is {policy}; migrate to OIDC with token fallback"
            ));
        }
        result.policy = Some(policy);
    }
    result
}

fn detect_publisher_details(root: &Path, workflows: &[Workflow], registry: Registry) -> Publisher {
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
                .and_then(|captures| parsed_workflows.get_key_value(&captures[1]));
            let mut chain = Vec::new();
            let environments = callee.map_or_else(
                || {
                    if job_publishes(root, parsed, job, registry) {
                        vec![environment_name(
                            child_block(&job.lines, child_indent(&job.lines), "environment")
                                .as_ref(),
                        )]
                    } else {
                        Vec::new()
                    }
                },
                |(&name, _)| {
                    called_environments(
                        root,
                        &parsed_workflows,
                        name,
                        registry,
                        &[workflow.name.as_str()],
                        &mut chain,
                    )
                },
            );
            if environments.is_empty() {
                continue;
            }
            let indent = child_indent(&job.lines);
            for environment in environments {
                found.push(Found {
                    workflow: workflow.name.clone(),
                    job: job.name.clone(),
                    callees: chain.clone(),
                    id_token: grants_id_token(
                        child_block(&job.lines, indent, "permissions").as_ref(),
                    )
                    .or(workflow_grant)
                    .unwrap_or(false),
                    environment,
                });
            }
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
        callees: unique(jobs.iter().flat_map(|item| item.callees.iter().cloned())),
        jobs: unique(jobs.iter().map(|item| item.job.clone())),
        environment: if environments.len() == 1 {
            environments.into_iter().next().flatten()
        } else {
            None
        },
        candidates: Vec::new(),
        warnings,
        ..Publisher::default()
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

fn called_environments(
    root: &Path,
    workflows: &BTreeMap<&str, ParsedWorkflow>,
    name: &str,
    registry: Registry,
    visited: &[&str],
    chain: &mut Vec<String>,
) -> Vec<Option<String>> {
    if visited.contains(&name) {
        return Vec::new();
    }
    let Some(parsed) = workflows.get(name) else {
        return Vec::new();
    };
    let mut seen = visited.to_vec();
    seen.push(name);
    let mut environments = Vec::new();
    for job in &parsed.jobs {
        if let Some(captures) = job
            .lines
            .iter()
            .find_map(|line| LOCAL_WORKFLOW.captures(line))
        {
            environments.extend(called_environments(
                root,
                workflows,
                &captures[1],
                registry,
                &seen,
                chain,
            ));
        } else if job_publishes(root, parsed, job, registry) {
            // Exact reusable-workflow identity: each callee job that publishes.
            chain.push(format!("{} -> {}", seen[1..].join(" -> "), job.name));
            environments.push(environment_name(
                child_block(&job.lines, child_indent(&job.lines), "environment").as_ref(),
            ));
        }
    }
    unique(environments)
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
        "{} reads secrets.{}; publish with {} and retain a rotated token as fallback",
        secret.workflow,
        secret.secret,
        trusted_publishing_hint(registry)
    )
}

/// Detect publication in one job, including referenced scripts.
#[must_use]
pub fn job_publishes(root: &Path, parsed: &ParsedWorkflow, job: &Job, registry: Registry) -> bool {
    let executable = executable_lines(&job.lines);
    // SwiftPM consumers resolve Git tags: a tag or release that names the
    // Swift package is the publication.
    if registry == Registry::Swiftpm {
        return crate::swift_distribution::swift_release_evidence(&executable.join("\n"));
    }
    if let Some(evidence) = release_evidence(registry) {
        return evidence.is_match(&executable.join("\n"));
    }
    let Some(pattern) = job_pattern(registry) else {
        return false;
    };
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

pub(crate) fn only_called(header: &[String]) -> bool {
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

static USES_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"^\s*-?\s*uses\s*:\s*['"]?[\w./-]*$"#));
static RUN_LINE: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*-?\s*run\s*:\s*(.*)$"));
static USES_LINE: LazyLock<Regex> = LazyLock::new(|| regex(r"^\s*-?\s*uses\s*:"));
static BLOCK_SCALAR: LazyLock<Regex> = LazyLock::new(|| regex(r"^[|>][-+\d]*$"));
static DECLARATION: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:const|let|var)\s+(\w+)\s*="));
static SPAWN_CALL: LazyLock<Regex> = LazyLock::new(|| {
    regex(r"(?:exec|spawn|run|Command::new|subprocess|system|(?:npm|pnpm|yarn)\s*\(\s*\[)")
});

fn publishing_line(lines: &[String], pattern: &Regex) -> bool {
    lines.iter().any(|line| {
        !line.contains("--dry-run")
            && pattern.find_iter(line).any(|found| {
                command_position(&line[..found.start()])
                    || USES_PREFIX.is_match(&line[..found.start()])
            })
    })
}

/// Extract executable run blocks and action references; names and env values are inert.
#[must_use]
pub fn executable_lines(lines: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut block_indent = None;
    for line in lines {
        let indent = indent_of(line);
        if block_indent.is_some_and(|level| line.trim().is_empty() || indent > level) {
            result.push(line.clone());
            continue;
        }
        block_indent = None;
        if USES_LINE.is_match(line) {
            result.push(line.clone());
        }
        let Some(captures) = RUN_LINE.captures(line) else {
            continue;
        };
        if BLOCK_SCALAR.is_match(&captures[1]) {
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
    let commands: Vec<_> = lines
        .iter()
        .filter_map(|line| {
            let captures = DECLARATION.captures(line)?;
            pattern.is_match(line).then(|| captures[1].to_owned())
        })
        .collect();
    if lines.iter().any(|line| {
        commands.iter().any(|name| {
            regex(&format!(
                r"(?:exec(?:Sync)?|spawn(?:Sync)?|run|system)\s*\(\s*{name}\b"
            ))
            .is_match(line)
        })
    }) {
        return true;
    }
    let variables: Vec<_> = lines
        .iter()
        .filter_map(|line| {
            let captures = DECLARATION.captures(line)?;
            program.is_match(line).then(|| captures[1].to_owned())
        })
        .collect();
    lines.iter().enumerate().any(|(index, line)| {
        (program.is_match(line)
            || variables.iter().any(|name| {
                let nearby = lines[index..(index + 3).min(lines.len())].join("\n");
                regex(&format!(r"\b{name}\b")).is_match(&nearby)
            }))
            && SPAWN_CALL.is_match(line)
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
    let lexical_root = normalize(root);
    directories.iter().find_map(|directory| {
        let candidate = normalize(&lexical_root.join(directory).join(file));
        if !is_inside(&lexical_root, &candidate) {
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

pub(crate) fn child_indent(lines: &[String]) -> usize {
    lines
        .iter()
        .find(|line| !line.trim().is_empty())
        .map_or(0, |line| indent_of(line))
}

pub(crate) struct Block {
    inline: String,
    nested: Vec<String>,
}

pub(crate) fn child_block(lines: &[String], indent: usize, key: &str) -> Option<Block> {
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

pub(crate) fn grants_id_token(block: Option<&Block>) -> Option<bool> {
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
