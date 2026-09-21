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
}

impl Registry {
    pub const ALL: [Self; 7] = [
        Self::Npm,
        Self::CratesIo,
        Self::PyPi,
        Self::GoModules,
        Self::NuGet,
        Self::MavenCentral,
        Self::Packagist,
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
            _ => bail!(
                "unsupported registry '{value}'; expected npm, crates-io, pypi, go-modules, nuget, maven-central, or packagist"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub registry: Registry,
    pub name: String,
    pub version: Option<String>,
    pub manifest: String,
    pub publishable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryInfo {
    pub root: String,
    pub github_owner: Option<String>,
    pub github_repository: Option<String>,
    pub release_workflow: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspection {
    pub schema_version: u8,
    pub repository: RepositoryInfo,
    pub packages: Vec<Package>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepKind {
    Check,
    Browser,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPublisherPrefill {
    pub provider: String,
    pub organization: String,
    pub repository: String,
    pub workflow: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupPlan {
    pub schema_version: u8,
    pub registry: Registry,
    pub package: Package,
    pub repository: RepositoryInfo,
    pub steps: Vec<SetupStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_publisher: Option<TrustedPublisherPrefill>,
}
