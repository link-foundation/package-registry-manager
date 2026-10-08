use crate::mock_registry::MockRegistry;
use package_registry_manager::registry_state::{Endpoints, RegistryClient};
use package_registry_manager::{Package, Registry};

#[tokio::test]
async fn crates_probes_latest_provenance_and_configured_publishers_independently() {
    let registry = MockRegistry::start(|path| {
        if path.starts_with("/trusted_publishing/github_configs?") {
            Some(r#"{"github_configs":[{"id":1,"repository_owner":"konard","repository_name":"disk-space-saviour","workflow_filename":"release.yml"}],"meta":{"next_page":null}}"#.into())
        } else {
            Some(r#"{"crate":{"max_version":"2"},"versions":[{"num":"2","trustpub_data":null},{"num":"1","trustpub_data":{"repository":"konard/disk-space-saviour"}}]}"#.into())
        }
    });
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_CRATES_IO_API", &registry.base),
        false,
    );
    let package = Package::new(Registry::CratesIo, "tool".into(), None, "Cargo.toml".into());
    let state = client.probe_package(&package).await;
    assert_eq!(state.trusted, Some(false));
    assert_eq!(state.provenance_repositories, [] as [String; 0]);
    assert_eq!(
        state.configured_publishers.unwrap()[0].repository,
        "konard/disk-space-saviour"
    );
}
