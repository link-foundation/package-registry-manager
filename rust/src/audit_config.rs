//! Explicit, reviewable source decisions supplement static discovery.
use crate::{model::Package, workflows::Workflow};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::Path;

pub fn read(root: &Path) -> Result<Value> {
    let config: Value = match std::fs::read_to_string(root.join(".package-registry-manager.json")) {
        Ok(text) => serde_json::from_str(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.into()),
    };
    if let Some(paths) = config.get("publish") {
        if !paths.as_array().is_some_and(|paths| {
            paths.iter().all(|p| {
                p.as_str().is_some_and(|p| {
                    !p.starts_with('/')
                        && !p.contains('\\')
                        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
                })
            })
        }) {
            bail!("publish must list exact repository-relative manifest paths");
        }
    }
    Ok(config)
}
pub fn opted_in(config: &Value, manifest: &str) -> bool {
    config["publish"]
        .as_array()
        .is_some_and(|paths| paths.iter().any(|path| path == manifest))
}
pub fn apply(items: &mut [Package], config: &Value, workflows: &[Workflow]) -> Result<()> {
    let valid = regex::Regex::new(r"^[\w.-]+$")?;
    for item in items {
        if let Some(entry) = config["gradle"]
            .as_array()
            .and_then(|items| items.iter().find(|v| v["manifest"] == item.manifest))
        {
            if !["group", "artifact", "version", "component"]
                .iter()
                .all(|key| entry[key].as_str().is_some_and(|s| valid.is_match(s)))
            {
                bail!("explicit Gradle publication requires resolved group, artifact, version and component");
            }
            item.registry = crate::Registry::MavenCentral;
            item.name = format!(
                "{}:{}",
                entry["group"].as_str().unwrap_or_default(),
                entry["artifact"].as_str().unwrap_or_default()
            );
            item.version = entry["version"].as_str().map(str::to_owned);
            item.publishable = true;
            item.audit.remove("discovery_skip");
            item.audit
                .insert("publication_component".into(), entry["component"].clone());
        }
        if let Some(mapping) = config["shared_artifacts"].as_array().and_then(|m| {
            m.iter()
                .find(|m| m["registry"] == item.registry.to_string() && m["name"] == item.name)
        }) {
            item.audit.insert("shared_artifact".into(), mapping.clone());
        }
        if let Some(distribution) = item
            .audit
            .get_mut("distribution")
            .and_then(Value::as_object_mut)
        {
            if let Some(settings) = config["distribution"][&item.manifest].as_object() {
                distribution.extend(settings.clone());
            }
            let pattern = match item.registry {
                crate::Registry::Conan => r"conan\s+upload",
                crate::Registry::Vcpkg => r"vcpkg.*(?:x-add-version|registry|publish)",
                _ => r"cpack|cmake.*--install",
            };
            let pattern = regex::Regex::new(pattern)?;
            distribution.insert("workflows".into(),json!(workflows.iter().filter(|w|pattern.is_match(&w.contents)).map(|w|json!({"workflow":w.name,"remote_variable":w.contents.contains("CONAN_REMOTE_URL").then_some("CONAN_REMOTE_URL"),"credential_names":(["CONAN_LOGIN_USERNAME","CONAN_PASSWORD"].iter().filter(|n|w.contents.contains(**n)).collect::<Vec<_>>())})).collect::<Vec<_>>()));
        }
    }
    Ok(())
}
