//! Read-only version evidence, separate from ownership and publication authority.
use crate::model::{Package, Registry};
use crate::registry_state::{encode_uri_component as encode, Lookup, PackageState, RegistryClient};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;

static NUGET_VERSION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)^[0-9]+(?:\.[0-9]+){0,3}(?:-[0-9a-z-]+(?:\.[0-9a-z-]+)*)?(?:\+[0-9a-z-]+(?:\.[0-9a-z-]+)*)?$")
        .expect("static NuGet version regex")
});
static GO_VERSION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+incompatible)?$")
        .expect("static Go version regex")
});

// Proxy responses require canonical SemVer, with only +incompatible metadata.
fn canonical_go_version(version: &str) -> bool {
    GO_VERSION.captures(version).is_some_and(|parts| {
        parts.get(1).is_none_or(|pre| {
            pre.as_str().split('.').all(|part| {
                !(part.len() > 1
                    && part.starts_with('0')
                    && part.bytes().all(|c| c.is_ascii_digit()))
            })
        })
    })
}

/// Normalize the `NuGet` release identity used by the public flat-container API.
#[must_use]
pub fn normalized_nuget_version(version: &str) -> Option<String> {
    if !NUGET_VERSION.is_match(version) {
        return None;
    }
    let public = version.split('+').next()?.to_ascii_lowercase();
    let (release, prerelease) = public
        .split_once('-')
        .map_or((public.as_str(), None), |(a, b)| (a, Some(b)));
    let mut parts: Vec<_> = release.split('.').collect();
    if parts.is_empty()
        || parts.len() > 4
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    let normalized: Vec<_> = parts
        .iter()
        .map(|p| {
            let p = p.trim_start_matches('0');
            if p.is_empty() {
                "0"
            } else {
                p
            }
        })
        .collect();
    parts = normalized;
    while parts.len() < 3 {
        parts.push("0");
    }
    if parts.len() == 4 && parts[3] == "0" {
        parts.pop();
    }
    let mut version = parts.join(".");
    if let Some(prerelease) = prerelease {
        version.push('-');
        version.push_str(prerelease);
    }
    Some(version)
}

fn insert(fields: &mut BTreeMap<String, Value>, key: &str, value: &Value) {
    if !value.is_null() {
        fields.insert(key.into(), value.clone());
    }
}
/// Numeric releases can be ordered; other version formats remain incomparable.
#[must_use]
pub fn older(expected: &str, latest: &str) -> bool {
    let parse = |value: &str| -> Option<Vec<u64>> {
        let parts = value
            .trim_start_matches('v')
            .split('.')
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()
            .ok()?;
        (parts.len() == 3).then_some(parts)
    };
    parse(expected)
        .zip(parse(latest))
        .is_some_and(|(a, b)| a < b)
}
/// Attach exact-version and public source evidence to a successful name lookup.
pub async fn evidence(
    client: &RegistryClient,
    item: &Package,
    doc: &Value,
    url: &str,
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::from([("registry_evidence_url".into(), json!(url))]);
    let latest = match item.registry {
        Registry::Npm => &doc["version"],
        Registry::CratesIo => &doc["crate"]["max_version"],
        Registry::PyPi => &doc["info"]["version"],
        Registry::NuGet => doc["versions"]
            .as_array()
            .and_then(|v| v.last())
            .unwrap_or(&Value::Null),
        Registry::Packagist => &doc["packages"][&item.name][0]["version"],
        Registry::Jsr => &doc["latest"],
        _ => &Value::Null,
    };
    insert(&mut out, "latest_version", latest);
    if item.registry == Registry::Npm {
        insert(
            &mut out,
            "registry_repository",
            doc["repository"]
                .get("url")
                .unwrap_or_else(|| &doc["repository"]),
        );
        if let Some(owners) = doc["maintainers"].as_array() {
            out.insert(
                "registry_owners".into(),
                json!(owners
                    .iter()
                    .filter_map(|v| v["name"].as_str())
                    .collect::<Vec<_>>()),
            );
        }
    }
    if item.registry == Registry::CratesIo {
        insert(&mut out, "registry_repository", &doc["crate"]["repository"]);
    }
    let Some(version) = &item.version else {
        out.insert("version_state".into(), json!("manifest-version-unresolved"));
        return out;
    };
    let exact_url = match item.registry {
        Registry::Npm => Some(format!(
            "{}{version}",
            url.trim_end_matches("latest"),
            version = encode(version)
        )),
        Registry::CratesIo => Some(format!("{url}/{}", encode(version))),
        _ => None,
    };
    let exists = if let Some(exact_url) = exact_url {
        out.insert("version_evidence_url".into(), json!(exact_url));
        match client.get_json(&exact_url).await {
            Lookup::Missing => Some(false),
            Lookup::Found(value)
                if value["version"] == *version || value["version"]["num"] == *version =>
            {
                Some(true)
            }
            _ => {
                out.insert(
                    "registry_state_reason".into(),
                    json!("exact-version lookup failed or returned invalid metadata"),
                );
                None
            }
        }
    } else {
        out.insert("version_evidence_url".into(), json!(url));
        match item.registry {
            Registry::PyPi => doc["releases"].as_object().map(|v| v.contains_key(version)),
            Registry::Jsr => doc["versions"].as_object().map(|v| v.contains_key(version)),
            Registry::NuGet => doc["versions"].as_array().map(|v| {
                v.iter().any(|s| {
                    s.as_str()
                        .and_then(normalized_nuget_version)
                        .zip(normalized_nuget_version(version))
                        .is_some_and(|(a, b)| a == b)
                })
            }),
            Registry::Packagist => doc["packages"][&item.name].as_array().map(|v| {
                v.iter().any(|v| {
                    v["version"].as_str().is_some_and(|v| {
                        v.trim_start_matches('v') == version.trim_start_matches('v')
                    })
                })
            }),
            _ => {
                out.insert(
                    "registry_state_reason".into(),
                    json!("exact-version lookup is unsupported for this provider"),
                );
                None
            }
        }
    };
    if let Some(exists) = exists {
        out.insert("manifest_version_exists".into(), json!(exists));
        if !exists {
            out.insert(
                "version_state".into(),
                json!(if older(version, latest.as_str().unwrap_or_default()) {
                    "stale-manifest-version"
                } else {
                    "manifest-version-absent"
                }),
            );
        }
    }
    if !out.contains_key("manifest_version_exists") && out.contains_key("registry_state_reason") {
        out.insert("version_state".into(), json!("manifest-version-unknown"));
    }
    out
}
impl RegistryClient {
    async fn publication_text(&self, url: &str) -> Result<Option<String>, ()> {
        let response = self.client.get(url).send().await.map_err(|_| ())?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(());
        }
        response.text().await.map(Some).map_err(|_| ())
    }
    /// Maven XML and Go proxy endpoints are not general JSON registry documents.
    pub async fn distribution_state(&self, item: &Package) -> PackageState {
        let Some(base) = self.endpoints().base(item.registry) else {
            return PackageState::default();
        };
        let mut state = PackageState::default();
        if item.registry == Registry::MavenCentral {
            let Some((group, artifact)) = item.name.split_once(':').filter(|(g, a)| {
                [*g, *a].iter().all(|s| {
                    !s.is_empty()
                        && s.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
                })
            }) else {
                state.audit.insert(
                    "registry_state_reason".into(),
                    json!("unresolved Maven coordinates"),
                );
                return state;
            };
            let directory = format!("{base}/{}/{artifact}", group.replace('.', "/"));
            let url = format!("{directory}/maven-metadata.xml");
            state
                .audit
                .insert("registry_evidence_url".into(), json!(url));
            match self.publication_text(&url).await {
                Ok(None) => state.exists = Some(false),
                Ok(Some(text)) if text.contains("<metadata") => {
                    state.exists = Some(true);
                    if let Some(capture) = regex::Regex::new(r"<(?:release|latest)>([^<]+)</")
                        .expect("regex")
                        .captures(&text)
                    {
                        state
                            .audit
                            .insert("latest_version".into(), json!(&capture[1]));
                    }
                }
                _ => {}
            }
            if let Some(version) = &item.version {
                if !version.contains(['$', '{', '}', '/', '\\']) {
                    let url = format!(
                        "{directory}/{}/{artifact}-{}.pom",
                        encode(version),
                        encode(version)
                    );
                    state
                        .audit
                        .insert("version_evidence_url".into(), json!(url));
                    let exact = match self.publication_text(&url).await {
                        Ok(None) => Some(false),
                        Ok(Some(text))
                            if regex::Regex::new(r"<project[\s>]")
                                .expect("regex")
                                .is_match(&text) =>
                        {
                            Some(true)
                        }
                        _ => {
                            state.audit.insert("registry_state_reason".into(),json!("Maven exact-version lookup failed or returned invalid metadata"));
                            None
                        }
                    };
                    if let Some(exact) = exact {
                        state
                            .audit
                            .insert("manifest_version_exists".into(), json!(exact));
                        if exact {
                            state.exists = Some(true);
                        } else {
                            state.audit.insert(
                                "version_state".into(),
                                json!(if older(
                                    version,
                                    state
                                        .audit
                                        .get("latest_version")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                ) {
                                    "stale-manifest-version"
                                } else {
                                    "manifest-version-absent"
                                }),
                            );
                        }
                    }
                }
            } else {
                state
                    .audit
                    .insert("version_state".into(), json!("manifest-version-unresolved"));
            }
        } else {
            let module = item
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_uppercase() {
                        format!("!{}", c.to_ascii_lowercase())
                    } else {
                        c.to_string()
                    }
                })
                .collect::<String>();
            let url = format!("{base}/{module}/@v/list");
            state
                .audit
                .insert("registry_evidence_url".into(), json!(url));
            let list = self.publication_text(&url).await;
            let latest = self.get_json(&format!("{base}/{module}/@latest")).await;
            if let Lookup::Found(doc) = latest {
                if doc["Version"].as_str().is_some_and(canonical_go_version) {
                    state.exists = Some(true);
                    state
                        .audit
                        .insert("latest_version".into(), doc["Version"].clone());
                }
            } else if matches!(latest, Lookup::Missing) && matches!(list, Ok(None)) {
                state.exists = Some(false);
            }
            if let Ok(Some(list)) = list {
                if list
                    .lines()
                    .filter(|line| !line.is_empty())
                    .all(canonical_go_version)
                {
                    let tagged = !list.trim().is_empty();
                    state
                        .audit
                        .insert("tagged_release_exists".into(), json!(tagged));
                    if !tagged && state.exists == Some(true) {
                        state
                            .audit
                            .insert("version_state".into(), json!("no-tagged-release"));
                    }
                } else {
                    state.audit.insert(
                        "registry_state_reason".into(),
                        json!("Go proxy tagged-version list returned invalid metadata"),
                    );
                }
            }
        }
        if state.exists.is_none() {
            state.audit.insert(
                "registry_state_reason".into(),
                json!("lookup failed or returned invalid metadata"),
            );
        }
        state
    }
}
