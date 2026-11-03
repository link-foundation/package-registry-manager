//! Version sidecars for account scans: the files local discovery reads next to
//! the fetched manifests, fetched bounded and at the same commit.
use crate::github::GithubGateway;
use crate::swift_distribution::{parse_swift_manifest, swift_version_candidates};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

/// At most this many sidecars per repository; the rest are reported.
pub const SIDECAR_LIMIT: usize = 200;

const GRADLE_SIDECARS: [&str; 4] = [
    "gradle.properties",
    "settings.gradle",
    "settings.gradle.kts",
    "gradle/libs.versions.toml",
];

static SAFE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.+-]+(?:/[\w.+-]+)*$").expect("static pattern"));
static CMAKE_READ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\bfile\s*\(\s*READ\s+"?\$\{CMAKE_CURRENT_(?:SOURCE|LIST)_DIR\}/([\w./+-]+)"?\s+\w+"#,
    )
    .expect("static pattern")
});
/// `cat go/VERSION`, `< cpp/VERSION`, `version-file: swift/VERSION` and the like;
/// the surrounding characters are checked separately (no lookaround in `regex`).
static WORKFLOW_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:[\w.+-]+/)*VERSION").expect("static pattern"));

/// Sidecar paths to fetch, those beyond the limit, and unsafe references.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidecarSelection {
    /// Sorted repository-relative paths within [`SIDECAR_LIMIT`].
    pub paths: Vec<String>,
    /// Sorted paths beyond the limit; never fetched.
    pub skipped: Vec<String>,
    /// Escaping or unsafe references; never followed.
    pub rejected: Vec<String>,
}

/// Sidecar fetch outcome recorded on the scanned repository.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarFetch {
    /// Paths requested at the scanned commit.
    pub requested: Vec<String>,
    /// Requested paths that were not returned.
    pub missing: Vec<String>,
    /// The whole sidecar read failed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

/// POSIX-style lexical join and normalization that keeps leading `..`.
fn join(base: &str, reference: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in base.split('/').chain(reference.split('/')) {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or((".", path))
}

/// Repository-relative version sources local discovery reads next to the
/// fetched manifests.
///
/// Sibling VERSION files (Go, C++, `SwiftPM`), `CMake`
/// `file(READ ...)` targets, `SwiftPM` target-directory VERSION files, Gradle
/// properties/settings/version catalogs, and VERSION files that workflow
/// commands name. Nothing is executed; unsafe or escaping references are
/// rejected, not followed.
#[must_use]
pub fn version_sidecars(files: &[Value]) -> SidecarSelection {
    let known: BTreeSet<&str> = files
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    let mut wanted = BTreeSet::new();
    let mut rejected = BTreeSet::new();
    let mut add = |base: &str, reference: &str| {
        let joined = join(base, reference);
        if reference.starts_with('/')
            || joined == ".."
            || joined.starts_with("../")
            || !SAFE_PATH.is_match(&joined)
            || joined.split('/').any(|part| part == ".git" || part == "..")
        {
            rejected.insert(joined);
        } else if !known.contains(joined.as_str()) {
            wanted.insert(joined);
        }
    };
    for file in files {
        let Some(path) = file["path"].as_str() else {
            continue;
        };
        let (directory, name) = split(path);
        let text = file["content"].as_str().unwrap_or_default();
        if matches!(name, "go.mod" | "CMakeLists.txt") {
            add(directory, "VERSION");
        }
        if name == "CMakeLists.txt" {
            for captures in CMAKE_READ.captures_iter(text) {
                add(directory, &captures[1]);
            }
        }
        if name == "Package.swift" {
            for candidate in swift_version_candidates(path, &parse_swift_manifest(text)) {
                add(".", &candidate);
            }
        }
        if matches!(name, "build.gradle" | "build.gradle.kts") {
            for sidecar in GRADLE_SIDECARS {
                add(directory, sidecar);
            }
        }
        if path.starts_with(".github/workflows/") {
            for found in WORKFLOW_VERSION.find_iter(text) {
                let before = text[..found.start()].chars().next_back();
                let after = text[found.end()..].chars().next();
                if before.is_none_or(|c| c.is_whitespace() || "\"'=:(<".contains(c))
                    && after.is_none_or(|c| c.is_whitespace() || "\"')|;".contains(c))
                {
                    add(".", found.as_str().trim_start_matches("./"));
                }
            }
        }
    }
    let mut paths: Vec<String> = wanted.into_iter().collect();
    let skipped = paths.split_off(paths.len().min(SIDECAR_LIMIT));
    SidecarSelection {
        paths,
        skipped,
        rejected: rejected.into_iter().collect(),
    }
}

/// Fetches the version sidecars at the same commit as the manifests.
///
/// Exact matches are appended to `files`. A failed read is an explicit
/// repository error; the peer packages found from the manifests are kept.
#[allow(clippy::future_not_send)] // Injectable GitHub gateways need not implement Send.
pub async fn fetch_version_sidecars(
    github: &impl GithubGateway,
    slug: &str,
    files: &mut Vec<Value>,
    reference: Option<&str>,
    cwd: &Path,
    mut on_error: impl FnMut(Value),
) -> SidecarFetch {
    let selection = version_sidecars(files);
    for rejected in &selection.rejected {
        on_error(json!({
            "stage": "sidecar",
            "manifest": rejected,
            "message": "unsafe version file reference was not followed",
        }));
    }
    if !selection.skipped.is_empty() {
        on_error(json!({
            "stage": "sidecar",
            "message": format!(
                "{} version files beyond the {SIDECAR_LIMIT}-file limit were not fetched; their packages keep unknown versions",
                selection.skipped.len()
            ),
        }));
    }
    if selection.paths.is_empty() {
        return SidecarFetch::default();
    }
    let mut args: Vec<String> = vec![
        "repo".into(),
        "files".into(),
        slug.into(),
        "--content".into(),
    ];
    if let Some(reference) = reference {
        args.extend(["--branch".into(), reference.into()]);
    }
    for path in &selection.paths {
        args.extend(["--match".into(), path.clone()]);
    }
    let fetched = match github.call(&args, None, cwd).await {
        Ok(Value::Array(fetched)) => fetched,
        result => {
            let reason = result.err().map_or_else(
                || "gh-manager returned an invalid file list".to_owned(),
                |error| error.to_string(),
            );
            on_error(json!({
                "stage": "sidecar",
                "message": format!("version files unavailable ({reason}); affected versions stay unknown"),
            }));
            return SidecarFetch {
                requested: selection.paths.clone(),
                missing: selection.paths,
                failed: true,
            };
        }
    };
    let exact: Vec<Value> = fetched
        .into_iter()
        .filter(|file| {
            file["path"]
                .as_str()
                .is_some_and(|path| selection.paths.iter().any(|wanted| wanted == path))
        })
        .collect();
    let found: BTreeSet<String> = exact
        .iter()
        .filter_map(|file| file["path"].as_str().map(str::to_owned))
        .collect();
    files.extend(exact);
    SidecarFetch {
        missing: selection
            .paths
            .iter()
            .filter(|path| !found.contains(*path))
            .cloned()
            .collect(),
        requested: selection.paths,
        failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_and_escaping_references() {
        let result = version_sidecars(&[
            json!({"path":"cpp/CMakeLists.txt","content":"file(READ \"${CMAKE_CURRENT_SOURCE_DIR}/../../etc/VERSION\" V)\nfile(READ \"${CMAKE_CURRENT_SOURCE_DIR}/version.txt\" W)"}),
            json!({"path":".github/workflows/r.yml","content":"run: cat ../VERSION && cat go/VERSION"}),
            json!({"path":"go/go.mod","content":"module x"}),
        ]);
        assert_eq!(
            result.paths,
            ["cpp/VERSION", "cpp/version.txt", "go/VERSION"]
        );
        assert_eq!(result.rejected, ["../VERSION", "../etc/VERSION"]);
    }

    #[test]
    fn bounds_the_number_of_sidecars() {
        let files: Vec<Value> = (0..SIDECAR_LIMIT + 5)
            .map(|index| json!({"path": format!("m{index:03}/go.mod"), "content": "module x"}))
            .collect();
        let result = version_sidecars(&files);
        assert_eq!(result.paths.len(), SIDECAR_LIMIT);
        assert_eq!(result.skipped.len(), 5);
    }

    #[test]
    fn workflow_references_need_word_boundaries() {
        let result = version_sidecars(&[json!({
            "path": ".github/workflows/r.yml",
            "content": "run: cat ./cpp/VERSION\nversion-file: swift/VERSION\nx: MYVERSION go/VERSIONS",
        })]);
        assert_eq!(result.paths, ["cpp/VERSION", "swift/VERSION"]);
    }
}
