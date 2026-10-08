//! Compare the canonical GitHub repository with metadata and registry identities.

use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;

use crate::model::{
    Inspection, Package, PublisherIdentity, RepositoryInfo, RepositoryMismatch,
    TrustedPublisherPrefill,
};

/// Normalize GitHub HTTPS, SSH, npm shorthand, and provenance source URLs.
#[must_use]
pub fn github_slug(value: &str) -> Option<String> {
    let text = value.trim();
    let text = text.strip_prefix("git+").unwrap_or(text);
    let text = text.strip_prefix("github:").unwrap_or(text);
    let text = [
        "https://github.com/",
        "git@github.com:",
        "ssh://git@github.com/",
    ]
    .iter()
    .find_map(|prefix| text.strip_prefix(prefix))
    .unwrap_or(text);
    let parts: Vec<_> = text.split(['/', '?', '#', '@']).take(2).collect();
    if parts.len() != 2 {
        return None;
    }
    let name = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || b"_.-".contains(&ch))
    };
    (valid(parts[0]) && valid(name)).then(|| format!("{}/{name}", parts[0]))
}

/// The repository being set up, as owner/name.
#[must_use]
pub fn repository_slug(repository: &RepositoryInfo) -> Option<String> {
    Some(format!(
        "{}/{}",
        repository.github_owner.as_ref()?,
        repository.github_repository.as_ref()?
    ))
}

/// Extract declared repository URLs from npm, Cargo, and Python manifests.
#[must_use]
pub fn manifest_urls(contents: &str, manifest: &str) -> Vec<String> {
    let contents = contents.trim_start_matches('\u{feff}');
    if manifest.ends_with("package.json") {
        let Ok(document) = serde_json::from_str::<Value>(contents) else {
            return Vec::new();
        };
        return document["repository"]
            .as_str()
            .or_else(|| document["repository"]["url"].as_str())
            .map(str::to_owned)
            .into_iter()
            .collect();
    }
    let Ok(document) = toml::from_str::<toml::Value>(contents) else {
        return Vec::new();
    };
    let mut urls = Vec::new();
    for table in [
        document.get("package"),
        document.get("tool").and_then(|value| value.get("poetry")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(url) = table.get("repository").and_then(toml::Value::as_str) {
            urls.push(url.to_owned());
        }
    }
    for table in [
        document.get("project"),
        document.get("tool").and_then(|value| value.get("poetry")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(table) = table.get("urls").and_then(toml::Value::as_table) {
            for url in table.values() {
                if let Some(url) = url.as_str() {
                    urls.push(url.to_owned());
                }
            }
        }
    }
    urls
}

/// Compare evidence and produce warnings without confusing provenance with settings.
pub fn compare_repositories(package: &mut Package, repository: &RepositoryInfo, urls: &[String]) {
    let Some(slug) = repository_slug(repository) else {
        return;
    };
    let mut pairs: Vec<_> = urls
        .iter()
        .filter_map(|url| github_slug(url).map(|slug| ("manifest", slug)))
        .collect();
    pairs.extend(
        package
            .provenance_repositories
            .iter()
            .filter_map(|url| github_slug(url).map(|slug| ("provenance", slug))),
    );
    if let Some(publishers) = &package.configured_publishers {
        pairs.extend(
            publishers
                .iter()
                .map(|publisher| ("trusted publisher", publisher.repository.clone())),
        );
    }
    package.repository_mismatches.clear();
    package
        .warnings
        .retain(|warning| !warning.starts_with("repository mismatch:"));
    for (source, old) in pairs {
        if old.eq_ignore_ascii_case(&slug) {
            continue;
        }
        let finding = RepositoryMismatch {
            source: source.to_owned(),
            repository: old,
        };
        if !package.repository_mismatches.contains(&finding) {
            package.warnings.push(format!(
                "repository mismatch: {} still names {}; repository being set up is {slug}",
                finding.source, finding.repository
            ));
            package.repository_mismatches.push(finding);
        }
    }
}

/// Check local manifest identities even during offline inspection.
pub fn inspect_manifest_repositories(inspection: &mut Inspection) {
    for package in &mut inspection.packages {
        if !matches!(
            package.registry,
            crate::model::Registry::Npm
                | crate::model::Registry::CratesIo
                | crate::model::Registry::PyPi
        ) {
            continue;
        }
        let Ok(contents) =
            std::fs::read_to_string(Path::new(&inspection.repository.root).join(&package.manifest))
        else {
            continue;
        };
        let urls = manifest_urls(&contents, &package.manifest);
        compare_repositories(package, &inspection.repository, &urls);
        if package
            .repository_mismatches
            .iter()
            .any(|finding| finding.source == "manifest")
        {
            package.manifest_urls = urls;
        }
    }
}

/// Run a bounded read-only lookup without relaying authentication output.
pub async fn identity_command(
    program: &str,
    args: &[&str],
    directory: &Path,
    verbose: bool,
) -> Option<String> {
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(program)
            .args(args)
            .current_dir(directory)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    if let Ok(Ok(output)) = result {
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
    }
    if verbose {
        eprintln!("{program} repository identity lookup unavailable");
    }
    None
}

/// Follow repository transfers through GitHub's canonical `full_name`.
pub async fn resolve_repository(inspection: &mut Inspection, verbose: bool) {
    let Some(slug) = repository_slug(&inspection.repository) else {
        return;
    };
    let endpoint = format!("repos/{slug}");
    if let Some(canonical) = identity_command(
        "gh",
        &["api", &endpoint, "--jq", ".full_name"],
        Path::new(&inspection.repository.root),
        verbose,
    )
    .await
    .and_then(|output| github_slug(&output))
    {
        if let Some((owner, name)) = canonical.split_once('/') {
            inspection.repository.github_owner = Some(owner.to_owned());
            inspection.repository.github_repository = Some(name.to_owned());
        }
    }
}

/// Normalize npm, crates.io, and `PyPI` publisher JSON to public settings.
#[must_use]
pub fn publisher_identities(document: &Value) -> Vec<PublisherIdentity> {
    if let Some(items) = document.as_array() {
        return items.iter().flat_map(publisher_identities).collect();
    }
    if document["type"]
        .as_str()
        .is_some_and(|kind| kind != "github")
    {
        return Vec::new();
    }
    for key in ["github_configs", "publishers", "trustedPublishers"] {
        if document[key].is_array() {
            return publisher_identities(&document[key]);
        }
    }
    let owned = document["repository_owner"]
        .as_str()
        .zip(document["repository_name"].as_str())
        .map(|(owner, name)| format!("{owner}/{name}"));
    let repository = document["claims"]["repository"]
        .as_str()
        .or_else(|| document["repository"].as_str())
        .or(owned.as_deref())
        .and_then(github_slug);
    let Some(repository) = repository else {
        return Vec::new();
    };
    vec![PublisherIdentity {
        id: document["id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| document["id"].as_u64().map(|id| id.to_string())),
        repository,
        workflow: document["claims"]["workflow_ref"]["file"]
            .as_str()
            .or_else(|| document["workflow_filename"].as_str())
            .or_else(|| document["workflow"].as_str())
            .or_else(|| document["file"].as_str())
            .map(str::to_owned),
        environment: document["claims"]["environment"]
            .as_str()
            .or_else(|| document["environment"].as_str())
            .map(str::to_owned),
    }]
}

/// Parse npm's interactive human-readable list, including multiple publishers.
#[must_use]
pub fn npm_publishers(output: &str) -> Vec<PublisherIdentity> {
    if let Ok(document) = serde_json::from_str(output) {
        return publisher_identities(&document);
    }
    let ansi = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("constant ANSI pattern");
    let output = ansi.replace_all(output, "");
    let mut result = Vec::new();
    let mut fields = serde_json::Map::new();
    for line in output.lines() {
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        if key == "type" && !fields.is_empty() {
            result.extend(publisher_identities(&Value::Object(std::mem::take(
                &mut fields,
            ))));
        }
        if ["id", "type", "repository", "file", "environment"].contains(&key) {
            fields.insert(key.to_owned(), Value::String(value.trim().to_owned()));
        }
    }
    result.extend(publisher_identities(&Value::Object(fields)));
    result
}

/// Require repository, workflow, and environment equality for verification.
#[must_use]
pub fn publisher_matches(
    publisher: &PublisherIdentity,
    expected: &TrustedPublisherPrefill,
) -> bool {
    publisher.repository.eq_ignore_ascii_case(&format!(
        "{}/{}",
        expected.organization, expected.repository
    )) && publisher.workflow.as_deref() == Some(&expected.workflow)
        && publisher.environment == expected.environment
}

/// Extract npm SLSA and `PyPI` publisher repository identities from provenance.
#[must_use]
pub fn provenance_repositories(document: &Value) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(bundles) = document["attestation_bundles"].as_array() {
        for bundle in bundles {
            if bundle["publisher"]["kind"] == "GitHub" {
                if let Some(url) = bundle["publisher"]["repository"].as_str() {
                    urls.push(url.to_owned());
                }
            }
        }
    }
    if let Some(attestations) = document["attestations"].as_array() {
        for item in attestations {
            let Some(payload) = item["bundle"]["dsseEnvelope"]["payload"]
                .as_str()
                .filter(|payload| payload.len() <= 2 * 1024 * 1024)
            else {
                continue;
            };
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(payload) else {
                continue;
            };
            let Ok(statement) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            for url in [
                statement["predicate"]["invocation"]["configSource"]["uri"].as_str(),
                statement["predicate"]["buildDefinition"]["externalParameters"]["workflow"]
                    ["repository"]
                    .as_str(),
            ]
            .into_iter()
            .flatten()
            {
                urls.push(url.to_owned());
            }
        }
    }
    let mut repositories: Vec<_> = urls.iter().filter_map(|url| github_slug(url)).collect();
    repositories.sort();
    repositories.dedup();
    repositories
}
