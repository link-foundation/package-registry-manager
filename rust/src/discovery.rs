use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::containers::{container_packages, CONTAINER_FILES};
use crate::model::{Inspection, Package, Registry, RepositoryInfo, Skipped};
use crate::publishers::{
    detect_publisher, token_secret_warning, token_secrets, Publisher, TRUSTED_REGISTRIES,
};
use crate::skips::{
    ignore_reason, ignored_by, read_ignore_list, referenced_by_workflow, test_directory,
    test_directory_reason,
};
use crate::workflows::{read_workflows, Workflow};

const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".package-registry-manager",
    ".venv",
    "node_modules",
    "target",
    "vendor",
];

/// Options that shape repository inspection.
#[derive(Debug, Clone, Copy, Default)]
pub struct InspectOptions {
    /// List the manifests that inspection left out under `skipped`.
    pub include_skipped: bool,
}

/// Inspect a repository without running package-manager commands.
pub fn inspect_repository(root: &Path) -> Result<Inspection> {
    inspect_repository_with(root, InspectOptions::default())
}

/// Inspect a repository with explicit options. Manifests in test and example
/// directories, or matched by the ignore list, are left out.
pub fn inspect_repository_with(root: &Path, options: InspectOptions) -> Result<Inspection> {
    let root = root
        .canonicalize()
        .with_context(|| format!("repository does not exist: {}", root.display()))?;
    let mut manifests = Vec::new();
    let mut dockerfiles = Vec::new();
    collect_manifests(&root, &mut manifests, &mut dockerfiles)?;
    manifests.sort();
    manifests.retain(|path| {
        path.file_name().and_then(|name| name.to_str()) != Some("setup.py")
            || !path.with_file_name("pyproject.toml").is_file()
    });

    let workflows = read_workflows(&root)?;
    let ignore = read_ignore_list(&root)?;
    let mut skipped = Vec::new();
    let ignored = |manifest: &str, skipped: &mut Vec<Skipped>| {
        ignored_by(&ignore, manifest).is_some_and(|pattern| {
            skipped.push(Skipped {
                manifest: manifest.to_owned(),
                reason: ignore_reason(pattern),
            });
            true
        })
    };
    let mut parsed = Vec::new();
    for path in &manifests {
        let relative = relative_path(path, &root);
        if ignored(&relative, &mut skipped) {
            continue;
        }
        match parse_manifest(path, &root) {
            Ok(item) => parsed.extend(item),
            // An unparsable fixture is skipped like any other fixture.
            Err(_) if test_directory(&relative).is_some() => {
                kept(&workflows, &relative, false, &mut skipped);
            }
            Err(error) => return Err(error),
        }
    }
    let coordinate_manifests = manifests
        .iter()
        .filter(|path| {
            let relative = relative_path(path, &root);
            ignored_by(&ignore, &relative).is_none() && test_directory(&relative).is_none()
        })
        .cloned()
        .collect::<Vec<_>>();
    let (github_owner, github_repository) = github_coordinates(&root, &coordinate_manifests)?;
    let publishers = TRUSTED_REGISTRIES
        .iter()
        .map(|registry| (*registry, detect_publisher(&root, &workflows, *registry)))
        .collect::<BTreeMap<_, _>>();
    let mut packages = parsed
        .into_iter()
        .filter(|item| kept(&workflows, &item.manifest, item.publishable, &mut skipped))
        .map(|item| with_publisher(item, &workflows, &publishers))
        .collect::<Vec<_>>();
    let dockerfiles = dockerfiles
        .iter()
        .map(|path| relative_path(path, &root))
        .filter(|item| !ignored(item, &mut skipped) && kept(&workflows, item, true, &mut skipped))
        .collect::<Vec<_>>();
    packages.extend(container_packages(
        &dockerfiles,
        &workflows,
        github_owner.as_deref(),
        github_repository.as_deref(),
    ));
    packages.sort_by(|left, right| {
        (left.registry, &left.manifest, &left.name).cmp(&(
            right.registry,
            &right.manifest,
            &right.name,
        ))
    });
    if options.include_skipped {
        skipped.sort_by(|left, right| left.manifest.cmp(&right.manifest));
    } else {
        skipped.clear();
    }
    Ok(Inspection {
        schema_version: 1,
        repository: RepositoryInfo {
            root: root.to_string_lossy().into_owned(),
            github_owner,
            github_repository,
            release_workflow: TRUSTED_REGISTRIES
                .iter()
                .find_map(|registry| publishers[registry].workflow.clone()),
        },
        packages,
        skipped,
    })
}

/// Keep a manifest outside test and example directories, or one inside them
/// that a workflow publishes; record why others are skipped.
fn kept(
    workflows: &[Workflow],
    manifest: &str,
    publishable: bool,
    skipped: &mut Vec<Skipped>,
) -> bool {
    let Some(directory) = test_directory(manifest) else {
        return true;
    };
    if publishable && referenced_by_workflow(workflows, manifest) {
        return true;
    }
    skipped.push(Skipped {
        manifest: manifest.to_owned(),
        reason: test_directory_reason(directory),
    });
    false
}

fn with_publisher(
    mut item: Package,
    workflows: &[Workflow],
    publishers: &BTreeMap<Registry, Publisher>,
) -> Package {
    let Some(publisher) = publishers.get(&item.registry).filter(|_| item.publishable) else {
        return item;
    };
    item.workflow.clone_from(&publisher.workflow);
    item.workflow_jobs.clone_from(&publisher.jobs);
    item.environment.clone_from(&publisher.environment);
    if publisher.candidates.len() > 1 {
        item.workflow_candidates.clone_from(&publisher.candidates);
    }
    let secrets = token_secrets(workflows, item.registry);
    item.warnings.extend(publisher.warnings.iter().cloned());
    item.warnings.extend(
        secrets
            .iter()
            .map(|secret| token_secret_warning(item.registry, secret)),
    );
    for secret in secrets {
        if !item.token_secrets.contains(&secret.secret) {
            item.token_secrets.push(secret.secret);
        }
    }
    item
}

fn collect_manifests(
    directory: &Path,
    manifests: &mut Vec<PathBuf>,
    dockerfiles: &mut Vec<PathBuf>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("cannot read {}", directory.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if !IGNORED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref()) {
                collect_manifests(&path, manifests, dockerfiles)?;
            }
        } else if CONTAINER_FILES.contains(&entry.file_name().to_string_lossy().as_ref()) {
            dockerfiles.push(path);
        } else if is_manifest(&path) {
            manifests.push(path);
        }
    }
    Ok(())
}

fn is_manifest(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        "package.json"
            | "Cargo.toml"
            | "pyproject.toml"
            | "setup.py"
            | "go.mod"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "composer.json"
    ) || matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("csproj" | "fsproj" | "vbproj")
    )
}

/// Read a manifest without the byte order mark that editors on Windows may
/// save; npm and Cargo accept it.
fn read_manifest(path: &Path) -> Result<String> {
    let mut contents = fs::read_to_string(path)
        .with_context(|| format!("cannot read manifest {}", path.display()))?;
    if contents.starts_with('\u{feff}') {
        contents.drain(..'\u{feff}'.len_utf8());
    }
    Ok(contents)
}

fn parse_manifest(path: &Path, root: &Path) -> Result<Option<Package>> {
    let relative = relative_path(path, root);
    let contents = read_manifest(path)?;
    let contents = contents.as_str();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    match name {
        "package.json" => parse_npm(contents, relative),
        "Cargo.toml" => parse_cargo(contents, relative),
        "pyproject.toml" => parse_pyproject(contents, relative),
        "setup.py" => Ok(Some(parse_setup_py(contents, relative))),
        "go.mod" => Ok(Some(parse_go(contents, relative))),
        "pom.xml" => Ok(Some(parse_maven(contents, relative))),
        "build.gradle" | "build.gradle.kts" => Ok(Some(parse_gradle(contents, relative))),
        "composer.json" => parse_composer(contents, relative),
        _ if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("csproj" | "fsproj" | "vbproj")
        ) =>
        {
            Ok(Some(parse_dotnet(contents, path, relative)))
        }
        _ => Ok(None),
    }
}

fn parse_npm(contents: &str, manifest: String) -> Result<Option<Package>> {
    let value: JsonValue =
        serde_json::from_str(contents).with_context(|| format!("invalid JSON in {manifest}"))?;
    let Some(name) = value.get("name").and_then(JsonValue::as_str) else {
        return Ok(None);
    };
    let package = Package::new(
        Registry::Npm,
        name.to_owned(),
        json_string(&value, "version"),
        manifest,
    );
    let private = value
        .get("private")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    Ok(Some(if private {
        package.unpublishable("package.json marks this package as private")
    } else {
        package
    }))
}

fn parse_cargo(contents: &str, manifest: String) -> Result<Option<Package>> {
    let value: TomlValue =
        toml::from_str(contents).with_context(|| format!("invalid TOML in {manifest}"))?;
    let Some(package) = value.get("package") else {
        return Ok(None);
    };
    let Some(name) = package.get("name").and_then(TomlValue::as_str) else {
        return Ok(None);
    };
    let item = Package::new(
        Registry::CratesIo,
        name.to_owned(),
        package
            .get("version")
            .and_then(TomlValue::as_str)
            .map(str::to_owned),
        manifest,
    );
    Ok(Some(
        if matches!(package.get("publish"), Some(TomlValue::Boolean(false))) {
            item.unpublishable("Cargo.toml disables publishing")
        } else {
            item
        },
    ))
}

fn parse_pyproject(contents: &str, manifest: String) -> Result<Option<Package>> {
    let value: TomlValue =
        toml::from_str(contents).with_context(|| format!("invalid TOML in {manifest}"))?;
    let metadata = value
        .get("project")
        .or_else(|| value.get("tool")?.get("poetry"));
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let Some(name) = metadata.get("name").and_then(TomlValue::as_str) else {
        return Ok(None);
    };
    Ok(Some(Package::new(
        Registry::PyPi,
        name.to_owned(),
        metadata
            .get("version")
            .and_then(TomlValue::as_str)
            .map(str::to_owned),
        manifest,
    )))
}

fn parse_setup_py(contents: &str, manifest: String) -> Package {
    Package::new(
        Registry::PyPi,
        regex_capture(contents, r#"(?m)\bname\s*=\s*['\"]([^'\"]+)"#)
            .unwrap_or_else(|| "unknown-python-package".to_owned()),
        regex_capture(contents, r#"(?m)\bversion\s*=\s*['\"]([^'\"]+)"#),
        manifest,
    )
}

fn parse_go(contents: &str, manifest: String) -> Package {
    let name = contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("module "))
        .map_or("unknown-go-module", str::trim)
        .to_owned();
    Package::new(Registry::GoModules, name, None, manifest)
}

fn parse_dotnet(contents: &str, path: &Path, manifest: String) -> Package {
    let fallback = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-dotnet-package")
        .to_owned();
    Package::new(
        Registry::NuGet,
        xml_tag(contents, "PackageId")
            .or_else(|| xml_tag(contents, "AssemblyName"))
            .unwrap_or(fallback),
        xml_tag(contents, "PackageVersion").or_else(|| xml_tag(contents, "Version")),
        manifest,
    )
}

fn parse_maven(contents: &str, manifest: String) -> Package {
    let artifact =
        xml_tag(contents, "artifactId").unwrap_or_else(|| "unknown-maven-artifact".to_owned());
    let group = xml_tag(contents, "groupId");
    Package::new(
        Registry::MavenCentral,
        group.map_or_else(|| artifact.clone(), |group| format!("{group}:{artifact}")),
        xml_tag(contents, "version"),
        manifest,
    )
}

fn parse_gradle(contents: &str, manifest: String) -> Package {
    let group = regex_capture(contents, r#"(?m)^\s*group\s*=\s*['\"]([^'\"]+)"#);
    let artifact = regex_capture(
        contents,
        r#"(?m)^\s*(?:archivesBaseName|rootProject\.name)\s*=\s*['\"]([^'\"]+)"#,
    )
    .unwrap_or_else(|| "gradle-project".to_owned());
    Package::new(
        Registry::MavenCentral,
        group.map_or_else(|| artifact.clone(), |group| format!("{group}:{artifact}")),
        regex_capture(contents, r#"(?m)^\s*version\s*=\s*['\"]([^'\"]+)"#),
        manifest,
    )
}

fn parse_composer(contents: &str, manifest: String) -> Result<Option<Package>> {
    let value: JsonValue =
        serde_json::from_str(contents).with_context(|| format!("invalid JSON in {manifest}"))?;
    let Some(name) = value.get("name").and_then(JsonValue::as_str) else {
        return Ok(None);
    };
    Ok(Some(Package::new(
        Registry::Packagist,
        name.to_owned(),
        json_string(&value, "version"),
        manifest,
    )))
}

fn json_string(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
}

fn regex_capture(contents: &str, pattern: &str) -> Option<String> {
    Regex::new(pattern)
        .expect("static regular expression must compile")
        .captures(contents)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str().to_owned())
}

fn xml_tag(contents: &str, tag: &str) -> Option<String> {
    regex_capture(
        contents,
        &format!(r"(?s)<{tag}(?:\s[^>]*)?>\s*([^<]+?)\s*</{tag}>"),
    )
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn github_coordinates(
    root: &Path,
    manifests: &[PathBuf],
) -> Result<(Option<String>, Option<String>)> {
    let config_path = root.join(".git/config");
    if let Ok(contents) = fs::read_to_string(&config_path) {
        if let Some(remote) = Regex::new(r"(?m)^\s*url\s*=\s*(\S+)")?
            .captures(&contents)
            .and_then(|captures| captures.get(1))
            .map(|capture| capture.as_str())
        {
            if let Some(coordinates) = parse_github_url(remote) {
                return Ok(coordinates);
            }
        }
    }

    for manifest in manifests {
        let contents = read_manifest(manifest)?;
        let repository = match manifest.file_name().and_then(|name| name.to_str()) {
            Some("package.json") => {
                let value: JsonValue = serde_json::from_str(&contents)?;
                value
                    .get("repository")
                    .and_then(|repository| {
                        repository
                            .as_str()
                            .or_else(|| repository.get("url").and_then(JsonValue::as_str))
                    })
                    .map(str::to_owned)
            }
            Some("Cargo.toml") => {
                let value: TomlValue = toml::from_str(&contents)?;
                value
                    .get("package")
                    .and_then(|package| package.get("repository"))
                    .and_then(TomlValue::as_str)
                    .map(str::to_owned)
            }
            _ => None,
        };
        if let Some(coordinates) = repository.as_deref().and_then(parse_github_url) {
            return Ok(coordinates);
        }
    }
    Ok((None, None))
}

fn parse_github_url(remote: &str) -> Option<(Option<String>, Option<String>)> {
    let normalized = remote
        .trim_end_matches(".git")
        .trim_start_matches("git+")
        .replace("git@github.com:", "https://github.com/")
        .replace("ssh://git@github.com/", "https://github.com/");
    let path = normalized.strip_prefix("https://github.com/")?;
    let mut parts = path.split('/');
    Some((
        parts.next().map(str::to_owned),
        parts.next().map(str::to_owned),
    ))
}
