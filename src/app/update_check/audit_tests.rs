// SPDX-License-Identifier: Apache-2.0

use super::tests::{fixture, release_server_body};
use super::*;
use clap::Parser;
use serde_json::json;

const NOW: u64 = 1_000_000;
const URL: &str = "https://example.invalid/independent-release";

async fn check(app: &mut App, body: &str) -> usize {
    let (endpoint, server) = release_server_body("200 OK", body);
    let tasks = tokio::task::LocalSet::new();
    tasks
        .run_until(async { start_update_check_with_clock(app, false, &endpoint, || Some(NOW)) })
        .await;
    tokio::time::timeout(Duration::from_secs(12), tasks).await.unwrap();
    let request = server.join().unwrap();
    assert!(request.starts_with("GET /releases/latest HTTP/1.1"));
    let request = request.to_ascii_lowercase();
    assert!(request.contains("x-github-api-version: 2022-11-28"));
    assert!(request.contains("accept: application/vnd.github+json"));
    let mut count = 0;
    while let Ok(event) = app.event_rx.try_recv() {
        count += 1;
        crate::app::events::handle_client_event(app, event);
    }
    count
}

fn assert_cache(app: &App, url: &str) {
    let saved = settings::load_from_path(app.global_settings_path.as_deref().unwrap()).unwrap();
    for settings in [&app.global_settings, &saved] {
        let result = settings.updates.last_result.as_ref().unwrap();
        assert_eq!(result.current_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(result.latest_version, "99.0.0");
        assert_eq!(result.release_url, url);
        assert_eq!(result.checked_at_unix_secs, NOW);
    }
    assert_eq!(app.input.text(), "preserve my draft");
    assert_eq!(app.surface_mode, super::super::SurfaceMode::Chat);
    assert!(!app.shutdown_requested());
}

#[tokio::test(flavor = "current_thread")]
async fn http_results_reach_saved_metadata_and_the_exact_manual_or_exit_action() {
    for enabled in [None, Some(false), Some(true)] {
        let (_dir, mut app) = fixture(enabled);
        check(&mut app, &json!({"tag_name":"v99.0.0","html_url":URL}).to_string()).await;
        assert_cache(&app, URL);
        let saved = settings::load_from_path(app.global_settings_path.as_deref().unwrap()).unwrap();
        assert_eq!(
            settings::update_prompt_candidate(&saved, env!("CARGO_PKG_VERSION"), NOW),
            (enabled != Some(true)).then(|| settings::UpdatePrompt {
                current_version: env!("CARGO_PKG_VERSION").into(),
                latest_version: "99.0.0".into(),
                release_url: URL.into(),
                last_error: None
            })
        );
        app.request_shutdown();
        assert_eq!(
            automatic_update_action_with(&app, &Cli::parse_from(["claude-rs"]), || {
                InstallMethod::Npm
            }),
            (enabled == Some(true)).then(|| super::super::PostExitAction::InstallUpdate {
                latest_version: "99.0.0".into(),
                method: InstallMethod::Npm
            })
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn queued_check_preserves_revocation_and_exit_reloads_each_supported_consent_state() {
    let (_dir, mut app) = fixture(Some(true));
    let path = app.global_settings_path.clone().unwrap();
    let (endpoint, server) = release_server_body("200 OK", r#"{"tag_name":"v99.0.0"}"#);
    let tasks = tokio::task::LocalSet::new();
    tasks
        .run_until(async { start_update_check_with_clock(&app, false, &endpoint, || Some(NOW)) })
        .await;
    tokio::time::timeout(Duration::from_secs(12), tasks).await.unwrap();
    server.join().unwrap();
    std::fs::write(&path, r#"{"updates":{"autoInstall":false,"future":42},"notifications":{"actionsRequired":false}}"#).unwrap();
    let event = app.event_rx.try_recv().unwrap();
    crate::app::events::handle_client_event(&mut app, event);
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["updates"]["autoInstall"], false);
    assert_eq!(saved["updates"]["future"], 42);
    assert_eq!(saved["notifications"]["actionsRequired"], false);
    app.request_shutdown();
    assert!(
        automatic_update_action_with(&app, &Cli::parse_from(["claude-rs"]), || panic!(
            "revocation must precede detection"
        ))
        .is_none()
    );

    for case in [
        "enabled",
        "reset",
        "deleted",
        "malformed",
        "absent-result",
        "current",
        "disabled",
        "forced",
        "unknown",
        "no-path",
    ] {
        let (_dir, mut app) = fixture(Some(false));
        let path = app.global_settings_path.clone().unwrap();
        let mut document = json!({"updates":{"autoInstall":true,"last_result":{"checked_at_unix_secs":NOW,"current_version":env!("CARGO_PKG_VERSION"),"latest_version":"99.0.0","release_url":URL,"source":"github_release"}}});
        match case {
            "reset" => {
                document["updates"].as_object_mut().unwrap().remove("autoInstall");
            }
            "absent-result" => {
                document["updates"].as_object_mut().unwrap().remove("last_result");
            }
            "current" => {
                document["updates"]["last_result"]["latest_version"] =
                    json!(env!("CARGO_PKG_VERSION"));
            }
            _ => {}
        }
        std::fs::write(&path, document.to_string()).unwrap();
        if case == "deleted" {
            std::fs::remove_file(&path).unwrap();
        }
        if case == "malformed" {
            std::fs::write(&path, "{broken").unwrap();
        }
        if case == "no-path" {
            app.global_settings_path = None;
        }
        if case == "forced" {
            app.force_shutdown();
        } else {
            app.request_shutdown();
        }
        let before = std::fs::read(&path).ok();
        let cli = if case == "disabled" {
            Cli::parse_from(["claude-rs", "--no-update-check"])
        } else {
            Cli::parse_from(["claude-rs"])
        };
        let called = std::cell::Cell::new(false);
        let action = automatic_update_action_with(&app, &cli, || {
            called.set(true);
            if case == "unknown" { InstallMethod::Unknown } else { InstallMethod::Npm }
        });
        assert_eq!(called.get(), matches!(case, "enabled" | "unknown"), "{case}");
        assert_eq!(
            action,
            (case == "enabled").then(|| super::super::PostExitAction::InstallUpdate {
                latest_version: "99.0.0".into(),
                method: InstallMethod::Npm
            }),
            "{case}"
        );
        assert_eq!(std::fs::read(&path).ok(), before, "exit selection never writes: {case}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cache_boundary_invalid_release_and_explicit_disable_control_real_requests() {
    for enabled in [None, Some(false), Some(true)] {
        for (age, version, disabled, refresh) in [
            (86_400, "98.0.0", false, false),
            (86_401, "98.0.0", false, true),
            (0, "invalid", false, true),
            (86_401, "98.0.0", true, false),
        ] {
            let (_dir, mut app) = fixture(enabled);
            settings::record_update_check_result(
                &mut app.global_settings,
                env!("CARGO_PKG_VERSION"),
                version,
                "old-url",
                NOW - age,
            );
            let path = app.global_settings_path.clone().unwrap();
            settings::save_global_settings(&path, &app.global_settings).unwrap();
            let before = std::fs::read(&path).unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut endpoint = format!("http://{}/releases/latest", listener.local_addr().unwrap());
            let server = if refresh {
                let (url, thread) = release_server_body("200 OK", r#"{"tag_name":"v99.0.0"}"#);
                endpoint = url;
                Some(thread)
            } else {
                None
            };
            let tasks = tokio::task::LocalSet::new();
            let disabled = if disabled { update_check_disabled(true) } else { false };
            tasks
                .run_until(async {
                    start_update_check_with_clock(&app, disabled, &endpoint, || Some(NOW));
                })
                .await;
            tokio::time::timeout(Duration::from_secs(12), tasks).await.unwrap();
            if let Some(server) = server {
                assert!(server.join().unwrap().starts_with("GET /releases/latest HTTP/1.1"));
                let event = app.event_rx.try_recv().unwrap();
                crate::app::events::handle_client_event(&mut app, event);
                assert_cache(
                    &app,
                    "https://github.com/srothgan/claude-code-rust/releases/tag/v99.0.0",
                );
            } else {
                assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
                assert!(matches!(
                    app.event_rx.try_recv(),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                ));
                assert_eq!(std::fs::read(&path).unwrap(), before);
            }
        }
    }
}

#[test]
fn numeric_release_policy_normalizes_supported_spelling_and_compares_each_component() {
    for (input, normalized) in [
        ("v1.2.3", Some("1.2.3")),
        ("v2.4.6-rc1", Some("2.4.6")),
        ("1.2", None),
        ("1.2.3.4", None),
        ("v1.two.3", None),
    ] {
        assert_eq!(normalize_version_string(input).as_deref(), normalized);
        if normalized.is_none() {
            assert!(!is_newer_version(input, "1.0.0"));
            assert!(!is_newer_version("9.0.0", input));
        }
    }
    for (candidate, current, newer) in [
        ("2.0.0", "1.99.99", true),
        ("1.3.0", "1.2.99", true),
        ("1.2.10", "1.2.9", true),
        ("1.2.9", "1.2.10", false),
        ("1.99.99", "2.0.0", false),
        ("1.2.99", "1.3.0", false),
        ("v2.4.6-rc1", "2.4.6", false),
        ("v2.4.7-rc1", "2.4.6", true),
        ("1.2.3", "1.2.3", false),
    ] {
        assert_eq!(is_newer_version(candidate, current), newer, "{candidate} vs {current}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn successful_http_status_still_requires_a_usable_release_and_recovers_after_rejection() {
    for body in ["{broken", "{}", r#"{"tag_name":42}"#, r#"{"tag_name":"1.bad.3"}"#] {
        let (_dir, mut app) = fixture(Some(true));
        settings::record_update_check_result(
            &mut app.global_settings,
            env!("CARGO_PKG_VERSION"),
            "98.0.0",
            "old-url",
            1,
        );
        let path = app.global_settings_path.clone().unwrap();
        settings::save_global_settings(&path, &app.global_settings).unwrap();
        let before = std::fs::read(&path).unwrap();
        let cache = app.global_settings.updates.last_result.clone();
        assert_eq!(check(&mut app, body).await, 0, "rejected responses emit no completion");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(app.global_settings.updates.last_result, cache);
        assert_eq!(app.input.text(), "preserve my draft");
        check(&mut app, r#"{"tag_name":"v99.0.0"}"#).await;
        assert_cache(&app, "https://github.com/srothgan/claude-code-rust/releases/tag/v99.0.0");
    }
    for value in [None, Some(json!(null)), Some(json!("  ")), Some(json!(URL))] {
        let (_dir, mut app) = fixture(None);
        let mut body = json!({"tag_name":"v99.0.0"});
        if let Some(value) = &value {
            body["html_url"] = value.clone();
        }
        check(&mut app, &body.to_string()).await;
        assert_cache(
            &app,
            if value == Some(json!(URL)) {
                URL
            } else {
                "https://github.com/srothgan/claude-code-rust/releases/tag/v99.0.0"
            },
        );
    }
}
