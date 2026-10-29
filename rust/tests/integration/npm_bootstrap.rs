use package_registry_manager::registry_state::{Endpoints, PackageState, RegistryClient};
use package_registry_manager::{Package, Registry};

use crate::mock_registry::MockRegistry;

#[tokio::test]
async fn probes_npm_existence_and_trusted_publishing() {
    let registry = MockRegistry::start(|path| match path {
        "/@acme%2Fwidgets/latest" => {
            Some(r#"{"version":"1.0.0","_npmUser":{"trustedPublisher":{"id":"github"}}}"#.into())
        }
        "/manual/latest" => Some(r#"{"version":"2.0.0"}"#.into()),
        _ => None,
    });
    let endpoints =
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY", &registry.base);
    let client = RegistryClient::new(endpoints, false);
    let probe = |name: &'static str| {
        let client = client.clone();
        async move {
            client
                .probe_package(&Package::new(
                    Registry::Npm,
                    name.to_owned(),
                    None,
                    "package.json".to_owned(),
                ))
                .await
        }
    };
    let state = |exists, trusted, version: Option<&str>| PackageState {
        exists: Some(exists),
        trusted: Some(trusted),
        version: version.map(str::to_owned),
        ..PackageState::default()
    };
    assert_eq!(probe("missing").await, state(false, false, None));
    for (name, trusted, version) in [("@acme/widgets", true, "1.0.0"), ("manual", false, "2.0.0")] {
        let actual = probe(name).await;
        assert_eq!(actual.audit["latest_version"], version);
        assert_eq!(actual.audit["version_state"], "manifest-version-unresolved");
        assert_eq!(
            actual.audit["registry_evidence_url"],
            format!("{}/{}/latest", registry.base, name.replace('/', "%2F"))
        );
        assert_eq!(
            PackageState {
                audit: std::collections::BTreeMap::default(),
                ..actual
            },
            state(true, trusted, Some(version))
        );
    }

    let offline = RegistryClient::new(
        Endpoints::default().with(
            "PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY",
            "http://127.0.0.1:9",
        ),
        false,
    );
    assert_eq!(
        offline
            .probe_package(&Package::new(
                Registry::Npm,
                "down".to_owned(),
                None,
                "package.json".to_owned()
            ))
            .await,
        PackageState::default()
    );
}
