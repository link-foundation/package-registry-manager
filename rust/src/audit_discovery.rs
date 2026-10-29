//! Static artifact discovery. Never execute repository recipes while inspecting.
use crate::model::{Package, Registry};
use anyhow::{bail, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::{fs, path::Path};

fn capture(text: &str, expression: &str) -> Option<String> {
    Regex::new(expression)
        .expect("static regex")
        .captures(text)?
        .get(1)
        .map(|v| v.as_str().to_owned())
}

/// Expand generated npm artifacts before workflow assignment and identity grouping.
pub fn aliases(root: &Path, packages: &mut Vec<Package>) -> Result<()> {
    let mut aliases = Vec::new();
    let valid = Regex::new(r"^(?:@[a-z0-9._-]+/)?[a-z0-9][a-z0-9._-]*$")?;
    for item in packages.iter_mut().filter(|p| p.registry == Registry::Npm) {
        let data: Value = serde_json::from_str(
            fs::read_to_string(root.join(&item.manifest))?.trim_start_matches('\u{feff}'),
        )?;
        let Some(names) = data.get("publishAliases") else {
            continue;
        };
        let Some(names) = names.as_array() else {
            bail!("{}: publishAliases must be an array", item.manifest)
        };
        let mut seen = std::collections::BTreeSet::from([item.name.clone()]);
        let generator = data["scripts"]
            .as_object()
            .into_iter()
            .flat_map(|v| v.values())
            .filter_map(Value::as_str)
            .find_map(|script| {
                capture(
                    script,
                    r"(?:^|\s)(?:node|bun)\s+(?:\./)?([^\s]+prepare[^\s]*\.mjs)",
                )
            });
        let all_names: Vec<_> = std::iter::once(json!(item.name))
            .chain(names.iter().cloned())
            .collect();
        let metadata = json!({"source_manifest":item.manifest,"source_name":item.name,"generator":generator,"names":all_names});
        if !names.is_empty() {
            item.audit
                .insert("generated_artifact".into(), metadata.clone());
        }
        for name in names {
            let Some(name) = name.as_str().filter(|name| valid.is_match(name)) else {
                bail!("{}: invalid publishAliases name", item.manifest)
            };
            if !seen.insert(name.to_owned()) {
                bail!(
                    "{}: publishAliases must contain distinct names",
                    item.manifest
                )
            }
            let mut alias = item.clone();
            name.clone_into(&mut alias.name);
            alias
                .audit
                .insert("generated_alias".into(), metadata.clone());
            aliases.push(alias);
        }
    }
    packages.extend(aliases);
    Ok(())
}

/// Resolve Cargo's inherited version only from the declared/nearest workspace.
pub fn cargo_version(root: &Path, manifest: &Path, text: &str) -> Option<String> {
    let data: toml::Value = toml::from_str(text).ok()?;
    if data["package"].get("version")?.get("workspace")?.as_bool() != Some(true) {
        return None;
    }
    let directory = manifest.parent()?;
    let explicit = data["package"]
        .get("workspace")
        .and_then(toml::Value::as_str);
    let paths = explicit.map_or_else(
        || {
            directory
                .ancestors()
                .take_while(|p| p.starts_with(root))
                .map(Path::to_path_buf)
                .collect()
        },
        |workspace| vec![directory.join(workspace)],
    );
    for path in paths {
        let Ok(text) = fs::read_to_string(path.join("Cargo.toml")) else {
            continue;
        };
        let Ok(data) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        if let Some(workspace) = data.get("workspace") {
            return workspace
                .get("package")?
                .get("version")?
                .as_str()
                .map(str::to_owned);
        }
    }
    None
}

/// A Gradle build is publishable only with resolved coordinates and publication inputs.
#[must_use]
pub fn gradle(text: &str, manifest: String) -> Package {
    let literal = |key: &str| capture(text, &format!(r#"\b{key}\s*(?:=\s*)?['"]([^'"]+)"#));
    let group = literal("groupId").or_else(|| literal("group"));
    let artifact = literal("artifactId")
        .or_else(|| literal("archivesBaseName"))
        .or_else(|| literal(r"rootProject\.name"));
    let version = literal("version");
    let publish = Regex::new(r"maven-publish|MavenPublication")
        .expect("regex")
        .is_match(text);
    let input = Regex::new(r"from\s*\(?\s*components|artifact\s*\(")
        .expect("regex")
        .is_match(text);
    if let (Some(group), Some(artifact), Some(version)) = (group, artifact, version) {
        if publish && input && !format!("{group}{artifact}{version}").contains(['$', '{', '}']) {
            return Package::new(
                Registry::MavenCentral,
                format!("{group}:{artifact}"),
                Some(version),
                manifest,
            );
        }
    }
    let mut item = Package::new(Registry::MavenCentral, String::new(), None, manifest)
        .unpublishable("unresolved Gradle Maven publication");
    item.audit.insert("discovery_skip".into(), json!("Gradle application/build definition has no resolved Maven publication coordinates and component"));
    item
}

/// Read `CMake`, Conan and vcpkg distribution identities without running their code.
pub fn cpp(root: &Path, manifest: &Path, relative: String, text: &str) -> Result<Package> {
    let filename = manifest
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    let cmake = if filename == "CMakeLists.txt" {
        text.to_owned()
    } else {
        fs::read_to_string(manifest.with_file_name("CMakeLists.txt")).unwrap_or_default()
    };
    let project = capture(&cmake, r"(?i)\bproject\s*\(\s*([\w.-]+)\s+VERSION\s+\d");
    let project_version = capture(
        &cmake,
        r"(?i)\bproject\s*\(\s*[\w.-]+\s+VERSION\s+(\d+(?:\.\d+)+)",
    );
    let registry = match filename {
        "vcpkg.json" => Registry::Vcpkg,
        "conanfile.py" => Registry::Conan,
        _ => Registry::Cmake,
    };
    let (name, version) = match registry {
        Registry::Vcpkg => {
            let data: Value = serde_json::from_str(text)?;
            (
                data["name"].as_str().map(str::to_owned),
                [
                    "version",
                    "version-semver",
                    "version-string",
                    "version-date",
                ]
                .into_iter()
                .find_map(|key| data[key].as_str().map(str::to_owned)),
            )
        }
        Registry::Conan => (
            capture(text, r#"(?m)^\s*name\s*=\s*['"]([^'"]+)"#).or_else(|| project.clone()),
            capture(text, r#"(?m)^\s*version\s*=\s*['"]([^'"]+)"#).or(project_version),
        ),
        _ => (project.clone(), project_version),
    };
    let mut item = Package::new(registry, name.unwrap_or_default(), version, relative);
    if item.name.is_empty()
        || registry == Registry::Cmake
            && !Regex::new(r"(?i)install\s*\(|CPack|configure_package_config_file")?.is_match(text)
    {
        item.audit.insert(
            "discovery_skip".into(),
            json!("CMake build definition has no distribution installation/configuration"),
        );
        item.publishable = false;
    }
    let source = manifest.with_file_name("CMakeLists.txt");
    item.audit.insert("distribution".into(), json!({"version_source":if project.is_some(){source.strip_prefix(root).unwrap_or(&source).to_string_lossy().into_owned()}else{item.manifest.clone()},"provider_status":"unverified"}));
    if registry == Registry::Conan {
        item.audit.get_mut("distribution").expect("inserted")["remote_required"] =
            json!("CONAN_REMOTE_URL");
        item.audit.get_mut("distribution").expect("inserted")["credential_names"] =
            json!(["CONAN_LOGIN_USERNAME", "CONAN_PASSWORD"]);
    }
    item.warnings.push(format!(
        "{registry} distribution metadata does not establish a public registry publication"
    ));
    Ok(item)
}
