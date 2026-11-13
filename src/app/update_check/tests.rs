// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::json;
use std::io::{Read, Write};

pub(super) fn fixture(auto_install: Option<bool>) -> (tempfile::TempDir, App) {
    let directory = tempfile::tempdir().expect("fixture directory");
    let path = directory.path().join("settings.json");
    let mut document =
        json!({"updates": {"future": 42}, "notifications": {"actionsRequired": false}});
    if let Some(value) = auto_install {
        document["updates"]["autoInstall"] = json!(value);
    }
    std::fs::write(&path, document.to_string()).expect("initial app settings");
    let mut app = App::test_default();
    app.global_settings = settings::load_from_path(&path).expect("load settings");
    app.global_settings_path = Some(path);
    app.input.set_text("preserve my draft");
    (directory, app)
}

fn release_server(status: &str) -> (String, std::thread::JoinHandle<String>) {
    release_server_body(
        status,
        r#"{"tag_name":"v99.0.0","html_url":"https://example.invalid/v99.0.0"}"#,
    )
}

pub(super) fn release_server_body(
    status: &str,
    body: &str,
) -> (String, std::thread::JoinHandle<String>) {
    let body = body.to_owned();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("local HTTP listener");
    listener.set_nonblocking(true).expect("bounded server accept");
    let endpoint = format!("http://{}/releases/latest", listener.local_addr().expect("address"));
    let status = status.to_owned();
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "release request never arrived");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("release request failed: {error}"),
            }
        };
        // macOS and Windows hand the listener's non-blocking mode to accepted sockets.
        socket.set_nonblocking(false).expect("blocking request read");
        socket.set_read_timeout(Some(Duration::from_secs(5))).expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0; 2048];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).expect("HTTP request");
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("release response");
        String::from_utf8(request).expect("request text")
    });
    (endpoint, worker)
}

#[tokio::test(flavor = "current_thread")]
async fn failed_release_request_keeps_the_existing_cache() {
    let (endpoint, server) = release_server("503 Service Unavailable");
    let (_directory, mut app) = fixture(Some(true));
    settings::record_update_check_result(
        &mut app.global_settings,
        env!("CARGO_PKG_VERSION"),
        "98.0.0",
        "https://example.invalid/v98.0.0",
        unix_now_secs().expect("clock") - UPDATE_CHECK_TTL_SECS - 1,
    );
    let path = app.global_settings_path.as_deref().expect("settings path");
    settings::save_global_settings(path, &app.global_settings).expect("seed expired cache");
    let original = std::fs::read(path).expect("saved cache");
    let cached = app.global_settings.updates.last_result.clone();
    let tasks = tokio::task::LocalSet::new();
    tasks.run_until(async { start_update_check_at(&app, false, &endpoint) }).await;
    tasks.await;
    assert!(server.join().expect("server").starts_with("GET /releases/latest HTTP/1.1"));
    assert!(matches!(app.event_rx.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
    assert_eq!(app.global_settings.updates.last_result, cached);
    assert_eq!(std::fs::read(path).expect("preserved settings"), original);
    assert_eq!(app.input.text(), "preserve my draft");
}
