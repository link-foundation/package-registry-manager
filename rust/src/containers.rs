//! Docker Hub and GitHub Container Registry image detection.

use regex::Regex;

use crate::model::{Package, Registry};
use crate::publishers::token_secrets;
use crate::source_code::strip_comments;
use crate::workflows::{grants_packages_write, publishing_workflow, Workflow};

/// File names that describe a container image.
pub const CONTAINER_FILES: [&str; 2] = ["Dockerfile", "Containerfile"];

const HOSTS: [(Registry, &str); 2] = [
    (Registry::DockerHub, r"docker\.io"),
    (Registry::Ghcr, r"ghcr\.io"),
];

/// Describe Docker Hub and GHCR images built from a Dockerfile and pushed by a
/// GitHub Actions workflow. `dockerfiles` are repository-relative paths.
#[must_use]
pub fn container_packages(
    dockerfiles: &[String],
    workflows: &[Workflow],
    owner: Option<&str>,
    repository: Option<&str>,
) -> Vec<Package> {
    let Some(manifest) = dockerfiles.iter().min_by(|left, right| {
        (left.split('/').count(), left.as_str()).cmp(&(right.split('/').count(), right.as_str()))
    }) else {
        return Vec::new();
    };
    let mut packages = Vec::new();
    for (registry, host) in HOSTS {
        let Some(workflow) = publishing_workflow(workflows, registry) else {
            continue;
        };
        let name = literal_image(&strip_comments(&workflow.contents, "workflow.yml"), host)
            .or_else(|| default_image(owner, repository));
        let mut item = Package::new(
            registry,
            name.clone().unwrap_or_else(|| "unknown-image".to_owned()),
            None,
            manifest.clone(),
        );
        item.workflow = Some(workflow.name.clone());
        item.token_secrets = token_secrets(workflows, registry)
            .into_iter()
            .map(|secret| secret.secret)
            .collect();
        if name.is_none() {
            item = item.unpublishable(
                "the image name could not be derived from the workflow or a GitHub remote",
            );
        }
        if registry == Registry::Ghcr && !grants_packages_write(&workflow.contents) {
            item.warnings.push(format!(
                "{} does not grant packages: write, so GITHUB_TOKEN cannot push to ghcr.io",
                workflow.name
            ));
        }
        packages.push(item);
    }
    packages
}

fn literal_image(contents: &str, host: &str) -> Option<String> {
    Regex::new(&format!(
        r"(?i)\b{host}/([a-z0-9][a-z0-9._-]*/[a-z0-9][a-z0-9._/-]*[a-z0-9])"
    ))
    .expect("static pattern must compile")
    .captures(contents)
    .map(|captures| captures[1].to_lowercase())
}

fn default_image(owner: Option<&str>, repository: Option<&str>) -> Option<String> {
    Some(format!("{}/{}", owner?, repository?).to_lowercase())
}
