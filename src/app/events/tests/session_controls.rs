// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::client::CommandReceiver;
use crate::agent::model::EffortLevel;
use crate::agent::wire::BridgeCommand;
use pretty_assertions::assert_eq;

const DRAFT: &str = "keep my draft";

fn session_app() -> (App, CommandReceiver) {
    let (mut app, commands) = app_with_bridge_connection();
    app.session_runtime.mode = Some(mode_state("default"));
    app.session_runtime.current_model =
        Some(effort_model(&[EffortLevel::Low, EffortLevel::High, EffortLevel::Max]));
    app.session_runtime.config_options.insert("effortLevel".to_owned(), serde_json::json!("high"));
    app.input.set_text(DRAFT);
    let _ = app.input.set_cursor_col(4);
    app.pending_images = vec![crate::app::clipboard_image::ImageAttachment {
        data: "image".to_owned(),
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

async fn next_command(commands: &mut CommandReceiver) -> Option<BridgeCommand> {
    tokio::task::yield_now().await;
    commands.try_recv().ok().map(|envelope| envelope.command)
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
    assert_eq!(app.pending_images.len(), 1);
    assert!(app.pending_submit.is_none());
}

async fn assert_session_unchanged(app: &App, commands: &mut CommandReceiver) {
    assert_eq!(next_command(commands).await, None);
    assert_ne!(app.status, AppStatus::CommandPending);
    assert_eq!(current_mode(app), "default");
    assert_eq!(app.session_effort(), Some(EffortLevel::High));
}

#[tokio::test(flavor = "current_thread")]
async fn shift_arrows_and_shift_tab_cycle_modes_in_both_directions() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();

            shift(&mut app, KeyCode::Left);
            assert_eq!(app.status, AppStatus::CommandPending);
            assert_eq!(current_mode(&app), "default");
            assert_eq!(next_command(&mut commands).await, Some(mode_request("plan")));
            ack_mode(&mut app, "plan");
            assert_eq!(app.status, AppStatus::Ready);

            shift(&mut app, KeyCode::Right);
            assert_eq!(next_command(&mut commands).await, Some(mode_request("default")));
            ack_mode(&mut app, "default");

            shift(&mut app, KeyCode::BackTab);
            assert_eq!(next_command(&mut commands).await, Some(mode_request("acceptEdits")));
            ack_mode(&mut app, "acceptEdits");

            shift(&mut app, KeyCode::Left);
            assert_eq!(next_command(&mut commands).await, Some(mode_request("default")));
            ack_mode(&mut app, "default");

            assert_eq!(current_mode(&app), "default");
            assert_composer_intact(&app);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn shift_up_and_down_step_through_supported_effort_levels_and_stop_at_the_ends() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();

            shift(&mut app, KeyCode::Up);
            assert_eq!(app.status, AppStatus::CommandPending);
            assert_eq!(app.session_effort(), Some(EffortLevel::High));
            assert_eq!(next_command(&mut commands).await, Some(effort_request("max")));
            ack_effort(&mut app, "max");
            assert_eq!(app.status, AppStatus::Ready);
            assert_eq!(app.session_effort(), Some(EffortLevel::Max));

            shift(&mut app, KeyCode::Up);
            assert_eq!(next_command(&mut commands).await, None);
            assert_eq!(app.status, AppStatus::Ready);

            shift(&mut app, KeyCode::Down);
            assert_eq!(next_command(&mut commands).await, Some(effort_request("high")));
            ack_effort(&mut app, "high");

            shift(&mut app, KeyCode::Down);
            assert_eq!(next_command(&mut commands).await, Some(effort_request("low")));
            ack_effort(&mut app, "low");

            shift(&mut app, KeyCode::Down);
            assert_eq!(next_command(&mut commands).await, None);
            assert_eq!(app.status, AppStatus::Ready);
            assert_eq!(app.session_effort(), Some(EffortLevel::Low));
            assert_composer_intact(&app);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_shortcuts_follow_the_levels_of_the_current_model() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            app.session_runtime.current_model =
                Some(effort_model(&[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High]));
            shift(&mut app, KeyCode::Up);
            assert_eq!(next_command(&mut commands).await, None);

            app.session_runtime.current_model =
                Some(effort_model(&[EffortLevel::Medium, EffortLevel::XHigh]));
            shift(&mut app, KeyCode::Up);
            assert_eq!(next_command(&mut commands).await, Some(effort_request("xhigh")));
            reject_pending_change(&mut app, "SDK rejected effort");

            shift(&mut app, KeyCode::Down);
            assert_eq!(next_command(&mut commands).await, Some(effort_request("medium")));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_shortcuts_send_nothing_when_the_model_lacks_effort_support() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            app.session_runtime.current_model = Some(
                effort_model(&[EffortLevel::Low, EffortLevel::High, EffortLevel::Max])
                    .supports_effort(false),
            );

            shift(&mut app, KeyCode::Up);
            shift(&mut app, KeyCode::Down);

            assert_session_unchanged(&app, &mut commands).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_shortcuts_send_nothing_while_the_session_effort_is_unknown() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            app.session_runtime.config_options.remove("effortLevel");

            shift(&mut app, KeyCode::Up);
            shift(&mut app, KeyCode::Down);

            assert_eq!(next_command(&mut commands).await, None);
            assert_eq!(app.status, AppStatus::Ready);
            assert_eq!(app.session_effort(), None);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn rapid_shortcuts_send_one_request_until_the_change_is_acknowledged() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();

            shift(&mut app, KeyCode::Right);
            shift(&mut app, KeyCode::Right);
            shift(&mut app, KeyCode::Up);
            shift(&mut app, KeyCode::BackTab);

            assert_eq!(next_command(&mut commands).await, Some(mode_request("acceptEdits")));
            assert_eq!(next_command(&mut commands).await, None);
            assert_eq!(current_mode(&app), "default");
            assert_eq!(app.session_effort(), Some(EffortLevel::High));

            ack_mode(&mut app, "acceptEdits");
            shift(&mut app, KeyCode::Right);
            assert_eq!(next_command(&mut commands).await, Some(mode_request("plan")));
            assert_composer_intact(&app);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn rejected_effort_change_keeps_the_acknowledged_effort() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();

            shift(&mut app, KeyCode::Down);
            assert_eq!(next_command(&mut commands).await, Some(effort_request("low")));
            reject_pending_change(&mut app, "SDK rejected effort");

            assert_eq!(app.status, AppStatus::Ready);
            assert_eq!(app.session_effort(), Some(EffortLevel::High));
            assert!(canonical_messages_contain_text(&app, "SDK rejected effort"));
            assert_composer_intact(&app);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn arrows_with_extra_modifiers_do_not_change_mode_or_effort() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            for code in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
                for modifiers in [
                    KeyModifiers::SHIFT | KeyModifiers::CONTROL,
                    KeyModifiers::SHIFT | KeyModifiers::ALT,
                ] {
                    press(&mut app, code, modifiers);
                    assert_eq!(next_command(&mut commands).await, None, "{modifiers:?}+{code:?}");
                }
            }
            assert_session_unchanged(&app, &mut commands).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn plain_arrows_keep_moving_the_cursor() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();

            press(&mut app, KeyCode::Left, KeyModifiers::NONE);
            assert_eq!(app.input.cursor_col(), 3);
            press(&mut app, KeyCode::Right, KeyModifiers::NONE);

            assert_session_unchanged(&app, &mut commands).await;
            assert_composer_intact(&app);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_do_not_reach_past_a_focused_permission() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            let mut response_rx = attach_pending_permission(
                &mut app,
                "perm-1",
                vec![
                    model::PermissionOption::new(
                        "allow",
                        "Allow",
                        model::PermissionOptionKind::AllowOnce,
                    ),
                    model::PermissionOption::new(
                        "deny",
                        "Deny",
                        model::PermissionOptionKind::RejectOnce,
                    ),
                ],
                true,
            );

            press_every_session_shortcut(&mut app);

            assert_session_unchanged(&app, &mut commands).await;
            assert_eq!(app.focus_owner(), FocusOwner::Permission);
            assert!(matches!(
                response_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_do_not_reach_past_a_focused_question() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            let mut response_rx = attach_pending_question(
                &mut app,
                "question-1",
                model::QuestionPrompt::new(
                    "Choose one",
                    "Question",
                    false,
                    vec![
                        model::QuestionOption::new("yes", "Yes"),
                        model::QuestionOption::new("no", "No"),
                    ],
                ),
                true,
            );

            press_every_session_shortcut(&mut app);

            assert_session_unchanged(&app, &mut commands).await;
            assert_eq!(app.focus_owner(), FocusOwner::Permission);
            assert!(matches!(
                response_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_leave_an_open_mention_menu_in_place() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            app.mention = Some(mention::MentionState::new(
                0,
                0,
                String::new(),
                vec![crate::app::file_index::FileCandidate {
                    rel_path: "src/lib.rs".to_owned(),
                    rel_path_lower: "src/lib.rs".to_owned(),
                    basename_lower: "lib.rs".to_owned(),
                    depth: 1,
                }],
            ));
            app.claim_focus_target(FocusTarget::Mention);
            assert_eq!(app.focus_owner(), FocusOwner::Mention);

            for code in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
                shift(&mut app, code);
            }

            assert!(app.mention.is_some());
            assert_eq!(app.focus_owner(), FocusOwner::Mention);
            assert_session_unchanged(&app, &mut commands).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_leave_an_open_slash_menu_in_place() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            show_config_slash_menu(&mut app);
            app.claim_focus_target(FocusTarget::Mention);

            for code in [KeyCode::Left, KeyCode::Right, KeyCode::Up, KeyCode::Down] {
                shift(&mut app, code);
            }

            assert!(app.slash.is_visible());
            assert_session_unchanged(&app, &mut commands).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_do_nothing_in_a_fullscreen_view() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
            app.terminal_lifecycle =
                TerminalLifecycleState::Running(SurfaceMode::Fullscreen(FullscreenView::Config));

            press_every_session_shortcut(&mut app);

            assert_session_unchanged(&app, &mut commands).await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn session_shortcuts_do_nothing_during_a_turn() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = session_app();
            app.status = AppStatus::Thinking;

            press_every_session_shortcut(&mut app);

            assert_session_unchanged(&app, &mut commands).await;
            assert_eq!(app.status, AppStatus::Thinking);
        })
        .await;
}
