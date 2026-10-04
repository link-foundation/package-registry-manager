use std::fmt;
use std::str::FromStr;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// A supported public package registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Registry {
    #[serde(rename = "npm")]
    Npm,
    #[serde(rename = "crates-io")]
    CratesIo,
    #[serde(rename = "pypi")]
    PyPi,
    #[serde(rename = "go-modules")]
    GoModules,
    #[serde(rename = "nuget")]
    NuGet,
    #[serde(rename = "maven-central")]
    MavenCentral,
    #[serde(rename = "packagist")]
    Packagist,
    #[serde(rename = "docker-hub")]
    DockerHub,
    #[serde(rename = "ghcr")]
    Ghcr,
}

impl Registry {
    pub const ALL: [Self; 9] = [
        Self::Npm,
        Self::CratesIo,
        Self::PyPi,
        Self::GoModules,
        Self::NuGet,
        Self::MavenCentral,
        Self::Packagist,
        Self::DockerHub,
        Self::Ghcr,
    ];

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::CratesIo => "crates.io",
            Self::PyPi => "PyPI",
            Self::GoModules => "Go modules",
            Self::NuGet => "NuGet",
            Self::MavenCentral => "Maven Central",
            Self::Packagist => "Packagist",
            Self::DockerHub => "Docker Hub",
            Self::Ghcr => "GitHub Container Registry",
        }
    }
}

impl fmt::Display for Registry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Npm => "npm",
            Self::CratesIo => "crates-io",
            Self::PyPi => "pypi",
            Self::GoModules => "go-modules",
            Self::NuGet => "nuget",
            Self::MavenCentral => "maven-central",
            Self::Packagist => "packagist",
            Self::DockerHub => "docker-hub",
            Self::Ghcr => "ghcr",
        })
    }
}

impl FromStr for Registry {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let normalized = value.to_ascii_lowercase().replace(['.', '_'], "-");
        match normalized.as_str() {
            "npm" => Ok(Self::Npm),
            "cargo" | "crate" | "crates" | "crates-io" => Ok(Self::CratesIo),
            "python" | "pypi" => Ok(Self::PyPi),
            "go" | "go-module" | "go-modules" => Ok(Self::GoModules),
            "dotnet" | "nuget" => Ok(Self::NuGet),
            "java" | "maven" | "maven-central" => Ok(Self::MavenCentral),
            "composer" | "php" | "packagist" => Ok(Self::Packagist),
            "docker" | "dockerhub" | "docker-hub" | "docker-io" => Ok(Self::DockerHub),
            "ghcr" | "ghcr-io" | "github-container-registry" => Ok(Self::Ghcr),
            _ => bail!(
                "unsupported registry '{value}'; expected npm, crates-io, pypi, go-modules, nuget, maven-central, packagist, docker-hub, or ghcr"
            ),
        }
    }
}

/// A package discovered in a repository, with its registry state when probed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub registry: Registry,
    pub name: String,
    pub version: Option<String>,
    pub manifest: String,
    pub publishable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
    /// Non-fatal findings, such as a GHCR workflow without `packages: write`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The GitHub Actions workflow file that publishes this package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// The jobs of `workflow` that publish this package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_jobs: Vec<String>,
    /// The GitHub environment of the publishing jobs, when they share one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Workflow files that all publish this package, when detection cannot
    /// choose one; `--workflow` picks the trusted publisher among them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflow_candidates: Vec<String>,
    /// Long-lived registry token secrets that workflows still read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub token_secrets: Vec<String>,
    /// Whether the registry already has the package; unset when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists_on_registry: Option<bool>,
    /// Whether the latest release came through trusted publishing; unset when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_publishing: Option<bool>,
}

impl Package {
    /// Create a publishable package with no problems and unknown registry state.
    #[must_use]
    pub const fn new(
        registry: Registry,
        name: String,
        version: Option<String>,
        manifest: String,
    ) -> Self {
        Self {
            registry,
            name,
            version,
            manifest,
            publishable: true,
            problems: Vec::new(),
            warnings: Vec::new(),
            workflow: None,
            workflow_jobs: Vec::new(),
            environment: None,
            workflow_candidates: Vec::new(),
            token_secrets: Vec::new(),
            exists_on_registry: None,
            trusted_publishing: None,
        }
    }

    /// Mark the package as not publishable for the given reason.
    #[must_use]
    pub fn unpublishable(mut self, problem: impl Into<String>) -> Self {
        self.publishable = false;
        self.problems.push(problem.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub root: String,
    pub github_owner: Option<String>,
    pub github_repository: Option<String>,
    pub release_workflow: Option<String>,
    /// The workflow that deploys to GitHub Pages, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages_workflow: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspection {
    pub schema_version: u8,
    pub repository: RepositoryInfo,
    pub packages: Vec<Package>,
    /// Manifests left out of `packages`; listed only when asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Skipped>,
}

/// A manifest that inspection left out, such as a test fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    /// Manifest path relative to the repository root.
    pub manifest: String,
    /// Why the manifest was left out.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepKind {
    Check,
    Command,
    Wait,
    Browser,
    /// A registry API call made through the automated browser's session.
    Api,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStep {
    pub id: String,
    pub title: String,
    pub kind: StepKind,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Condition that must hold for the step to run, such as `package-missing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    /// Working directory; `{worktree}` is the temporary release checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Ask before running, because the step publishes or changes secrets.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
}

impl SetupStep {
    /// Create a step with no command, URL, condition, or working directory.
    #[must_use]
    pub fn new(id: &str, title: &str, kind: StepKind, description: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            kind,
            description: description.into(),
            command: None,
            url: None,
            when: None,
            cwd: None,
            confirm: false,
        }
    }

    /// Attach an exact argument vector.
    #[must_use]
    pub fn command(mut self, program: &str, args: &[&str]) -> Self {
        self.command = Some(CommandSpec {
            program: program.to_owned(),
            args: args.iter().map(|value| (*value).to_owned()).collect(),
        });
        self
    }

    /// Attach a URL.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Run the step only while the condition holds.
    #[must_use]
    pub fn when(mut self, condition: &str) -> Self {
        self.when = Some(condition.to_owned());
        self
    }

    /// Run the step in the given directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Ask for confirmation before running the step.
    #[must_use]
    pub const fn confirmed(mut self) -> Self {
        self.confirm = true;
        self
    }
}

/// What a setup plan does for a package, given its registry state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanMode {
    /// The package is missing: publish it once, then attach trusted publishing.
    Bootstrap,
    /// The package exists without trusted publishing: attach it.
    Attach,
    /// Trusted publishing is already in use: nothing to do.
    Complete,
}

impl fmt::Display for PlanMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bootstrap => "bootstrap",
            Self::Attach => "attach",
            Self::Complete => "complete",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPublisherPrefill {
    pub provider: String,
    pub organization: String,
    pub repository: String,
    pub workflow: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// `PyPI` project name for pending publishers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// A manual prerequisite of a plan: what was detected and what is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prerequisite {
    pub id: String,
    pub title: String,
    pub detected: String,
    pub required: String,
    /// Whether the prerequisite is met; `None` when it cannot be told.
    pub ok: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPlan {
    pub schema_version: u8,
    pub registry: Registry,
    pub package: Package,
    pub repository: RepositoryInfo,
    pub steps: Vec<SetupStep>,
    /// Bootstrap, attach, or complete; unset when the registry state is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PlanMode>,
    /// Manual prerequisites, listed before the steps; empty without a probe.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<Prerequisite>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_publisher: Option<TrustedPublisherPrefill>,
    /// Why no steps were planned, for packages whose manifest forbids publishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
}
