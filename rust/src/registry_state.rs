//! Public registry lookups: does a package exist, and does it use trusted publishing?

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use serde_json::Value;

use crate::model::{Inspection, Package, Registry};
use crate::tokens::{token_state, TokenState};

const USER_AGENT: &str =
    "package-registry-manager (+https://github.com/link-foundation/package-registry-manager)";

const fn endpoint_default(registry: Registry) -> Option<(&'static str, &'static str)> {
    match registry {
        Registry::Npm => Some((
            "PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY",
            "https://registry.npmjs.org",
        )),
        Registry::CratesIo => Some((
            "PACKAGE_REGISTRY_MANAGER_CRATES_IO_API",
            "https://crates.io/api/v1",
        )),
        Registry::PyPi => Some(("PACKAGE_REGISTRY_MANAGER_PYPI_API", "https://pypi.org")),
        Registry::DockerHub => Some((
            "PACKAGE_REGISTRY_MANAGER_DOCKER_HUB_API",
            "https://hub.docker.com/v2",
        )),
        _ => None,
    }
}

/// Registry API base URLs, overridable through `PACKAGE_REGISTRY_MANAGER_*`
/// environment variables for tests and mirrors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Endpoints {
    overrides: BTreeMap<String, String>,
}

impl Endpoints {
    /// Read overrides from the process environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            overrides: Registry::ALL
                .iter()
                .filter_map(|registry| endpoint_default(*registry))
                .filter_map(|(variable, _)| {
                    std::env::var(variable)
                        .ok()
                        .map(|value| (variable.to_owned(), value))
                })
                .collect(),
        }
    }

    /// Override one environment variable, such as `PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY`.
    #[must_use]
    pub fn with(mut self, variable: &str, value: &str) -> Self {
        self.overrides.insert(variable.to_owned(), value.to_owned());
        self
    }

    /// Return the API base URL of a registry without a trailing slash.
    #[must_use]
    pub fn base(&self, registry: Registry) -> Option<String> {
        let (variable, fallback) = endpoint_default(registry)?;
        let value = self
            .overrides
            .get(variable)
            .filter(|value| !value.is_empty())
            .map_or(fallback, String::as_str);
        Some(value.trim_end_matches('/').to_owned())
    }

    /// Return the public URL that answers whether a package exists.
    #[must_use]
    pub fn state_url(&self, package: &Package) -> Option<String> {
        let base = self.base(package.registry)?;
        let name = &package.name;
        Some(match package.registry {
            Registry::Npm => format!("{base}/{}/latest", npm_name(name)),
            Registry::CratesIo => format!("{base}/crates/{}", encode_uri_component(name)),
            Registry::PyPi => format!("{base}/pypi/{}/json", encode_uri_component(name)),
            _ => {
                let mut parts = name.split('/');
                let namespace = parts.next().unwrap_or_default();
                let repository = parts.next().unwrap_or("undefined");
                format!(
                    "{base}/namespaces/{}/repositories/{}",
                    encode_uri_component(namespace),
                    encode_uri_component(repository)
                )
            }
        })
    }
}

/// Encode like JavaScript `encodeURIComponent`.
#[must_use]
pub fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Encode an npm package name for a registry document URL.
#[must_use]
pub fn npm_name(name: &str) -> String {
    name.strip_prefix('@').map_or_else(
        || name.to_owned(),
        |rest| format!("@{}", encode_uri_component(rest)),
    )
}

/// The result of fetching a registry document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// The document exists.
    Found(Value),
    /// The registry answered 404.
    Missing,
    /// The state could not be determined.
    Unknown,
}

/// Registry state of one package; `None` means unknown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageState {
    /// Whether the package exists.
    pub exists: Option<bool>,
    /// Whether the latest release used trusted publishing.
    pub trusted: Option<bool>,
    /// Latest published version, when the registry reports it.
    pub version: Option<String>,
}

/// HTTP client for registry lookups.
#[derive(Debug, Clone)]
pub struct RegistryClient {
    client: reqwest::Client,
    endpoints: Endpoints,
    verbose: bool,
}

impl RegistryClient {
    /// Create a client with a 10-second timeout.
    #[must_use]
    pub fn new(endpoints: Endpoints, verbose: bool) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            client,
            endpoints,
            verbose,
        }
    }

    /// The endpoints used to build lookup URLs.
    #[must_use]
    pub const fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// Fetch a JSON document; 404 is `Missing`, anything else unexpected is `Unknown`.
    pub async fn get_json(&self, url: &str) -> Lookup {
        let response = self
            .client
            .get(url)
            .header("accept", "application/json")
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status();
                if self.verbose {
                    eprintln!("GET {url} -> {}", status.as_u16());
                }
                if status == reqwest::StatusCode::NOT_FOUND {
                    Lookup::Missing
                } else if status.is_success() {
                    response
                        .bytes()
                        .await
                        .ok()
                        .and_then(|body| serde_json::from_slice(&body).ok())
                        .map_or(Lookup::Unknown, Lookup::Found)
                } else {
                    Lookup::Unknown
                }
            }
            Err(error) => {
                if self.verbose {
                    eprintln!("GET {url} failed: {error}");
                }
                Lookup::Unknown
            }
        }
    }

    /// Ask crates.io whether a token still authenticates, through an
    /// endpoint only the website session may use. The token is sent only to
    /// `base` and never printed; `None` when the answer is unclear.
    pub async fn crates_token_state(&self, base: &str, token: &str) -> Option<TokenState> {
        let url = format!("{base}/me/tokens");
        let response = self
            .client
            .get(&url)
            .header("accept", "application/json")
            .header("authorization", token)
            .send()
            .await;
        match response {
            Ok(response) => {
                let status = response.status().as_u16();
                if self.verbose {
                    eprintln!("GET {url} -> {status}");
                }
                let body = response
                    .bytes()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_slice(&body).ok())
                    .unwrap_or(Value::Null);
                token_state(status, &body)
            }
            Err(error) => {
                if self.verbose {
                    eprintln!("GET {url} failed: {error}");
                }
                None
            }
        }
    }

    /// Probe one package; unknown fields stay `None`.
    pub async fn probe_package(&self, package: &Package) -> PackageState {
        let Some(url) = self.endpoints.state_url(package) else {
            return PackageState::default();
        };
        let document = match self.get_json(&url).await {
            Lookup::Unknown => return PackageState::default(),
            Lookup::Missing => {
                return PackageState {
                    exists: Some(false),
                    trusted: (package.registry != Registry::DockerHub).then_some(false),
                    version: None,
                }
            }
            Lookup::Found(document) => document,
        };
        match package.registry {
            Registry::Npm => PackageState {
                exists: Some(true),
                trusted: Some(npm_trusted(&document)),
                version: document["version"].as_str().map(str::to_owned),
            },
            Registry::CratesIo => PackageState {
                exists: Some(true),
                trusted: Some(document["versions"].as_array().is_some_and(|versions| {
                    versions.iter().any(|item| truthy(&item["trustpub_data"]))
                })),
                version: None,
            },
            Registry::PyPi => PackageState {
                exists: Some(true),
                trusted: Some(self.pypi_provenance(&document).await),
                version: None,
            },
            _ => PackageState {
                exists: Some(true),
                ..PackageState::default()
            },
        }
    }

    async fn pypi_provenance(&self, document: &Value) -> bool {
        let (Some(file), Some(version), Some(base)) = (
            document["urls"][0]["filename"].as_str(),
            document["info"]["version"].as_str(),
            self.endpoints.base(Registry::PyPi),
        ) else {
            return false;
        };
        let name = document["info"]["name"].as_str().unwrap_or_default();
        let url = format!(
            "{base}/integrity/{}/{}/{}/provenance",
            encode_uri_component(name),
            encode_uri_component(version),
            encode_uri_component(file)
        );
        matches!(self.get_json(&url).await, Lookup::Found(value) if truthy(&value))
    }

    /// Look up every publishable package concurrently. Unknown state is left
    /// unset so plans keep every conditional step.
    pub async fn probe_registry_state(&self, inspection: &Inspection) -> Inspection {
        let mut probed = inspection.clone();
        let mut tasks = tokio::task::JoinSet::new();
        for (index, package) in probed.packages.iter().enumerate() {
            if package.publishable {
                let client = self.clone();
                let package = package.clone();
                tasks.spawn(async move { (index, client.probe_package(&package).await) });
            }
        }
        while let Some(Ok((index, state))) = tasks.join_next().await {
            let package = &mut probed.packages[index];
            if state.exists.is_some() {
                package.exists_on_registry = state.exists;
            }
            if state.trusted.is_some() {
                package.trusted_publishing = state.trusted;
            }
        }
        probed
    }
}

/// Report whether an npm version document was published by a trusted publisher.
#[must_use]
pub fn npm_trusted(document: &Value) -> bool {
    truthy(&document["_npmUser"]["trustedPublisher"])
}

/// JavaScript-style truthiness for JSON values.
#[must_use]
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}
