use super::mock_registry::MockRegistry;
use package_registry_manager::{
    model::Package,
    registry_state::{Endpoints, RegistryClient},
    Registry,
};

#[tokio::test]
async fn public_crate_scan_does_not_query_private_publisher_settings() {
    let server = MockRegistry::start(|_| {
        Some(r#"{"crate":{"max_version":"1.0.0"},"versions":[{"num":"1.0.0","trustpub_data":{"repository_owner":"team","repository_name":"project"}}]}"#.into())
    });
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_CRATES_IO_API", &server.base),
        false,
    );
    let state = client
        .probe_package(&Package::new(
            Registry::CratesIo,
            "tool".into(),
            None,
            "Cargo.toml".into(),
        ))
        .await;
    assert_eq!(state.exists, Some(true));
    assert_eq!(state.trusted, Some(true));
    assert_eq!(
        server.requests().len(),
        1,
        "owner-only settings must be checked in an authenticated setup session"
    );
}

#[tokio::test]
async fn retry_after_is_respected_and_authorization_failure_stays_unknown() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for status in [
            "429 Too Many Requests",
            "403 Forbidden",
            "405 Method Not Allowed",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = [0; 8192];
            let received = stream.read(&mut request).unwrap();
            assert!(received > 0);
            write!(stream, "HTTP/1.1 {status}\r\nRetry-After: 2\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
        }
    });
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_CRATES_IO_API", &base),
        false,
    );
    let start = Instant::now();
    let state = client
        .probe_package(&Package::new(
            Registry::CratesIo,
            "tool".into(),
            None,
            "Cargo.toml".into(),
        ))
        .await;
    assert!(start.elapsed() >= Duration::from_secs(4));
    assert_eq!(state.exists, None);
    assert_eq!(state.registry_http_status, Some(405));
    server.join().unwrap();
}

#[tokio::test]
async fn concurrent_crates_clients_share_one_request_start_queue() {
    use package_registry_manager::request_policy::crates_permit;
    use std::time::{Duration, Instant};
    let start = Instant::now();
    tokio::join!(crates_permit(), crates_permit(), crates_permit());
    assert!(start.elapsed() >= Duration::from_secs(2));
}

#[tokio::test]
async fn retries_incomplete_registry_body_and_preserves_exact_404() {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for (path, response) in [
            ("/meta-language/latest", "HTTP/1.1 200 OK\r\nContent-Length: 99\r\nConnection: close\r\n\r\n{"),
            ("/meta-language/latest", "HTTP/1.1 200 OK\r\nContent-Length: 19\r\nConnection: close\r\n\r\n{\"version\":\"1.0.0\"}"),
            ("/meta-language/2.0.0", "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            assert!(String::from_utf8_lossy(&request).starts_with(&format!("GET {path} ")));
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY", &base),
        false,
    );
    let package = Package::new(
        Registry::Npm,
        "meta-language".into(),
        Some("2.0.0".into()),
        "package.json".into(),
    );
    let state = tokio::time::timeout(Duration::from_secs(5), client.probe_package(&package))
        .await
        .expect("lookup recovers within its test budget");
    assert_eq!(state.exists, Some(true));
    assert_eq!(state.audit["latest_version"], "1.0.0");
    assert_eq!(state.audit["manifest_version_exists"], false);
    assert_eq!(state.audit["version_state"], "manifest-version-absent");
    tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn exact_version_404_is_absent_when_the_latest_tag_is_missing() {
    let server = MockRegistry::start(|_| None);
    let client = RegistryClient::new(
        Endpoints::default().with("PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY", &server.base),
        false,
    );
    let package = Package::new(
        Registry::Npm,
        "missing".into(),
        Some("2.0.0".into()),
        "package.json".into(),
    );
    let state = client.probe_package(&package).await;
    assert_eq!(server.requests(), ["/missing/latest", "/missing/2.0.0"]);
    assert_eq!(state.exists, Some(false));
    assert_eq!(state.audit["manifest_version_exists"], false);
    assert_eq!(state.audit["version_state"], "manifest-version-absent");
}
