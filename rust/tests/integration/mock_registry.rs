//! A tiny HTTP registry for tests: answers every request with the JSON the
//! handler returns, or 404, so tests never touch the real registries.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

type Handler = dyn Fn(&str) -> Option<String> + Send + Sync;

pub struct MockRegistry {
    pub base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl MockRegistry {
    pub fn start(handler: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock registry");
        let base = format!("http://{}", listener.local_addr().expect("mock address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let handler = Arc::clone(&handler);
                let recorded = Arc::clone(&recorded);
                thread::spawn(move || {
                    let mut reader = BufReader::new(&stream);
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        return;
                    }
                    let mut header = String::new();
                    while reader.read_line(&mut header).is_ok_and(|read| read > 2) {
                        header.clear();
                    }
                    let path = request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_owned();
                    let body = handler(&path);
                    recorded.lock().expect("requests lock").push(path);
                    let (status, body) = body.map_or_else(
                        || ("404 Not Found", "{}".to_owned()),
                        |body| ("200 OK", body),
                    );
                    let _ = write!(
                        &stream,
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        Self { base, requests }
    }

    /// Environment variables that point every registry lookup at this server.
    pub fn env(&self) -> Vec<(String, String)> {
        [
            ("NPM_REGISTRY", "npm"),
            ("CRATES_IO_API", "crates"),
            ("PYPI_API", "pypi"),
            ("DOCKER_HUB_API", "docker"),
        ]
        .into_iter()
        .map(|(variable, prefix)| {
            (
                format!("PACKAGE_REGISTRY_MANAGER_{variable}"),
                format!("{}/{prefix}", self.base),
            )
        })
        .collect()
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests lock").clone()
    }
}
