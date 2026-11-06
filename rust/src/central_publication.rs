//! Exact public Gradle metadata graph verification; no Gradle execution or uploads.
use crate::central_packagist::{http, maven_path, StepContext};
use crate::github::GithubGateway;
use crate::model::{Registry, SetupStep};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn valid(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

fn resolve(value: &Value, parent: &str, base: &str) -> Option<String> {
    let url = value
        .as_str()
        .filter(|s| !s.is_empty() && !s.contains(['?', '#', '\\']))?;
    let resolved = reqwest::Url::parse(parent).ok()?.join(url).ok()?;
    resolved
        .as_str()
        .starts_with(base)
        .then(|| resolved.to_string())
}

/// Verify POM, exact platform metadata and availability of referenced files.
#[allow(clippy::future_not_send)]
pub async fn verify(context: &StepContext<'_, impl GithubGateway>, step: &SetupStep) -> Value {
    let coordinates = step.details.get("coordinates").cloned().unwrap_or_default();
    let (Some(group), Some(artifact), Some(version)) = (
        coordinates["group"].as_str(),
        coordinates["artifact"].as_str(),
        coordinates["version"].as_str(),
    ) else {
        return json!({"status":"unknown","reason":"the exact coordinates are unresolved"});
    };
    if ![group, artifact, version].into_iter().all(valid) {
        return json!({"status":"unknown","reason":"the exact coordinates are unresolved"});
    }
    let client = context.client;
    let path = |g: &str, a: &str, ext: &str| maven_path(client, g, a, version, ext);
    let root = http(client, &path(group, artifact, "pom"), true).await;
    if root.status != Some(200) {
        return json!({"status":if root.status == Some(404) {"unpublished"} else {"transport-unknown"},"reason":format!("root POM HTTP {}",root.shown())});
    }
    let mut evidence = json!({"root_pom":"available","artifacts":"unverified","signatures":"unverified","consumers":"unverified","release":"unverified"});
    if context
        .plan
        .package
        .audit
        .get("gradle")
        .is_none_or(|g| g["multiplatform"] != true)
    {
        return json!({"status":"verified","evidence":evidence});
    }
    let base = format!(
        "{}/",
        client
            .endpoints()
            .base(Registry::MavenCentral)
            .unwrap_or_default()
            .trim_end_matches('/')
    );
    let root_url = path(group, artifact, "module");
    let mut queue = vec![(group.to_owned(), artifact.to_owned())];
    let mut seen = BTreeSet::new();
    let mut queued = BTreeSet::from([format!("{group}:{artifact}")]);
    let mut root_missing = false;
    let mut files = BTreeSet::new();
    let mut missing = Vec::new();
    let mut malformed = Vec::new();
    let mut transport = Vec::new();
    let mut index = 0;
    while index < queue.len() {
        let (g, a) = queue[index].clone();
        index += 1;
        let coordinate = format!("{g}:{a}");
        if !seen.insert(coordinate.clone()) {
            continue;
        }
        if seen.len() > 128 {
            malformed.push("metadata graph exceeds 128 coordinates".into());
            break;
        }
        let url = path(&g, &a, "module");
        if index > 1 {
            let pom = http(client, &path(&g, &a, "pom"), true).await;
            if pom.status != Some(200) {
                let target = if pom.status == Some(404) {
                    &mut missing
                } else {
                    &mut transport
                };
                target.push(format!("{coordinate} POM (HTTP {})", pom.shown()));
            }
        }
        let answer = http(client, &url, false).await;
        if answer.status != Some(200) {
            if index == 1 && answer.status == Some(404) {
                root_missing = true;
            }
            let target = if answer.status == Some(404) {
                &mut missing
            } else {
                &mut transport
            };
            target.push(format!("{coordinate} module (HTTP {})", answer.shown()));
            continue;
        }
        let Some(doc) = answer.json.filter(|d| {
            d["formatVersion"] == "1.1"
                && d["variants"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty() && v.len() <= 4096)
        }) else {
            malformed.push(format!("{coordinate}: invalid Gradle 1.1 metadata"));
            continue;
        };
        let component = &doc["component"];
        let own =
            component["group"] == g && component["module"] == a && component["version"] == version;
        let redirected = component["group"] == group
            && component["module"] == artifact
            && component["version"] == version
            && resolve(&component["url"], &url, &base).as_deref() == Some(root_url.as_str());
        if !own && !redirected {
            malformed.push(format!("{coordinate}: incorrect component identity"));
            continue;
        }
        for variant in doc["variants"].as_array().expect("validated variants") {
            if !variant["name"].is_string()
                || (!variant["available-at"].is_null() && !variant["files"].is_null())
            {
                malformed.push(format!("{coordinate}: invalid variant"));
                continue;
            }
            if let Some(pointer) = variant.get("available-at") {
                let fields = ["group", "module", "version"].map(|key| pointer[key].as_str());
                let [Some(pg), Some(pa), Some(pv)] = fields else {
                    malformed.push(format!("{coordinate}: invalid available-at coordinate"));
                    continue;
                };
                if ![pg, pa, pv].into_iter().all(valid) || pv != version {
                    malformed.push(format!("{coordinate}: available-at must name a group/module at exact version {version}"));
                    continue;
                }
                if resolve(&pointer["url"], &url, &base).as_deref()
                    != Some(path(pg, pa, "module").as_str())
                {
                    malformed.push(format!(
                        "{coordinate}: available-at URL disagrees with coordinates"
                    ));
                    continue;
                }
                if queued.insert(format!("{pg}:{pa}")) {
                    queue.push((pg.into(), pa.into()));
                }
            }
            if variant.get("files").is_some_and(|f| !f.is_array()) {
                malformed.push(format!("{coordinate}: invalid files"));
                continue;
            }
            for file in variant["files"].as_array().into_iter().flatten() {
                let file_url = resolve(&file["url"], &url, &base);
                let name = file["name"].as_str().filter(|name| !name.is_empty());
                if file_url
                    .as_ref()
                    .is_none_or(|f| !f.starts_with(&url[..=url.rfind('/').expect("module path")]))
                    || name.is_none()
                    || file.get("size").is_some_and(|s| s.as_u64().is_none())
                {
                    malformed.push(format!("{coordinate}: invalid file record"));
                    continue;
                }
                let file_url = file_url.expect("checked URL");
                if !files.insert(file_url.clone()) {
                    continue;
                }
                if files.len() > 4096 {
                    malformed.push("metadata graph exceeds 4096 files".into());
                    break;
                }
                let result = http(client, &file_url, true).await;
                if result.status != Some(200) {
                    let target = if result.status == Some(404) {
                        &mut missing
                    } else {
                        &mut transport
                    };
                    target.push(format!(
                        "{coordinate}/{} (HTTP {})",
                        name.unwrap_or_default(),
                        result.shown()
                    ));
                }
            }
        }
    }
    for expected in step
        .details
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !seen.contains(expected) {
            missing.push(format!("{expected} (not in module metadata)"));
        }
    }
    let status = if !malformed.is_empty() {
        "malformed-metadata"
    } else if !transport.is_empty() {
        "transport-unknown"
    } else if root_missing {
        "root-metadata-missing"
    } else if !missing.is_empty() {
        "partial"
    } else {
        "verified"
    };
    seen.remove(&format!("{group}:{artifact}"));
    evidence["artifacts"] = json!(if status == "verified" {
        "referenced-files-available"
    } else {
        "incomplete"
    });
    evidence["hashes"] = json!("unverified");
    let mut result =
        json!({"status":status,"variants":seen,"files":files.len(),"evidence":evidence});
    for (key, values) in [
        ("missing", missing),
        ("malformed", malformed),
        ("transport", transport),
    ] {
        if !values.is_empty() {
            result[key] = json!(values);
        }
    }
    if status != "verified" {
        result["reason"] = json!("exact Gradle publication graph is incomplete");
    }
    result
}
