use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::model::{Inspection, Package, Registry, RepositoryInfo};

const IGNORED_DIRECTORIES: &[&str] = &[
    ".git",
    ".package-registry-manager",
    ".venv",
    "node_modules",
    "target",
    "vendor",
];

/// Inspect a repository without running package-manager commands.
pub fn inspect_repository(root: &Path) -> Result<Inspection> {
    let root = root
        .canonicalize()
        .with_context(|| format!("repository does not exist: {}", root.display()))?;
    let mut manifests = Vec::new();
    collect_manifests(&root, &mut manifests)?;
    manifests.sort();
    manifests.retain(|path| {
        path.file_name().and_then(|name| name.to_str()) != Some("setup.py")
            || !path.with_file_name("pyproject.toml").is_file()
    });

    let mut packages = manifests
        .iter()
        .filter_map(|path| parse_manifest(path, &root).transpose())
        .collect::<Result<Vec<_>>>()?;
    packages.sort_by(|left, right| {
        (left.registry, &left.manifest, &left.name).cmp(&(
            right.registry,
            &right.manifest,
            &right.name,
        ))
    });

    let (github_owner, github_repository) = github_coordinates(&root, &manifests)?;
    Ok(Inspection {
        schema_version: 1,
        repository: RepositoryInfo {
            root: root.to_string_lossy().into_owned(),
            github_owner,
            github_repository,
            release_workflow: release_workflow(&root)?,
        },
        packages,
    })
}

fn collect_manifests(directory: &Path, manifests: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("cannot read {}", directory.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if !IGNORED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref()) {
                collect_manifests(&path, manifests)?;
            }
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

fn parse_manifest(path: &Path, root: &Path) -> Result<Option<Package>> {
    let relative = relative_path(path, root);
    let contents = fs::read_to_string(path)
        .with_context(|| format!("cannot read manifest {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    match name {
        "package.json" => parse_npm(&contents, relative),
        "Cargo.toml" => parse_cargo(&contents, relative),
        "pyproject.toml" => parse_pyproject(&contents, relative),
        "setup.py" => Ok(Some(parse_setup_py(&contents, relative))),
        "go.mod" => Ok(Some(parse_go(&contents, relative))),
        "pom.xml" => Ok(Some(parse_maven(&contents, relative))),
        "build.gradle" | "build.gradle.kts" => Ok(Some(parse_gradle(&contents, relative))),
        "composer.json" => parse_composer(&contents, relative),
        _ if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("csproj" | "fsproj" | "vbproj")
        ) =>
        {
            Ok(Some(parse_dotnet(&contents, path, relative)))
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
    let publishable = !value
        .get("private")
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    Ok(Some(Package {
        registry: Registry::Npm,
        name: name.to_owned(),
        version: json_string(&value, "version"),
        manifest,
        publishable,
        problems: if publishable {
            Vec::new()
        } else {
            vec!["package.json marks this package as private".to_owned()]
        },
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
    let publishable = !matches!(package.get("publish"), Some(TomlValue::Boolean(false)));
    Ok(Some(Package {
        registry: Registry::CratesIo,
        name: name.to_owned(),
        version: package
            .get("version")
            .and_then(TomlValue::as_str)
            .map(str::to_owned),
        manifest,
        publishable,
        problems: if publishable {
            Vec::new()
        } else {
            vec!["Cargo.toml disables publishing".to_owned()]
        },
    }))
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
    Ok(Some(Package {
        registry: Registry::PyPi,
        name: name.to_owned(),
        version: metadata
            .get("version")
            .and_then(TomlValue::as_str)
            .map(str::to_owned),
        manifest,
        publishable: true,
        problems: Vec::new(),
    }))
}

fn parse_setup_py(contents: &str, manifest: String) -> Package {
    Package {
        registry: Registry::PyPi,
        name: regex_capture(contents, r#"(?m)\bname\s*=\s*['\"]([^'\"]+)"#)
            .unwrap_or_else(|| "unknown-python-package".to_owned()),
        version: regex_capture(contents, r#"(?m)\bversion\s*=\s*['\"]([^'\"]+)"#),
        manifest,
        publishable: true,
        problems: Vec::new(),
    }
}

fn parse_go(contents: &str, manifest: String) -> Package {
    let name = contents
        .lines()
        .find_map(|line| line.trim().strip_prefix("module "))
        .map_or("unknown-go-module", str::trim)
        .to_owned();
    Package {
        registry: Registry::GoModules,
        name,
        version: None,
        manifest,
        publishable: true,
        problems: Vec::new(),
    }
}

fn parse_dotnet(contents: &str, path: &Path, manifest: String) -> Package {
    let fallback = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-dotnet-package")
        .to_owned();
    Package {
        registry: Registry::NuGet,
        name: xml_tag(contents, "PackageId")
            .or_else(|| xml_tag(contents, "AssemblyName"))
            .unwrap_or(fallback),
        version: xml_tag(contents, "PackageVersion").or_else(|| xml_tag(contents, "Version")),
        manifest,
        publishable: true,
        problems: Vec::new(),
    }
}

fn parse_maven(contents: &str, manifest: String) -> Package {
    let artifact =
        xml_tag(contents, "artifactId").unwrap_or_else(|| "unknown-maven-artifact".to_owned());
    let group = xml_tag(contents, "groupId");
    Package {
        registry: Registry::MavenCentral,
        name: group.map_or_else(|| artifact.clone(), |group| format!("{group}:{artifact}")),
        version: xml_tag(contents, "version"),
        manifest,
        publishable: true,
        problems: Vec::new(),
    }
}

fn parse_gradle(contents: &str, manifest: String) -> Package {
    let group = regex_capture(contents, r#"(?m)^\s*group\s*=\s*['\"]([^'\"]+)"#);
    let artifact = regex_capture(
        contents,
        r#"(?m)^\s*(?:archivesBaseName|rootProject\.name)\s*=\s*['\"]([^'\"]+)"#,
    )
    .unwrap_or_else(|| "gradle-project".to_owned());
    Package {
        registry: Registry::MavenCentral,
        name: group.map_or_else(|| artifact.clone(), |group| format!("{group}:{artifact}")),
        version: regex_capture(contents, r#"(?m)^\s*version\s*=\s*['\"]([^'\"]+)"#),
        manifest,
        publishable: true,
        problems: Vec::new(),
    }
}

fn parse_composer(contents: &str, manifest: String) -> Result<Option<Package>> {
    let value: JsonValue =
        serde_json::from_str(contents).with_context(|| format!("invalid JSON in {manifest}"))?;
    let Some(name) = value.get("name").and_then(JsonValue::as_str) else {
        return Ok(None);
    };
    Ok(Some(Package {
        registry: Registry::Packagist,
        name: name.to_owned(),
        version: json_string(&value, "version"),
        manifest,
        publishable: true,
        problems: Vec::new(),
    }))
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
        let contents = fs::read_to_string(manifest)?;
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

fn release_workflow(root: &Path) -> Result<Option<String>> {
    let directory = root.join(".github/workflows");
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(None);
    };
    let mut candidates = entries.collect::<std::io::Result<Vec<_>>>()?;
    candidates.sort_by_key(std::fs::DirEntry::file_name);
    let mut fallback = None;
    for entry in candidates {
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let filename = entry.file_name().to_string_lossy().into_owned();
        let contents = fs::read_to_string(path)?;
        if contents.contains("npm publish") || contents.contains("npm stage publish") {
            return Ok(Some(filename));
        }
        if fallback.is_none() && filename.contains("release") {
            fallback = Some(filename);
        }
    }
    Ok(fallback)
}
