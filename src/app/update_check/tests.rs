// SPDX-License-Identifier: Apache-2.0

use super::*;
use clap::Parser;
use serde_json::json;
use std::io::{Read, Write};

fn fixture(auto_install: Option<bool>) -> (tempfile::TempDir, App) {
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
        let body = r#"{"tag_name":"v99.0.0","html_url":"https://example.invalid/v99.0.0"}"#;
        write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("release response");
        String::from_utf8(request).expect("request text")
    });
    (endpoint, worker)
}

#[tokio::test(flavor = "current_thread")]
async fn checks_in_both_modes_persist_results_and_choose_manual_or_exit_installation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for enabled in [None, Some(false), Some(true)] {
                let (_directory, mut app) = fixture(enabled);
                let (endpoint, server) = release_server("200 OK");
                start_update_check_at(&app, false, &endpoint);
                let event = tokio::time::timeout(Duration::from_secs(10), app.event_rx.recv())
                    .await
                    .expect("check timeout")
                    .expect("check event");
                crate::app::events::handle_client_event(&mut app, event);
                let request = server.join().expect("HTTP worker");
                assert!(request.starts_with("GET /releases/latest HTTP/1.1"));
                assert!(request.to_ascii_lowercase().contains("x-github-api-version: 2022-11-28"));
                let saved =
                    settings::load_from_path(app.global_settings_path.as_deref().expect("path"))
                        .expect("saved cache");
                assert_eq!(
                    saved.updates.last_result.as_ref().expect("result").latest_version,
                    "99.0.0"
                );
                assert_eq!(app.global_settings.updates.last_result, saved.updates.last_result);
                assert_eq!(app.input.text(), "preserve my draft");
                assert_eq!(app.surface_mode, super::super::SurfaceMode::Chat);
                assert!(!app.shutdown_requested());
                let candidate = settings::update_prompt_candidate(
                    &saved,
                    env!("CARGO_PKG_VERSION"),
                    unix_now_secs().expect("clock"),
                );
                assert_eq!(candidate.is_some(), enabled != Some(true));
                app.request_shutdown();
                let cli = Cli::parse_from(["claude-rs"]);
                let action = automatic_update_action_with(&app, &cli, || InstallMethod::Npm);
                assert_eq!(
                    action,
                    enabled.filter(|value| *value).map(|_| {
                        super::super::PostExitAction::InstallUpdate {
                            latest_version: "99.0.0".into(),
                            method: InstallMethod::Npm,
                        }
                    })
                );
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn check_results_preserve_revocation_and_reset_before_exit() {
    tokio::task::LocalSet::new().run_until(async {
        let (_directory, mut app) = fixture(Some(true));
        let path = app.global_settings_path.clone().expect("settings path");
        let (endpoint, server) = release_server("200 OK");
        start_update_check_at(&app, false, &endpoint);
        std::fs::write(&path, r#"{"updates":{"autoInstall":false,"future":42},"notifications":{"actionsRequired":false}}"#).expect("config save revokes automatic installation");
        let event = tokio::time::timeout(Duration::from_secs(10), app.event_rx.recv()).await.expect("check timeout").expect("event");
        crate::app::events::handle_client_event(&mut app, event);
        server.join().expect("server");
        app.request_shutdown();
        let cli = Cli::parse_from(["claude-rs"]);
        assert!(automatic_update_action_with(&app, &cli, || InstallMethod::Npm).is_none());
        let mut saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
        assert_eq!(saved["updates"]["autoInstall"], false);
        assert_eq!(saved["updates"]["future"], 42);
        assert_eq!(saved["notifications"]["actionsRequired"], false);
        saved["updates"]["autoInstall"] = json!(true);
        std::fs::write(&path, saved.to_string()).expect("enable");
        assert!(automatic_update_action_with(&app, &cli, || InstallMethod::Npm).is_some());
        assert!(automatic_update_action_with(&app, &cli, || InstallMethod::Unknown).is_none());
        assert!(automatic_update_action_with(&app, &Cli::parse_from(["claude-rs", "--no-update-check"]), || InstallMethod::Npm).is_none());
        app.force_shutdown();
        assert!(automatic_update_action_with(&app, &cli, || InstallMethod::Npm).is_none());
        app.shutdown = super::super::ShutdownState::Requested;
        saved["updates"].as_object_mut().expect("updates").remove("autoInstall");
        std::fs::write(&path, saved.to_string()).expect("reset");
        assert!(automatic_update_action_with(&app, &cli, || InstallMethod::Npm).is_none());
        std::fs::write(&path, "{broken").expect("invalid external edit");
        assert!(automatic_update_action_with(&app, &cli, || InstallMethod::Npm).is_none());
        assert_eq!(std::fs::read_to_string(path).expect("read"), "{broken");
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn fresh_cache_prevents_a_release_request_in_both_update_modes() {
    for enabled in [None, Some(false), Some(true)] {
        let (_directory, mut app) = fixture(enabled);
        settings::record_update_check_result(
            &mut app.global_settings,
            env!("CARGO_PKG_VERSION"),
            "99.0.0",
            "https://example.invalid/v99.0.0",
            unix_now_secs().expect("clock"),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reachable endpoint");
        listener.set_nonblocking(true).expect("observe requests without blocking");
        let endpoint =
            format!("http://{}/releases/latest", listener.local_addr().expect("address"));
        let tasks = tokio::task::LocalSet::new();
        tasks.run_until(async { start_update_check_at(&app, false, &endpoint) }).await;
        // Drain the actual background check before inspecting its network and event effects.
        tasks.await;
        assert_eq!(
            listener.accept().expect_err("no network request").kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(matches!(
            app.event_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
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
#[test]
fn parse_simple_version_accepts_v_prefix() {
    assert_eq!(
        parse_simple_version("v1.2.3"),
        Some(SimpleVersion { major: 1, minor: 2, patch: 3 })
    );
}

#[test]
fn parse_simple_version_rejects_invalid_shapes() {
    assert_eq!(parse_simple_version("1.2"), None);
    assert_eq!(parse_simple_version("1.2.3.4"), None);
    assert_eq!(parse_simple_version("v1.two.3"), None);
}

#[test]
fn parse_simple_version_ignores_prerelease_suffix() {
    assert_eq!(
        parse_simple_version("v2.4.6-rc1"),
        Some(SimpleVersion { major: 2, minor: 4, patch: 6 })
    );
}

#[test]
fn normalize_version_string_accepts_release_tag() {
    assert_eq!(normalize_version_string("v0.10.0").as_deref(), Some("0.10.0"));
}

#[test]
fn github_release_payload_parses_tag_name() {
    let payload = r#"{"tag_name":"v0.11.0"}"#;
    let parsed = serde_json::from_str::<GithubLatestRelease>(payload).ok();
    assert_eq!(parsed.map(|r| r.tag_name), Some("v0.11.0".to_owned()));
}

#[test]
fn update_check_disabled_prefers_flag() {
    assert!(update_check_disabled(true));
}

#[test]
fn is_newer_version_compares_semver_triplets() {
    assert!(is_newer_version("0.3.0", "0.2.9"));
    assert!(!is_newer_version("0.2.9", "0.3.0"));
    assert!(!is_newer_version("bad", "0.3.0"));
}
