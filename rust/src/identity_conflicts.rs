//! Group every source of a registry identity before selecting mutation targets.
use crate::model::{Inspection, Package, Registry, RepositoryInfo, SetupPlan};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Registry-specific public identity normalization.
#[must_use]
pub fn identity(item: &Package) -> String {
    let mut name = if item.registry == Registry::GoModules {
        item.name.clone()
    } else {
        item.name.to_lowercase()
    };
    if item.registry == Registry::PyPi {
        name = regex::Regex::new("[-_.]+")
            .expect("regex")
            .replace_all(&name, "-")
            .into_owned();
    }
    if item.registry == Registry::CratesIo {
        name = name.replace('_', "-");
    }
    format!("{}:{name}", item.registry)
}
fn groups<'a>(
    sources: impl Iterator<Item = (&'a RepositoryInfo, &'a Package)>,
) -> BTreeMap<String, Vec<Value>> {
    let mut groups = BTreeMap::<String, Vec<Value>>::new();
    for (repo, item) in sources.filter(|(_, item)| item.publishable) {
        let repository =
            crate::repository_identity::repository_slug(repo).unwrap_or_else(|| repo.root.clone());
        let source = json!({"repository":repository,"manifest":item.manifest,"source_artifact":item.audit.get("generated_alias").and_then(|v|v.get("source_manifest")).cloned().unwrap_or_else(||json!(item.manifest)),"version":item.version,"registry_repository":item.audit.get("registry_repository"),"registry_owners":item.audit.get("registry_owners"),"permissions":"unknown","shared_artifact":item.audit.get("shared_artifact")});
        let group = groups.entry(identity(item)).or_default();
        if !group
            .iter()
            .any(|v| v["repository"] == source["repository"] && v["manifest"] == source["manifest"])
        {
            group.push(source);
        }
    }
    groups.retain(|_, sources| sources.len() > 1);
    groups
}
/// Annotate sources before filtering, including all cross-repository collisions.
pub fn mark(inspections: &mut [&mut Inspection]) -> Vec<Value> {
    let conflicts = groups(inspections.iter().flat_map(|inspection| {
        inspection
            .packages
            .iter()
            .map(|item| (&inspection.repository, item))
    }));
    for inspection in inspections {
        for item in &mut inspection.packages {
            if let Some(sources) = conflicts.get(&identity(item)) {
                item.audit
                    .insert("identity_conflicts".into(), json!(sources));
            }
        }
    }
    conflicts
        .into_iter()
        .map(|(identity, sources)| json!({"identity":identity,"sources":sources}))
        .collect()
}
/// Fail the entire batch before any registry or GitHub settings can change.
pub fn validate(plans: &[SetupPlan]) -> Result<()> {
    let conflicts = groups(plans.iter().map(|plan| (&plan.repository, &plan.package)));
    let annotated: Vec<_> = plans
        .iter()
        .filter_map(|p| {
            p.package
                .audit
                .get("identity_conflicts")
                .map(|sources| json!({"identity":identity(&p.package),"sources":sources}))
        })
        .collect();
    if conflicts
        .iter()
        .any(|(identity, sources)| !reviewed_mapping(identity, sources, plans))
        || annotated.iter().any(|group| {
            !reviewed_mapping(
                group["identity"].as_str().unwrap_or_default(),
                group["sources"].as_array().map_or(&[], Vec::as_slice),
                plans,
            )
        })
    {
        bail!(
            "conflicting package identities require distinct configured names before setup: {}",
            json!({"conflicts":conflicts,"sources":annotated})
        );
    }
    Ok(())
}

fn reviewed_mapping(group: &str, sources: &[Value], plans: &[SetupPlan]) -> bool {
    let Some(mapping) = sources.first().map(|s| &s["shared_artifact"]) else {
        return false;
    };
    if mapping["reviewed"] != true || mapping["artifact"].as_str().is_none_or(str::is_empty) {
        return false;
    }
    let Some(inventory) = mapping["sources"].as_array() else {
        return false;
    };
    let same = |a: &Value, b: &Value| {
        ["repository", "manifest", "version"]
            .iter()
            .all(|key| a[key] == b[key])
    };
    if inventory.len() != sources.len()
        || !sources.iter().all(|source| {
            source["shared_artifact"] == *mapping
                && inventory.iter().filter(|entry| same(source, entry)).count() == 1
        })
    {
        return false;
    }
    plans
        .iter()
        .filter(|plan| identity(&plan.package) == group)
        .all(|plan| {
            crate::repository_identity::repository_slug(&plan.repository)
                .unwrap_or_else(|| plan.repository.root.clone())
                == mapping["publisher"]["repository"]
                    .as_str()
                    .unwrap_or_default()
                && plan.package.manifest == mapping["publisher"]["manifest"]
        })
}
