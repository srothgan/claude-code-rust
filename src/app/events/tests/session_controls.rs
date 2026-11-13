// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::client::CommandReceiver;
use crate::agent::model::EffortLevel;
use crate::agent::wire::BridgeCommand;
use pretty_assertions::assert_eq;

const DRAFT: &str = "keep my draft [Image #1]";
const IMAGE: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAADklEQVR4nGP4z8AAQg0AD3oDfnfpf5cAAAAASUVORK5CYII=";

fn session_app() -> (App, CommandReceiver) {
    let (mut app, commands) = app_with_bridge_connection();
    app.session_runtime.mode = Some(mode_state("default"));
    app.session_runtime.current_model =
        Some(effort_model(&[EffortLevel::Low, EffortLevel::High, EffortLevel::Max]));
    app.session_runtime.config_options.insert("effortLevel".to_owned(), serde_json::json!("high"));
    app.input.set_text(DRAFT);
    let _ = app.input.set_cursor_col(4);
    app.input.image_payloads = vec![crate::app::clipboard_image::ImageAttachment {
        data: IMAGE.to_owned(),
        mime_type: "image/png".to_owned(),
    }];
    (app, commands)
}

fn mode_state(current: &str) -> crate::app::ModeState {
    let available_modes =
        [("default", "Default"), ("acceptEdits", "Accept Edits"), ("plan", "Plan")]
            .into_iter()
            .map(|(id, name)| crate::app::ModeInfo { id: id.to_owned(), name: name.to_owned() })
            .collect::<Vec<_>>();
    let current_mode_name = available_modes
        .iter()
        .find(|mode| mode.id == current)
        .map(|mode| mode.name.clone())
        .expect("known mode");
    crate::app::ModeState {
        current_mode_id: current.to_owned(),
        current_mode_name,
        available_modes,
    }
}

fn effort_model(levels: &[EffortLevel]) -> model::CurrentModel {
    test_current_model("opus").supports_effort(true).supported_effort_levels(levels.to_vec())
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, modifiers)));
}

fn shift(app: &mut App, code: KeyCode) {
    press(app, code, KeyModifiers::SHIFT);
}

fn press_every_session_shortcut(app: &mut App) {
    for code in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down, KeyCode::BackTab] {
        shift(app, code);
    }
}

fn mode_request(mode: &str) -> BridgeCommand {
    BridgeCommand::SetMode { session_id: "test-session".to_owned(), mode: mode.to_owned() }
}

fn effort_request(effort: &str) -> BridgeCommand {
    BridgeCommand::SetEffort {
        session_id: "test-session".to_owned(),
        effort: Some(effort.to_owned()),
    }
}

fn ack_mode(app: &mut App, mode: &str) {
    handle_client_event(
        app,
        session_update(model::SessionUpdate::ModeStateUpdate(mode_state(mode))),
    );
}

fn ack_effort(app: &mut App, effort: &str) {
    handle_client_event(
        app,
        session_update(model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
            option_id: "effortLevel".to_owned(),
            value: serde_json::json!(effort),
        })),
    );
}

fn reject_pending_change(app: &mut App, message: &str) {
    handle_client_event(
        app,
        ClientEvent::SlashCommandError {
            session_id: Some("test-session".to_owned()),
            message: message.to_owned(),
        },
    );
}

fn current_mode(app: &App) -> &str {
    &app.session_runtime.mode.as_ref().expect("mode").current_mode_id
}

fn assert_composer_intact(app: &App) {
    assert_eq!(app.input.text(), DRAFT);
    assert_eq!(app.input.cursor_col(), 4);
    assert_eq!(app.input.image_payloads, [fixture_image()]);
    assert!(app.pending_submit.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn shift_arrows_and_shift_tab_cycle_modes_in_both_directions() {
    for (order, initial, steps) in [
        (
            vec!["default", "acceptEdits", "plan"],
            "default",
            vec![
                (KeyCode::Left, "plan"),
                (KeyCode::Right, "default"),
                (KeyCode::BackTab, "acceptEdits"),
                (KeyCode::Left, "default"),
            ],
        ),
        (
            vec!["review", "plan", "default"],
            "default",
            vec![
                (KeyCode::Right, "review"),
                (KeyCode::BackTab, "plan"),
                (KeyCode::Left, "review"),
                (KeyCode::Left, "default"),
            ],
        ),
    ] {
        let (mut app, mut commands) = session_app();
        mode_metadata(&mut app, initial, &order);
        let mut old = initial;
        for (code, target) in steps {
            dispatch(&mut app, |app| shift(app, code)).await;
            assert_eq!(received(&mut commands).await, mode_request(target));
            assert_eq!(current_mode(&app), old);
            assert_eq!(app.status, AppStatus::CommandPending);
            mode_metadata(&mut app, target, &order);
            assert_eq!(current_mode(&app), target);
            assert_eq!(app.status, AppStatus::Ready);
            assert_composer_intact(&app);
            assert!(commands.try_recv().is_err());
            old = target;
        }
    }
    mode_rejection_and_pending_gate().await;
    for absent in [false, true] {
        let (mut app, mut commands) = session_app();
        if absent {
            app.session_runtime.mode = None;
        } else {
            mode_metadata(&mut app, "default", &["default"]);
        }
        dispatch(&mut app, |app| {
            shift(app, KeyCode::Left);
            shift(app, KeyCode::Right);
            shift(app, KeyCode::BackTab);
        })
        .await;
        assert!(commands.try_recv().is_err());
        assert_composer_intact(&app);
        mode_metadata(&mut app, "default", &["default", "plan"]);
        dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
        assert_eq!(received(&mut commands).await, mode_request("plan"));
        mode_metadata(&mut app, "plan", &["default", "plan"]);
        assert_eq!(current_mode(&app), "plan");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn shift_up_and_down_step_through_supported_effort_levels_and_stop_at_the_ends() {
    let (mut app, mut commands) = session_app();
    for (key, old, target) in [
        (KeyCode::Up, EffortLevel::High, "max"),
        (KeyCode::Down, EffortLevel::Max, "high"),
        (KeyCode::Down, EffortLevel::High, "low"),
    ] {
        dispatch(&mut app, |app| shift(app, key)).await;
        assert_eq!(received(&mut commands).await, effort_request(target));
        assert_eq!(app.session_effort(), Some(old));
        ack_effort(&mut app, target);
        assert_eq!(app.session_effort().unwrap().as_stored(), target);
        assert_composer_intact(&app);
        if target == "max" || target == "low" {
            dispatch(&mut app, |app| {
                shift(app, if target == "max" { KeyCode::Up } else { KeyCode::Down });
            })
            .await;
            assert!(commands.try_recv().is_err());
            assert_eq!(app.status, AppStatus::Ready);
        }
    }
    for (key, target) in [(KeyCode::Up, "xhigh"), (KeyCode::Down, "medium")] {
        let (mut app, mut commands) = session_app();
        model_metadata(
            &mut app,
            effort_model(&[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High]),
        );
        dispatch(&mut app, |app| shift(app, KeyCode::Up)).await;
        assert!(commands.try_recv().is_err());
        model_metadata(&mut app, effort_model(&[EffortLevel::Medium, EffortLevel::XHigh]));
        dispatch(&mut app, |app| shift(app, key)).await;
        assert_eq!(received(&mut commands).await, effort_request(target));
        assert_eq!(app.session_effort(), Some(EffortLevel::High));
        reject_pending_change(&mut app, "distinct effort rejection");
        assert!(canonical_messages_contain_text(&app, "distinct effort rejection"));
        assert_eq!(app.session_effort(), Some(EffortLevel::High));
        assert_composer_intact(&app);
        dispatch(&mut app, |app| shift(app, key)).await;
        assert_eq!(received(&mut commands).await, effort_request(target));
        ack_effort(&mut app, target);
        assert_eq!(app.session_effort().unwrap().as_stored(), target);
        assert_composer_intact(&app);
    }
    effort_metadata_controls().await;
}

#[tokio::test(flavor = "current_thread")]
async fn only_exact_shortcut_modifiers_change_session_controls() {
    let (mut app, mut commands) = session_app();
    for (key, col) in [(KeyCode::Left, 3), (KeyCode::Right, 4)] {
        dispatch(&mut app, |app| press(app, key, KeyModifiers::NONE)).await;
        assert_eq!(app.input.cursor(), (0, col));
        assert_eq!(app.input.text(), DRAFT);
        assert!(commands.try_recv().is_err());
    }
    for key in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
        for modifier in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            dispatch(&mut app, |app| press(app, key, modifier | KeyModifiers::SHIFT)).await;
            assert!(commands.try_recv().is_err());
            assert_eq!(current_mode(&app), "default");
            assert_eq!(app.session_effort(), Some(EffortLevel::High));
            assert_eq!(app.input.text(), DRAFT);
            assert_eq!(app.input.image_payloads, [fixture_image()]);
        }
    }
    dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
    assert_eq!(received(&mut commands).await, mode_request("acceptEdits"));
    ack_mode(&mut app, "acceptEdits");
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_preserve_focused_permission_and_question_responses() {
    let (mut app, mut commands) = session_app();
    append_tool_call_block(&mut app, "control-permission");
    let (tx, mut rx) = oneshot::channel();
    handle_client_event(
        &mut app,
        ClientEvent::PermissionRequest {
            session_id: "test-session".into(),
            request: model::RequestPermissionRequest::new(
                "test-session",
                model::ToolCallUpdate::new(
                    "control-permission",
                    model::ToolCallUpdateFields::new(),
                ),
                vec![
                    model::PermissionOption::new(
                        "allow-once",
                        "Allow",
                        model::PermissionOptionKind::AllowOnce,
                    ),
                    model::PermissionOption::new(
                        "deny",
                        "Deny",
                        model::PermissionOptionKind::RejectOnce,
                    ),
                ],
                None,
            ),
            response_tx: tx,
        },
    );
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(permission_focus_state(&app, "control-permission"), Some(true));
    dispatch(&mut app, press_every_session_shortcut).await;
    assert_no_controls(&mut commands);
    assert_composer_intact(&app);
    crate::app::test_support::assert_pending(&mut rx);
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    crate::app::test_support::assert_permission_response(&mut rx, "deny");
    assert_eq!(app.focus_owner(), FocusOwner::Input);
    dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
    assert_eq!(received(&mut commands).await, mode_request("acceptEdits"));
    ack_mode(&mut app, "acceptEdits");
    assert_composer_intact(&app);
    question_isolation().await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_preserve_usable_file_and_slash_completion() {
    for file in [true, false] {
        let (mut app, mut commands) = session_app();
        let fixture = tempfile::tempdir().unwrap();
        if file {
            app.cwd_raw = fixture.path().to_string_lossy().into_owned();
            app.file_index.root = Some(fixture.path().to_path_buf());
            app.file_index.respect_gitignore = app.config.respect_gitignore_effective();
            app.file_index.status = crate::app::file_index::FileIndexStatus::Complete;
            app.file_index.entries.insert(
                "fixture.txt".into(),
                crate::app::file_index::FileCandidate {
                    rel_path: "fixture.txt".into(),
                    rel_path_lower: "fixture.txt".into(),
                    basename_lower: "fixture.txt".into(),
                    depth: 0,
                },
            );
        }
        app.input.set_text(if file {
            "@fixture keep my draft [Image #1]"
        } else {
            "/mo keep my draft [Image #1]"
        });
        app.input.image_payloads = vec![fixture_image()];
        app.input.set_cursor(0, if file { 7 } else { 2 });
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        if file {
            await_correlated_file_search(&mut app, "fixture", &["fixture.txt"]);
        }
        assert_eq!(app.focus_owner(), FocusOwner::Mention);
        dispatch(&mut app, press_every_session_shortcut).await;
        assert_no_controls(&mut commands);
        if file {
            assert_eq!(app.mention.as_ref().expect("same active token").query, "fixture");
            assert_completion_menu(&app, &["fixture.txt"], Some("fixture.txt"));
        } else {
            select_slash_for_render(&mut app, "/mode");
            assert_selected_slash_row(&app, "/mode");
        }
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(
            app.input.text(),
            if file {
                "@'fixture.txt' keep my draft [Image #1]"
            } else {
                "/mode keep my draft [Image #1]"
            }
        );
        assert!(app.pending_submit.is_none());
        assert_eq!(app.input.image_payloads, [fixture_image()]);
        if app.focus_owner() == FocusOwner::Mention {
            press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        }
        app.input.set_text(DRAFT);
        app.input.image_payloads = vec![fixture_image()];
        app.input.set_cursor(0, 3);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
        dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
        assert_eq!(received(&mut commands).await, mode_request("acceptEdits"));
        ack_mode(&mut app, "acceptEdits");
        assert_composer_intact(&app);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_do_nothing_in_a_fullscreen_view() {
    let (mut app, mut commands) = session_app();
    app.trust.status = crate::app::trust::TrustStatus::Trusted;
    dispatch(&mut app, |app| crate::app::config::open(app).expect("trusted config action")).await;
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    dispatch(&mut app, press_every_session_shortcut).await;
    assert_no_controls(&mut commands);
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
    dispatch(&mut app, |app| press(app, KeyCode::Char('c'), KeyModifiers::CONTROL)).await;
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_composer_intact(&app);
    dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
    assert_eq!(received(&mut commands).await, mode_request("acceptEdits"));
    ack_mode(&mut app, "acceptEdits");
    assert_composer_intact(&app);
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_do_nothing_during_a_turn() {
    let (mut app, mut commands) = session_app();
    app.input.set_text("start control turn");
    app.input.image_payloads.clear();
    dispatch(&mut app, |app| crate::app::test_support::press(app, KeyCode::Enter)).await;
    let request = received(&mut commands).await;
    assert!(
        matches!(request,BridgeCommand::Prompt {session_id,chunks,..} if session_id=="test-session" && chunks.iter().any(|c|c.kind=="text" && c.value==serde_json::json!("start control turn")))
    );
    assert_no_controls(&mut commands);
    app.input.set_text(DRAFT);
    app.input.set_cursor(0, 4);
    app.input.image_payloads = vec![fixture_image()];
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentActivityUpdate(
            model::AgentActivityPhase::Thinking,
        )),
    );
    assert_eq!(app.status, AppStatus::Thinking);
    dispatch(&mut app, press_every_session_shortcut).await;
    assert_no_controls(&mut commands);
    assert_composer_intact(&app);
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("control reply ")),
        ))),
    );
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentActivityUpdate(
            model::AgentActivityPhase::Working,
        )),
    );
    assert_eq!(app.status, AppStatus::Running);
    let owner = app.active_turn_assistant_idx().unwrap();
    let id = app.transcript.messages[owner].id;
    dispatch(&mut app, press_every_session_shortcut).await;
    assert_no_controls(&mut commands);
    assert_composer_intact(&app);
    assert_eq!(current_mode(&app), "default");
    assert_eq!(app.session_effort(), Some(EffortLevel::High));
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("continued")),
        ))),
    );
    assert_eq!(app.active_turn_assistant_idx(), Some(owner));
    assert_eq!(app.transcript.messages[owner].id, id);
    assert!(
        matches!(&app.transcript.messages[owner].blocks[0],MessageBlock::Text(t) if t.text=="control reply continued")
    );
    handle_client_event(&mut app, turn_complete(None));
    assert_eq!(app.status, AppStatus::Ready);
    assert_no_controls(&mut commands);
    dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
    assert_eq!(received(&mut commands).await, mode_request("acceptEdits"));
    ack_mode(&mut app, "acceptEdits");
    assert_composer_intact(&app);
}

fn fixture_image() -> crate::app::clipboard_image::ImageAttachment {
    crate::app::clipboard_image::ImageAttachment {
        data: IMAGE.into(),
        mime_type: "image/png".into(),
    }
}

// Await completion of every local task spawned by this action. Negative command
// assertions after this barrier cannot win a race against a queued dispatch.
async fn dispatch(app: &mut App, action: impl FnOnce(&mut App)) {
    let tasks = tokio::task::LocalSet::new();
    tasks
        .run_until(async {
            action(app);
        })
        .await;
    tokio::time::timeout(Duration::from_secs(2), tasks).await.expect("dispatch tasks complete");
}

async fn received(commands: &mut CommandReceiver) -> BridgeCommand {
    tokio::time::timeout(Duration::from_secs(2), commands.recv_envelope())
        .await
        .expect("bounded command")
        .expect("command")
        .command
}

fn assert_no_controls(commands: &mut CommandReceiver) {
    while let Ok(envelope) = commands.try_recv() {
        assert!(
            !matches!(
                envelope.command,
                BridgeCommand::SetMode { .. }
                    | BridgeCommand::SetEffort { .. }
                    | BridgeCommand::Prompt { .. }
                    | BridgeCommand::CancelTurn { .. }
            ),
            "unexpected exclusive command: {:?}",
            envelope.command
        );
    }
}

fn mode_metadata(app: &mut App, current: &str, order: &[&str]) {
    handle_client_event(
        app,
        session_update(model::SessionUpdate::ModeStateUpdate(crate::app::ModeState {
            current_mode_id: current.into(),
            current_mode_name: current.into(),
            available_modes: order
                .iter()
                .map(|id| crate::app::ModeInfo { id: (*id).into(), name: (*id).into() })
                .collect(),
        })),
    );
}

fn model_metadata(app: &mut App, model: model::CurrentModel) {
    handle_client_event(
        app,
        session_update(model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
            model,
        ))),
    );
}

async fn mode_rejection_and_pending_gate() {
    let (mut app, mut commands) = session_app();
    dispatch(&mut app, |app| shift(app, KeyCode::Left)).await;
    assert_eq!(received(&mut commands).await, mode_request("plan"));
    ack_mode(&mut app, "plan");
    dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
    assert_eq!(received(&mut commands).await, mode_request("default"));
    reject_pending_change(&mut app, "distinct mode rejection");
    assert_eq!(current_mode(&app), "plan");
    assert!(canonical_messages_contain_text(&app, "distinct mode rejection"));
    assert_composer_intact(&app);
    dispatch(&mut app, |app| {
        shift(app, KeyCode::Right);
        press_every_session_shortcut(app);
    })
    .await;
    assert_eq!(received(&mut commands).await, mode_request("default"));
    assert!(commands.try_recv().is_err());
    assert_eq!(current_mode(&app), "plan");
    ack_effort(&mut app, "high");
    assert_eq!(app.status, AppStatus::CommandPending);
    dispatch(&mut app, press_every_session_shortcut).await;
    assert!(commands.try_recv().is_err());
    ack_mode(&mut app, "default");
    assert_eq!(app.status, AppStatus::Ready);
    dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
    assert_eq!(received(&mut commands).await, mode_request("acceptEdits"));
    ack_mode(&mut app, "acceptEdits");
    assert_composer_intact(&app);
}

async fn effort_metadata_controls() {
    for missing_current in [false, true] {
        let (mut app, mut commands) = session_app();
        if missing_current {
            app.session_runtime.config_options.remove("effortLevel");
        } else {
            model_metadata(
                &mut app,
                effort_model(&[EffortLevel::Low, EffortLevel::High, EffortLevel::Max])
                    .supports_effort(false),
            );
        }
        dispatch(&mut app, |app| {
            shift(app, KeyCode::Up);
            shift(app, KeyCode::Down);
        })
        .await;
        assert!(commands.try_recv().is_err());
        assert_eq!(app.status, AppStatus::Ready);
        assert_composer_intact(&app);
        assert_eq!(
            app.session_effort(),
            if missing_current { None } else { Some(EffortLevel::High) }
        );
        if missing_current {
            ack_effort(&mut app, "high");
        } else {
            model_metadata(&mut app, effort_model(&[EffortLevel::High, EffortLevel::Max]));
        }
        dispatch(&mut app, |app| shift(app, KeyCode::Up)).await;
        assert_eq!(received(&mut commands).await, effort_request("max"));
        assert_eq!(app.session_effort(), Some(EffortLevel::High));
        ack_effort(&mut app, "max");
        assert_eq!(app.session_effort(), Some(EffortLevel::Max));
        assert_composer_intact(&app);
    }
}

async fn question_isolation() {
    let (mut app, mut commands) = session_app();
    append_tool_call_block(&mut app, "control-question");
    let (tx, mut rx) = oneshot::channel();
    handle_client_event(
        &mut app,
        ClientEvent::QuestionRequest {
            session_id: "test-session".into(),
            request: model::RequestQuestionRequest::new(
                "test-session",
                model::ToolCallUpdate::new("control-question", model::ToolCallUpdateFields::new()),
                model::QuestionPrompt::new(
                    "Choose",
                    "Control",
                    false,
                    vec![
                        model::QuestionOption::new("yes", "Yes"),
                        model::QuestionOption::new("no", "No"),
                    ],
                ),
                0,
                1,
            ),
            response_tx: tx,
        },
    );
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(question_focus_state(&app, "control-question"), Some(true));
    dispatch(&mut app, press_every_session_shortcut).await;
    crate::app::test_support::assert_pending(&mut rx);
    assert_no_controls(&mut commands);
    assert_composer_intact(&app);
    // Shift+Tab legitimately enters the question's notes editor.
    if app.tool_call("control-question").unwrap().pending_question.as_ref().unwrap().editing_notes {
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    }
    press(&mut app, KeyCode::Home, KeyModifiers::NONE);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        rx.try_recv().expect("explicit question answer").outcome,
        model::RequestQuestionOutcome::Answered(model::AnsweredQuestionOutcome::new(vec![
            "no".into()
        ]))
    );
    assert_eq!(app.focus_owner(), FocusOwner::Input);
    dispatch(&mut app, |app| shift(app, KeyCode::Right)).await;
    assert_eq!(received(&mut commands).await, mode_request("acceptEdits"));
    ack_mode(&mut app, "acceptEdits");
    assert_composer_intact(&app);
}

#[tokio::test(flavor = "current_thread")]
async fn session_control_send_failure_releases_pending_state_and_allows_retry() {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD.decode(IMAGE).unwrap();
    assert_eq!(
        image::load_from_memory(&bytes).unwrap().to_rgba8().into_raw(),
        [255, 0, 0, 255, 0, 0, 255, 128]
    );
    for mode in [false, true] {
        let (mut app, commands) = session_app();
        drop(commands);
        let key = if mode { KeyCode::Right } else { KeyCode::Down };
        dispatch(&mut app, |app| shift(app, key)).await;
        assert_eq!(app.status, AppStatus::CommandPending);
        let event = tokio::time::timeout(Duration::from_secs(2), app.event_rx.recv())
            .await
            .expect("real send failure")
            .expect("failure event");
        let ClientEvent::SlashCommandError { session_id, message } = &event else {
            panic!("expected send failure")
        };
        assert_eq!(session_id.as_deref(), Some("test-session"));
        assert!(message.contains(if mode { "/mode" } else { "/effort" }));
        assert!(message.contains("closed"));
        let cause = message.clone();
        handle_client_event(&mut app, event);
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(current_mode(&app), "default");
        assert_eq!(app.session_effort(), Some(EffortLevel::High));
        assert_composer_intact(&app);
        assert!(canonical_messages_contain_text(&app, &cause));
        let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(Rc::new(connection));
        dispatch(&mut app, |app| shift(app, key)).await;
        assert_eq!(
            received(&mut commands).await,
            if mode { mode_request("acceptEdits") } else { effort_request("low") }
        );
        assert_eq!(current_mode(&app), "default");
        assert_eq!(app.session_effort(), Some(EffortLevel::High));
        if mode {
            ack_mode(&mut app, "acceptEdits");
        } else {
            ack_effort(&mut app, "low");
        }
        assert_eq!(app.status, AppStatus::Ready);
        assert_composer_intact(&app);
        assert!(commands.try_recv().is_err());
        dispatch(&mut app, |app| shift(app, if mode { KeyCode::Left } else { KeyCode::Up })).await;
        assert_eq!(
            received(&mut commands).await,
            if mode { mode_request("default") } else { effort_request("high") }
        );
        if mode {
            ack_mode(&mut app, "default");
        } else {
            ack_effort(&mut app, "high");
        }
        assert_composer_intact(&app);
    }
}
