//! Public registry lookups: does a package exist, and does it use trusted publishing?

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use serde_json::{json, Value};

use crate::model::{Inspection, Package, PublisherIdentity, Registry, TrustedPublisherPrefill};
use crate::repository_identity::{
    compare_repositories, identity_command, inspect_manifest_repositories, npm_publishers,
    provenance_repositories, publisher_identities, publisher_matches, resolve_repository,
};
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
        Registry::NuGet => Some((
            "PACKAGE_REGISTRY_MANAGER_NUGET_API",
            "https://api.nuget.org/v3-flatcontainer",
        )),
        Registry::RubyGems => Some((
            "PACKAGE_REGISTRY_MANAGER_RUBYGEMS_API",
            "https://rubygems.org/api/v1",
        )),
        Registry::Jsr => Some(("PACKAGE_REGISTRY_MANAGER_JSR_API", "https://jsr.io")),
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
            Registry::NuGet => format!(
                "{base}/{}/index.json",
                encode_uri_component(&name.to_lowercase())
            ),
            Registry::RubyGems => format!("{base}/gems/{}.json", encode_uri_component(name)),
            Registry::Jsr => format!(
                "{base}/{}/meta.json",
                name.split('/')
                    .map(|part| part.strip_prefix('@').map_or_else(
                        || encode_uri_component(part),
                        |scope| format!("@{}", encode_uri_component(scope))
                    ))
                    .collect::<Vec<_>>()
                    .join("/")
            ),
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
    /// HTTP status when existence could not be determined.
    pub registry_http_status: Option<u16>,
    /// Why owner-only publisher settings were not checked.
    pub publisher_settings_reason: Option<String>,
    /// Whether the package exists.
    pub exists: Option<bool>,
    /// Whether the latest release used trusted publishing.
    pub trusted: Option<bool>,
    /// Latest published version, when the registry reports it.
    pub version: Option<String>,
    /// Latest release source identities, independent of configured publishers.
    pub provenance_repositories: Vec<String>,
    /// Configured publishers when the public lookup is authenticated successfully.
    pub configured_publishers: Option<Vec<PublisherIdentity>>,
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
        self.get_json_status(url).await.0
    }

    async fn get_json_status(&self, url: &str) -> (Lookup, Option<u16>) {
        let crates = reqwest::Url::parse(url).is_ok_and(|url| url.host_str() == Some("crates.io"));
        for attempt in 0..4 {
            if crates {
                crate::request_policy::crates_permit().await;
            }
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
                    if matches!(status.as_u16(), 403 | 429) && attempt < 3 {
                        let delay = crate::request_policy::retry_delay(
                            response
                                .headers()
                                .get("retry-after")
                                .and_then(|value| value.to_str().ok()),
                            attempt,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    if status == reqwest::StatusCode::NOT_FOUND {
                        return (Lookup::Missing, None);
                    }
                    if status.is_success() {
                        return (
                            response
                                .bytes()
                                .await
                                .ok()
                                .and_then(|body| serde_json::from_slice(&body).ok())
                                .map_or(Lookup::Unknown, Lookup::Found),
                            Some(status.as_u16()),
                        );
                    }
                    return (Lookup::Unknown, Some(status.as_u16()));
                }
                Err(error) => {
                    if self.verbose {
                        eprintln!("GET {url} failed: {error}");
                    }
                    return (Lookup::Unknown, None);
                }
            }
        }
        unreachable!("bounded request attempts return a result")
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

    /// Call the crates.io API at `url` with `token`; status 0 when the request
    /// failed. Only the method, URL, and status are traced, never the token.
    pub async fn token_call(
        &self,
        method: &str,
        url: &str,
        token: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or_default();
        let mut request = self
            .client
            .request(method.clone(), url)
            .timeout(Duration::from_secs(30))
            .header("accept", "application/json")
            .header("authorization", token);
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_string());
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if self.verbose {
                    eprintln!("{method} {url} -> {status}");
                }
                let body = response
                    .bytes()
                    .await
                    .ok()
                    .and_then(|body| serde_json::from_slice(&body).ok())
                    .unwrap_or(Value::Null);
                (status, body)
            }
            Err(error) => {
                if self.verbose {
                    eprintln!("{method} {url} failed: {error}");
                }
                (0, json!({ "errors": [{ "detail": error.to_string() }] }))
            }
        }
    }

    /// Probe one package; unknown fields stay `None`.
    pub async fn probe_package(&self, package: &Package) -> PackageState {
        let Some(url) = self.endpoints.state_url(package) else {
            return PackageState::default();
        };
        let (lookup, registry_http_status) = self.get_json_status(&url).await;
        let document = match lookup {
            Lookup::Unknown => {
                return PackageState {
                    registry_http_status,
                    ..PackageState::default()
                }
            }
            Lookup::Missing => {
                return PackageState {
                    exists: Some(false),
                    trusted: (package.registry != Registry::DockerHub).then_some(false),
                    version: None,
                    ..PackageState::default()
                }
            }
            Lookup::Found(document) => document,
        };
        match package.registry {
            Registry::Npm => PackageState {
                exists: Some(true),
                trusted: Some(npm_trusted(&document)),
                version: document["version"].as_str().map(str::to_owned),
                provenance_repositories: if let Some(url) =
                    document["dist"]["attestations"]["url"].as_str()
                {
                    match self.get_json(url).await {
                        Lookup::Found(value) => provenance_repositories(&value),
                        _ => Vec::new(),
                    }
                } else {
                    Vec::new()
                },
                ..PackageState::default()
            },
            Registry::CratesIo => {
                let latest = document["versions"].as_array().and_then(|versions| {
                    versions
                        .iter()
                        .find(|version| version["num"] == document["crate"]["max_version"])
                        .or_else(|| versions.first())
                });
                let source = latest.map_or(&Value::Null, |version| &version["trustpub_data"]);
                PackageState {
                    exists: Some(true),
                    trusted: Some(truthy(source)),
                    provenance_repositories: publisher_identities(source)
                        .into_iter()
                        .map(|publisher| publisher.repository)
                        .collect(),
                    publisher_settings_reason: Some("publisher settings require an authenticated crates.io session; public provenance is independent".into()),
                    version: None,
                    ..PackageState::default()
                }
            }
            Registry::PyPi => {
                let provenance = self.pypi_provenance(&document).await;
                PackageState {
                    exists: Some(true),
                    trusted: Some(truthy(&provenance)),
                    provenance_repositories: provenance_repositories(&provenance),
                    ..PackageState::default()
                }
            }
            _ => PackageState {
                exists: Some(true),
                ..PackageState::default()
            },
        }
    }

    async fn pypi_provenance(&self, document: &Value) -> Value {
        let (Some(file), Some(version), Some(base)) = (
            document["urls"][0]["filename"].as_str(),
            document["info"]["version"].as_str(),
            self.endpoints.base(Registry::PyPi),
        ) else {
            return Value::Null;
        };
        let name = document["info"]["name"].as_str().unwrap_or_default();
        let url = format!(
            "{base}/integrity/{}/{}/{}/provenance",
            encode_uri_component(name),
            encode_uri_component(version),
            encode_uri_component(file)
        );
        match self.get_json(&url).await {
            Lookup::Found(value) => value,
            _ => Value::Null,
        }
    }

    /// Look up every publishable package concurrently. Unknown state is left
    /// unset so plans keep every conditional step.
    pub async fn probe_registry_state(&self, inspection: &Inspection) -> Inspection {
        self.probe_registry_state_with(inspection, true).await
    }

    /// Probe an API-discovered repository whose canonical identity is already known.
    pub async fn probe_known_repository(&self, inspection: &Inspection) -> Inspection {
        self.probe_registry_state_with(inspection, false).await
    }

    async fn probe_registry_state_with(
        &self,
        inspection: &Inspection,
        resolve: bool,
    ) -> Inspection {
        let mut probed = inspection.clone();
        if resolve {
            resolve_repository(&mut probed, self.verbose).await;
        }
        inspect_manifest_repositories(&mut probed);
        let mut tasks = tokio::task::JoinSet::new();
        for (index, package) in probed.packages.iter().enumerate() {
            if package.publishable {
                let client = self.clone();
                let package = package.clone();
                tasks.spawn(async move { (index, client.probe_package(&package).await) });
            }
        }
        while let Some(result) = tasks.join_next().await {
            let Ok((index, state)) = result else {
                continue;
            };
            let package = &mut probed.packages[index];
            package.registry_http_status = state.registry_http_status;
            package.publisher_settings_reason = state.publisher_settings_reason;
            if state.exists.is_some() {
                package.exists_on_registry = state.exists;
            }
            if state.trusted.is_some() {
                package.trusted_publishing = state.trusted;
            }
            package.provenance_repositories = state.provenance_repositories;
            package.configured_publishers = state.configured_publishers;
            if package.registry == Registry::Npm && state.exists == Some(true) {
                if let Some(output) = identity_command(
                    "npm",
                    &["trust", "list", &package.name, "--browser=false"],
                    std::path::Path::new(&inspection.repository.root),
                    self.verbose,
                )
                .await
                {
                    package.configured_publishers = Some(npm_publishers(&output));
                }
            }
            let urls = package.manifest_urls.clone();
            compare_repositories(package, &probed.repository, &urls);
            if state.trusted == Some(true)
                && matches!(
                    package.registry,
                    Registry::Npm | Registry::CratesIo | Registry::PyPi
                )
            {
                let expected = TrustedPublisherPrefill {
                    provider: "github-actions".into(),
                    organization: probed.repository.github_owner.clone().unwrap_or_default(),
                    repository: probed
                        .repository
                        .github_repository
                        .clone()
                        .unwrap_or_default(),
                    workflow: package.workflow.clone().unwrap_or_default(),
                    environment: package.environment.clone(),
                    project: None,
                };
                let verified = package
                    .configured_publishers
                    .as_ref()
                    .is_some_and(|publishers| {
                        publishers
                            .iter()
                            .any(|publisher| publisher_matches(publisher, &expected))
                    });
                package.publisher_settings_verified = Some(verified);
                if !verified {
                    package.warnings.push("configured trusted publisher could not be verified; setup must check the repository, workflow and environment in registry settings".into());
                }
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
