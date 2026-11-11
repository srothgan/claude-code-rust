// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::{FullscreenView, SurfaceMode};

fn connected_view_app() -> (tempfile::TempDir, App, crate::agent::client::CommandReceiver) {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, commands) = crate::app::test_support::app_with_connection();
    app.status = crate::app::AppStatus::Ready;
    app.settings_home_override = Some(directory.path().join("home"));
    app.cwd_raw = directory.path().to_string_lossy().into_owned();
    app.cwd = app.cwd_raw.clone();
    (directory, app, commands)
}

fn routed_key(app: &mut App, code: crossterm::event::KeyCode) {
    crate::app::test_support::press(app, code);
}

fn interrupt_view(app: &mut App) {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    crate::app::handle_terminal_event(
        app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
}

fn open_populated_diff(app: &mut App, directory: &std::path::Path) {
    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(directory)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    std::fs::write(directory.join("view-cedar.txt"), "VIEW_CEDAR_CONTENT\n").unwrap();
    crate::app::diff_view::open(app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        crate::app::diff_view::poll(app);
        let view = app.diff_view.as_ref().unwrap();
        if view.loading.is_none() && view.content.is_some() {
            assert!(
                view.snapshot
                    .as_ref()
                    .unwrap()
                    .files
                    .iter()
                    .any(|file| file.path == "view-cedar.txt")
            );
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "diff fixture did not load: {:?}",
            view.error
        );
        std::thread::yield_now();
    }
}

#[test]
fn view_transition_preserves_a_real_completion_draft_and_its_attachment() {
    use crossterm::event::{Event, KeyCode};
    let (_directory, mut app, mut commands) = connected_view_app();
    let image = crate::app::test_support::session_image();
    app.input.set_text("/he [Image #1] keep 界");
    app.input.image_payloads.push(image.clone());
    app.input.set_cursor(0, 3);
    crate::app::slash::sync_with_cursor(&mut app);
    assert!(app.slash.visible().is_some_and(|state| !state.candidates.is_empty()));
    crate::app::config::open_tab(&mut app, crate::app::ConfigTab::Help).unwrap();
    routed_key(&mut app, KeyCode::Esc);
    assert_eq!(app.input.text(), "/he [Image #1] keep 界");
    assert_eq!(app.input.cursor_col(), 3);
    routed_key(&mut app, KeyCode::Home);
    crate::app::handle_terminal_event(&mut app, Event::Paste("continue ".into()));
    crate::app::finalize_pending_paste_event(&mut app);
    assert_eq!(app.input.text(), "continue /he [Image #1] keep 界");
    assert!(commands.try_recv().is_err());
    routed_key(&mut app, KeyCode::Enter);
    crate::app::test_support::expect_session_prompt(
        &mut commands,
        "session-1",
        "continue /he [Image #1] keep 界",
        &[image],
        &[],
    );
}

#[test]
fn view_transition_keeps_accepted_paste_before_and_after_its_normal_drain() {
    use crossterm::event::{Event, KeyCode};
    for drain_before in [false, true] {
        let (_directory, mut app, mut commands) = connected_view_app();
        let source = "雪_line\n".repeat(180);
        app.input.set_text("before ");
        crate::app::handle_terminal_event(&mut app, Event::Paste(source.clone()));
        if drain_before {
            crate::app::finalize_pending_paste_event(&mut app);
        }
        crate::app::config::open_tab(&mut app, crate::app::ConfigTab::Help).unwrap();
        routed_key(&mut app, KeyCode::Esc);
        crate::app::finalize_pending_paste_event(&mut app);
        assert_eq!(app.input.text(), format!("before {source}"));
        assert_eq!(app.input.inline_pastes().as_slice(), std::slice::from_ref(&source));
        assert!(commands.try_recv().is_err());
        routed_key(&mut app, KeyCode::Enter);
        crate::app::test_support::expect_session_prompt(
            &mut commands,
            "session-1",
            &format!("before {source}"),
            &[],
            &[source],
        );
    }
}

#[test]
fn view_transition_cancels_the_armed_submit_until_the_next_explicit_enter() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (_directory, mut app, mut commands) = connected_view_app();
    app.input.set_text("deferred cedar 界");
    crate::app::handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    );
    assert!(app.pending_submit.is_some());
    crate::app::config::open_tab(&mut app, crate::app::ConfigTab::Help).unwrap();
    routed_key(&mut app, KeyCode::Esc);
    crate::app::finalize_deferred_submit(&mut app);
    assert!(commands.try_recv().is_err());
    assert_eq!(app.input.text(), "deferred cedar 界");
    routed_key(&mut app, KeyCode::Enter);
    crate::app::test_support::expect_session_prompt(
        &mut commands,
        "session-1",
        "deferred cedar 界",
        &[],
        &[],
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_real_form_retargets_a_populated_diff_and_returns_to_the_preserved_draft() {
    use crate::agent::{events::ClientEvent, types, wire::BridgeCommand};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    tokio::task::LocalSet::new().run_until(Box::pin(async {
        let (directory, mut app, mut commands) = connected_view_app();
        app.input.set_text("retarget draft 界");
        open_populated_diff(&mut app, directory.path());
        crate::app::handle_client_event(&mut app, ClientEvent::McpElicitationRequest {
            session_id: "session-1".into(),
            request: types::ElicitationRequest { request_id: "view-form".into(), server_name: "cedar".into(), message: "Choose cedar answer".into(), mode: types::ElicitationMode::Form, url: None, elicitation_id: None, field_order: vec!["answer".into()], requested_schema: Some(serde_json::json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]})) },
        });
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
        assert!(crate::app::config::handle_paste(&mut app, "CEDAR_ANSWER"));
        crate::app::handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)));
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), commands.recv_envelope()).await.unwrap().unwrap();
        assert!(matches!(envelope.command, BridgeCommand::ElicitationResponse { elicitation_request_id, action: types::ElicitationAction::Accept, content: Some(content), .. } if elicitation_request_id == "view-form" && content == serde_json::json!({"answer":"CEDAR_ANSWER"})));
        crate::app::handle_client_event(&mut app, ClientEvent::McpElicitationSettled { session_id:"session-1".into(), request_id:"view-form".into(), session_closing:false });
        routed_key(&mut app, KeyCode::Esc);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert_eq!(app.input.text(), "retarget draft 界");
        assert!(app.diff_view.is_none());
        crate::app::test_support::drain_session_refreshes(&mut commands, "session-1");
        routed_key(&mut app, KeyCode::Enter);
        crate::app::test_support::expect_session_prompt(&mut commands, "session-1", "retarget draft 界", &[], &[]);
    })).await;
}

#[test]
fn repeated_chat_surface_preserves_the_real_selected_completion_and_suffix() {
    use crossterm::event::KeyCode;
    let (_directory, mut app, mut commands) = connected_view_app();
    app.input.set_text("/ keep suffix");
    app.input.set_cursor(0, 1);
    crate::app::slash::sync_with_cursor(&mut app);
    let first = app.slash.visible().unwrap().candidates[0].primary.as_str();
    let target = if first == "/help" { "/config" } else { "/help" };
    let count = app.slash.visible().unwrap().candidates.len();
    for _ in 0..count {
        let state = app.slash.visible().unwrap();
        if state.candidates[state.dialog.selected].primary == target {
            break;
        }
        routed_key(&mut app, KeyCode::Down);
    }
    set_surface_mode(&mut app, SurfaceMode::Chat);
    routed_key(&mut app, KeyCode::Tab);
    assert_eq!(app.input.text(), format!("{target} keep suffix"));
    assert!(commands.try_recv().is_err());
}

fn prepare_interrupt_view(app: &mut App, directory: &tempfile::TempDir, view: FullscreenView) {
    use crossterm::event::KeyCode;
    match view {
        FullscreenView::Config => {
            crate::app::config::open_tab(app, crate::app::ConfigTab::Help).unwrap();
            routed_key(app, KeyCode::Down);
        }
        FullscreenView::Diff => open_populated_diff(app, directory.path()),
        FullscreenView::Trusted => {
            let preferences = directory.path().join("trust.json");
            std::fs::write(&preferences, "{}").unwrap();
            app.trust.preferences_path = Some(preferences);
            crate::app::trust::initialize(app);
            routed_key(app, KeyCode::Down);
        }
        FullscreenView::SessionPicker => {
            app.recent_sessions = vec![crate::app::RecentSessionInfo {
                session_id: "picker-cedar".into(),
                summary: "Cedar selection".into(),
                custom_title: Some("Cedar selection".into()),
                first_prompt: None,
                cwd: None,
                git_branch: None,
                file_size_bytes: 1,
                last_modified_ms: 0,
            }];
            crate::app::session_picker::open(app);
            crate::app::handle_terminal_event(app, crossterm::event::Event::Paste("Cedar".into()));
            assert_eq!(app.session_picker.query, "Cedar");
            routed_key(app, KeyCode::Right);
            assert_eq!(app.session_picker.turn_session_id.as_deref(), Some("picker-cedar"));
        }
        FullscreenView::Update => {
            app.update_prompt = Some(
                crate::app::UpdatePrompt {
                    current_version: "1.2.3".into(),
                    latest_version: "9.8.7".into(),
                    release_url: "https://example.invalid/view".into(),
                    last_error: Some("PREPARED_UPDATE_FAILURE".into()),
                }
                .into(),
            );
            let next = crate::app::update_prompt::post_trust_surface(app);
            set_surface_mode(app, next);
            routed_key(app, KeyCode::Down);
        }
        _ => unreachable!(),
    }
}

#[test]
fn ctrl_c_dismisses_real_config_diff_picker_update_and_untrusted_views() {
    use crossterm::event::KeyCode;
    for view in [
        FullscreenView::Config,
        FullscreenView::Diff,
        FullscreenView::Trusted,
        FullscreenView::SessionPicker,
        FullscreenView::Update,
    ] {
        let (directory, mut app, mut commands) = connected_view_app();
        app.input.set_text("surviving draft 界");
        let settings_before = serde_json::to_value(&app.global_settings).unwrap();
        prepare_interrupt_view(&mut app, &directory, view);
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(view));
        interrupt_view(&mut app);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        assert!(!app.shutdown_requested());
        assert!(app.post_exit_action.is_none());
        assert_eq!(app.input.text(), "surviving draft 界");
        assert_eq!(
            serde_json::to_value(&app.global_settings).unwrap(),
            settings_before,
            "dismissal must not save or skip an update"
        );
        while let Ok(envelope) = commands.try_recv() {
            assert!(
                matches!(
                    envelope.command,
                    crate::agent::wire::BridgeCommand::GetRewindTargets { .. }
                        | crate::agent::wire::BridgeCommand::GetMcpSnapshot { .. }
                        | crate::agent::wire::BridgeCommand::GetStatusSnapshot { .. }
                        | crate::agent::wire::BridgeCommand::InspectSettings { .. }
                        | crate::agent::wire::BridgeCommand::GetContextUsage { .. }
                ),
                "dismissal emitted an action: {:?}",
                envelope.command
            );
        }
        if view == FullscreenView::Trusted {
            assert!(!app.is_project_trusted());
            assert_eq!(std::fs::read(app.trust.preferences_path.as_ref().unwrap()).unwrap(), b"{}");
            crate::app::trust::initialize(&mut app);
            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));
            interrupt_view(&mut app);
        } else {
            if view == FullscreenView::SessionPicker {
                crate::app::session_picker::open(&mut app);
                assert!(app.session_picker.turn_session_id.is_none());
                assert!(app.session_picker.query.is_empty());
                interrupt_view(&mut app);
            }
            routed_key(&mut app, KeyCode::Enter);
            crate::app::test_support::expect_session_prompt(
                &mut commands,
                "session-1",
                "surviving draft 界",
                &[],
                &[],
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn set_surface_mode_same_view_is_noop() {
    use crate::agent::{events::ClientEvent, types, wire::BridgeCommand};
    use crate::app::events::{handle_client_event, handle_terminal_event};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            let mut app = App::test_default();
            let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
            app.session_runtime.conn = Some(std::rc::Rc::new(connection));
            app.session_runtime.session_id =
                Some(crate::agent::model::SessionId::new("same-surface"));
            app.status = crate::app::AppStatus::Ready;
            handle_terminal_event(&mut app, Event::Paste("ACCEPTED_DRAFT".into()));
            set_surface_mode(&mut app, SurfaceMode::Chat);
            crate::app::finalize_pending_paste_event(&mut app);
            assert_eq!(app.input.text(), "ACCEPTED_DRAFT");
            assert!(commands.try_recv().is_err());
            handle_client_event(
                &mut app,
                ClientEvent::McpElicitationRequest {
                    session_id: "same-surface".into(),
                    request: types::ElicitationRequest {
                        request_id: "pending-form".into(),
                        server_name: "forms".into(),
                        message: "Pending answer".into(),
                        mode: types::ElicitationMode::Form,
                        url: None,
                        elicitation_id: None,
                        field_order: vec!["name".into()],
                        requested_schema: Some(serde_json::json!({
                            "type": "object", "properties": {"name": {"type": "string"}},
                            "required": ["name"]
                        })),
                    },
                },
            );
            assert!(crate::app::config::handle_paste(&mut app, "retained-answer"));
            set_surface_mode(&mut app, SurfaceMode::Fullscreen(FullscreenView::Config));
            assert!(commands.try_recv().is_err());
            assert!(crate::app::config::handle_paste(&mut app, "!"));
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)),
            );
            let command =
                tokio::time::timeout(std::time::Duration::from_secs(2), commands.recv_envelope())
                    .await
                    .unwrap()
                    .unwrap()
                    .command;
            assert!(matches!(command, BridgeCommand::ElicitationResponse {
                elicitation_request_id, action: types::ElicitationAction::Accept,
                content: Some(value), ..
            } if elicitation_request_id == "pending-form"
                && value == serde_json::json!({"name": "retained-answer!"})));
            assert_eq!(app.input.text(), "ACCEPTED_DRAFT");
            assert!(commands.try_recv().is_err());
        }))
        .await;
}
