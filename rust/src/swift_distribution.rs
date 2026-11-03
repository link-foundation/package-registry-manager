//! `SwiftPM` distributions: Git tags of the repository, never an upload.
//!
//! Package.swift is read as text and never evaluated during discovery. An
//! optional Swift package registry (SE-0292) is a separate channel and is
//! never assumed.
use crate::model::{Package, Registry, SetupStep, StepKind};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;

const SEMVER: &str = r"^v?\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?$";

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static pattern")
}

fn all(pattern: &str, text: &str) -> Vec<String> {
    regex(pattern)
        .captures_iter(text)
        .map(|captures| captures[1].to_owned())
        .collect()
}

/// The static `SwiftPM` distribution facts shared by every Swift package.
#[must_use]
pub fn swift_distribution_facts() -> Value {
    json!({
        "channel": "git-tag",
        "registry": "none",
        "tag_format": "bare semantic version (SwiftPM ignores prefixed tags)",
        "identity": "repository URL",
    })
}

/// What a Package.swift declares, read without running it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwiftManifest {
    /// `Package(name:)`.
    pub name: Option<String>,
    /// Library products, the only products consumers can depend on.
    pub libraries: Vec<String>,
    /// Executable products.
    pub executables: Vec<String>,
    /// Explicit target `path:` arguments.
    pub paths: Vec<String>,
    /// Local `.package(path:)` dependencies.
    pub local_dependencies: Vec<String>,
    /// The `swift-tools-version` comment.
    pub tools: Option<String>,
}

/// Reads Package.swift as text: name, products, target paths, local path
/// dependencies and tools version. Comments are ignored.
#[must_use]
pub fn parse_swift_manifest(contents: &str) -> SwiftManifest {
    let code = regex(r"(?s)/\*.*?\*/")
        .replace_all(&regex(r"//[^\n]*").replace_all(contents, ""), "")
        .into_owned();
    SwiftManifest {
        name: regex(r#"\bPackage\s*\(\s*name\s*:\s*"([^"]+)""#)
            .captures(&code)
            .map(|captures| captures[1].to_owned()),
        libraries: all(r#"\.library\s*\(\s*name\s*:\s*"([^"]+)""#, &code),
        executables: all(r#"\.executable\s*\(\s*name\s*:\s*"([^"]+)""#, &code),
        paths: all(
            r#"\.(?:target|executableTarget|testTarget|binaryTarget)\s*\([^)]*?\bpath\s*:\s*"([^"]+)""#,
            &code,
        ),
        local_dependencies: all(r#"\.package\s*\([^)]*?\bpath\s*:\s*"([^"]+)""#, &code),
        tools: regex(r"(?m)^\s*//\s*swift-tools-version\s*:\s*([\d.]+)")
            .captures(contents)
            .map(|captures| captures[1].to_owned()),
    }
}

/// Normalizes a POSIX path lexically; `None` when it escapes its base.
#[must_use]
pub fn normalize(path: &str) -> Option<String> {
    if path.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

fn join(directory: &str, file: &str) -> Option<String> {
    normalize(&if directory == "." || directory.is_empty() {
        file.to_owned()
    } else {
        format!("{directory}/{file}")
    })
}

/// Version-file candidates: a sibling VERSION, then a VERSION in the one
/// top-level directory holding every explicit target path.
#[must_use]
pub fn swift_version_candidates(manifest: &str, parsed: &SwiftManifest) -> Vec<String> {
    let directory = manifest
        .rsplit_once('/')
        .map_or(".", |(directory, _)| directory);
    let mut candidates = join(directory, "VERSION").into_iter().collect::<Vec<_>>();
    let roots = parsed
        .paths
        .iter()
        .filter_map(|item| normalize(item))
        .filter_map(|item| item.split('/').next().map(str::to_owned))
        .filter(|item| item != ".")
        .collect::<BTreeSet<_>>();
    if roots.len() == 1 {
        if let Some(candidate) = roots
            .first()
            .and_then(|root| join(directory, &format!("{root}/VERSION")))
        {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

/// Reads a version file below `root`; unsafe references and symlinks are
/// errors, absent files are `Ok(None)`.
fn read_version(root: &Path, relative: &str) -> Result<Option<String>, String> {
    let Some(normalized) =
        normalize(relative).filter(|path| path != "." && !path.split('/').any(|p| p == ".git"))
    else {
        return Err(format!("unsafe Swift version file reference {relative}"));
    };
    let path = root.join(&normalized);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "Swift version file {relative} is a symlink and was not followed"
            ));
        }
        Ok(metadata) if !metadata.is_file() => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("unreadable Swift version file {relative}: {error}")),
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let value = text.trim_start_matches('\u{feff}').trim().to_owned();
            Ok(regex(SEMVER).is_match(&value).then_some(value))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("unreadable Swift version file {relative}: {error}")),
    }
}

fn skip(manifest: String, reason: &str) -> Package {
    let mut item = Package::new(Registry::Swiftpm, String::new(), None, manifest)
        .unpublishable("not a SwiftPM distribution");
    item.audit.insert("discovery_skip".into(), json!(reason));
    item
}

/// Builds the `SwiftPM` package record for one Package.swift. `mapping` is
/// the reviewed `swiftpm` entry of `.package-registry-manager.json`.
#[must_use]
pub fn swift_distribution(
    contents: &str,
    manifest: String,
    root: &Path,
    mapping: Option<&Value>,
) -> Package {
    let parsed = parse_swift_manifest(contents);
    let ancestor = parsed
        .local_dependencies
        .iter()
        .find(|item| item.starts_with("..") || normalize(item).is_none());
    if let (Some(ancestor), None) = (ancestor, mapping) {
        return skip(manifest, &format!("Package.swift depends on the checkout at {ancestor} by path: a consumer example, not a SwiftPM distribution"));
    }
    let Some(name) = parsed.name.clone() else {
        return skip(manifest, "Package.swift declares no library product; SwiftPM consumers can only depend on library products");
    };
    if parsed.libraries.is_empty() && mapping.is_none() {
        return skip(manifest, "Package.swift declares no library product; SwiftPM consumers can only depend on library products");
    }
    let mut problems = Vec::new();
    let candidates = mapping
        .and_then(|mapping| mapping["version_file"].as_str())
        .map_or_else(
            || swift_version_candidates(&manifest, &parsed),
            |file| vec![file.to_owned()],
        );
    let mut found = Vec::new();
    for candidate in &candidates {
        match read_version(root, candidate) {
            Ok(Some(value)) => found.push((candidate.clone(), value)),
            Ok(None) => {}
            Err(problem) => problems.push(problem),
        }
    }
    let distinct = found
        .iter()
        .map(|(_, value)| value)
        .collect::<BTreeSet<_>>();
    let mut version = None;
    let mut source = None;
    if distinct.len() > 1 {
        problems.push(format!(
            "ambiguous Swift version: {}; list the version file under \"swiftpm\" in .package-registry-manager.json",
            found.iter().map(|(file, value)| format!("{file}={value}")).collect::<Vec<_>>().join(", ")
        ));
    } else if let Some((file, value)) = found.first() {
        version = Some(value.clone());
        source = Some(file.clone());
    } else if problems.is_empty() {
        problems.push(format!(
            "no Swift version source: none of {} holds a semantic version; the release tag stays unknown",
            candidates.join(", ")
        ));
    }
    let products = mapping
        .and_then(|mapping| mapping["products"].as_array())
        .map_or_else(
            || json!(parsed.libraries),
            |products| Value::Array(products.clone()),
        );
    let mut facts = swift_distribution_facts();
    facts["products"] = products;
    if !parsed.executables.is_empty() {
        facts["executables"] = json!(parsed.executables);
    }
    if let Some(tools) = &parsed.tools {
        facts["tools_version"] = json!(tools);
    }
    facts["tag"] = json!(version
        .as_deref()
        .map(|value| value.trim_start_matches('v')));
    let name = mapping
        .and_then(|mapping| mapping["name"].as_str())
        .map_or(name, str::to_owned);
    let mut item = Package::new(Registry::Swiftpm, name, version, manifest);
    item.audit.insert("swift_package".into(), facts);
    if let Some(source) = source {
        item.audit.insert("version_source".into(), json!(source));
    }
    item.warnings = problems;
    item
}

/// Whether a job's text creates a tag or GitHub release and names the
/// Swift package (its manifest, version file or toolchain).
#[must_use]
pub fn swift_release_evidence(text: &str) -> bool {
    regex(r"\bgh\s+release\s+create\b|\bgit\s+tag\b|\bgit\s+push\b[^\n]*\btags?\b|ensure-[\w-]*tag")
        .is_match(text)
        && regex(r"Package\.swift|swift/VERSION|\bswift\s+(?:package|build|run|test)\b")
            .is_match(text)
}

/// The expected bare tag of a Swift package, when its version is known.
#[must_use]
pub fn swift_tag(item: &Package) -> Option<String> {
    item.audit
        .get("swift_package")
        .and_then(|facts| facts["tag"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            item.version
                .as_deref()
                .map(|v| v.trim_start_matches('v').to_owned())
        })
}

/// Validation and verification steps for a Git-tag `SwiftPM` distribution.
#[must_use]
pub fn swift_plan(item: &Package, slug: Option<&str>) -> Vec<SetupStep> {
    let check = |id: &str, title: &str, description: &str, args: &[&str]| {
        SetupStep::new(id, title, StepKind::Check, description).command("swift", args)
    };
    let tag = swift_tag(item);
    let shown = tag.as_deref().unwrap_or("(unresolved)");
    let workflow = item.workflow.as_ref().map_or_else(
        || "No workflow creates the tag yet; add the release job in a reviewed pull request.".to_owned(),
        |workflow| format!("The existing {workflow} workflow creates the tag with its GITHUB_TOKEN; no SwiftPM OIDC, token or registry upload exists."),
    );
    let mut release = SetupStep::new(
        "verify-swift-release",
        &format!("Verify tag {shown}, tagged Package.swift and GitHub release"),
        StepKind::Api,
        format!(
            "SwiftPM publishes by tag: read the exact tag {shown}{}, its commit, the tagged Package.swift and the public GitHub release. Missing tag, wrong commit, missing or incorrect manifest and missing release are distinct; an existing tag is never moved. {workflow}",
            slug.map_or(String::new(), |slug| format!(" of {slug}"))
        ),
    );
    if let Some(tag) = &tag {
        release = release.detail("tag", json!(tag));
    }
    vec![
        check(
            "describe-package",
            "Resolve the Swift package manifest",
            "Let the official toolchain evaluate Package.swift in a reviewed checkout; discovery itself never runs it.",
            &["package", "describe", "--type", "json"],
        ),
        check("build-package", "Build the Swift package", "Build every product.", &["build"]),
        check("test-package", "Test the Swift package", "Run the package tests.", &["test"]),
        release,
        SetupStep::new(
            "verify-swift-consumer",
            "Verify a downstream consumer resolves the exact version",
            StepKind::Manual,
            format!(
                "Resolve .package(url: ..., exact: \"{}\") from a clean consumer and run it (swift run --package-path <consumer>). A skipped or failed consumer job keeps the release unverified.",
                tag.as_deref().unwrap_or("<version>")
            ),
        )
        .detail("state", json!("manual-prerequisite")),
    ]
}

/// Composer and `SwiftPM` both read bare semantic-version tags of the same
/// repository, so one tag releases both: record the sharing so neither side
/// moves or recreates the other's tag.
pub fn mark_shared_tags(items: &mut [Package]) {
    let peers = items
        .iter()
        .filter(|item| {
            item.registry == Registry::Packagist
                && item.manifest == "composer.json"
                && !item.audit.contains_key("discovery_skip")
        })
        .map(|item| format!("{}:{}", item.registry, item.name))
        .collect::<Vec<_>>();
    if peers.is_empty() {
        return;
    }
    for swift in items
        .iter_mut()
        .filter(|item| item.registry == Registry::Swiftpm)
    {
        let Some(facts) = swift.audit.get_mut("swift_package") else {
            continue;
        };
        let Some(tag) = facts["tag"].as_str().map(str::to_owned) else {
            continue;
        };
        facts["shared_tag"] = json!({
            "tag": tag,
            "with": peers,
            "policy": "create the tag once at the gated commit, verify an existing tag points there, never move it",
        });
    }
}
