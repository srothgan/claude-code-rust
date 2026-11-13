// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::{events::ClientEvent, model, wire::BridgeCommand};
use crate::app::activity::testing::{Clock, choices};
use crate::ui::host_tips::HOST_TIPS;
use std::time::{Duration, Instant};

fn fixture() -> (SessionFixture, crate::agent::client::CommandReceiver) {
    let mut f = SessionFixture::new(120, 45);
    let (connection, rx) = crate::agent::client::AgentConnection::test_channel();
    f.app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    f.message("KEPT_HISTORY");
    setting(&mut f, "prefersReducedMotion", true);
    (f, rx)
}
fn setting(f: &mut SessionFixture, id: &str, value: bool) {
    let snapshot =
        f.app.config.snapshot.get_or_insert_with(crate::agent::settings::SettingsSnapshot::default);
    snapshot.values.retain(|entry| entry.id != id);
    snapshot.values.extend(
        crate::agent::settings::SettingsSnapshot::test_value(id, serde_json::json!(value)).values,
    );
    f.app.request_chat_repaint();
}
#[allow(clippy::expect_used)]
fn event(f: &mut SessionFixture, update: model::SessionUpdate) {
    let session_id = f.app.session_runtime.session_id.as_ref().expect("session").to_string();
    crate::app::handle_client_event(&mut f.app, ClientEvent::SessionUpdate { session_id, update });
}
fn admit(f: &mut SessionFixture, rx: &mut crate::agent::client::CommandReceiver) -> &'static str {
    f.app.input.set_text("ADMITTED_WORK");
    crate::app::test_support::press(&mut f.app, crossterm::event::KeyCode::Enter);
    let mut prompt = false;
    while let Ok(command) = rx.try_recv() {
        prompt |= matches!(command.command, BridgeCommand::Prompt { .. });
    }
    assert!(prompt, "turn must enter the real command queue");
    let verb = choices(&mut f.app);
    f.app.input.set_text("UNSENT_DRAFT");
    f.app.request_chat_repaint();
    f.settle();
    verb
}
fn activity(f: &mut SessionFixture) -> Vec<String> {
    let count = ChatTerminalSession::build_composer_surface(&mut f.app, 120).activity_rows.len();
    let start = usize::from(
        f.session.terminal.rendered_area().y + f.app.chat_render.live_region.last_rendered_rows,
    );
    f.io.screen_rows().into_iter().skip(start).take(count).collect()
}
fn assert_no_activity(f: &mut SessionFixture, verb: Option<&str>) {
    assert!(activity(f).is_empty());
    let screen = f.io.screen_rows();
    assert!(
        !screen.iter().any(|row| row.contains("Tip:")
            || row.contains("Compacting")
            || row.contains("Cancelling")),
        "stale activity on the physical screen: {screen:?}"
    );
    if let Some(verb) = verb {
        assert!(
            !screen.iter().any(|row| row.contains('?') && row.contains(verb)),
            "stale activity label on the physical screen: {screen:?}"
        );
    }
}
fn dense(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace() && *c != '│').collect()
}
fn contains_tip(rows: &[String], tip: &str) -> bool {
    dense(&rows.join("")).contains(&dense(tip))
}
fn tick(f: &mut SessionFixture, clock: &Clock, at: Instant) {
    clock.set(at);
    f.app.tick_activity(at);
    if f.app.surface_dirty.active_surface_needs_draw(f.app.terminal_lifecycle)
        || f.session.has_pending_work()
    {
        f.settle();
    }
}
fn permission(
    f: &mut SessionFixture,
    id: &str,
) -> tokio::sync::oneshot::Receiver<model::RequestPermissionResponse> {
    event(
        f,
        model::SessionUpdate::ToolCall(
            model::ToolCall::new(id, "Read").status(model::ToolCallStatus::InProgress),
        ),
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    crate::app::handle_client_event(
        &mut f.app,
        ClientEvent::PermissionRequest {
            session_id: "test-session".into(),
            request: model::RequestPermissionRequest::new(
                "test-session",
                model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                vec![model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                )],
                None,
            ),
            response_tx: tx,
        },
    );
    f.settle();
    rx
}

#[test]
fn admitted_activity_keeps_choices_through_output_and_real_wait_resume() {
    let base = Instant::now();
    let clock = Clock::at(base);
    let (mut f, mut commands) = fixture();
    let verb = admit(&mut f, &mut commands);
    assert!(contains_tip(&activity(&mut f), HOST_TIPS[0]));
    for update in [
        model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::Running),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("RECEIVED_OUTPUT")),
        )),
        model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Working),
        model::SessionUpdate::ToolCall(
            model::ToolCall::new("work", "Read").status(model::ToolCallStatus::Completed),
        ),
    ] {
        clock.set(base + Duration::from_secs(20));
        event(&mut f, update);
        f.settle();
        assert!(activity(&mut f).iter().any(|row| row.contains(verb)));
        assert!(contains_tip(&activity(&mut f), HOST_TIPS[0]));
    }
    tick(&mut f, &clock, base + Duration::from_secs(61));
    assert!(contains_tip(&activity(&mut f), HOST_TIPS[1]));
    // A repeated Running notification is still the same admitted turn.
    event(
        &mut f,
        model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::Running),
    );
    let mut response = permission(&mut f, "permission");
    assert_no_activity(&mut f, Some(verb));
    event(
        &mut f,
        model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::RequiresAction),
    );
    if f.app.focus_owner() != crate::app::FocusOwner::Permission {
        crate::app::test_support::press(&mut f.app, crossterm::event::KeyCode::Tab);
    }
    assert_eq!(f.app.focus_owner(), crate::app::FocusOwner::Permission);
    crate::app::test_support::press(&mut f.app, crossterm::event::KeyCode::Enter);
    assert!(response.try_recv().is_ok());
    f.settle();
    assert_no_activity(&mut f, Some(verb));
    tick(&mut f, &clock, base + Duration::from_secs(130));
    event(
        &mut f,
        model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::Running),
    );
    f.settle();
    assert!(contains_tip(&activity(&mut f), HOST_TIPS[2]));
    assert!(activity(&mut f).iter().any(|row| row.contains(verb)));
    f.complete_turn();
    f.settle();
    assert_no_activity(&mut f, Some(verb));
    clock.set(base + Duration::from_secs(200));
    admit(&mut f, &mut commands);
    assert!(contains_tip(&activity(&mut f), HOST_TIPS[0]));
    assert!(f.io.transcript().iter().any(|row| row.contains("RECEIVED_OUTPUT")));
}

#[test]
fn deadline_ticks_draw_complete_tip_traversals_without_animation_or_hidden_repaints() {
    let base = Instant::now();
    let clock = Clock::at(base);
    let (mut f, mut commands) = fixture();
    let verb = admit(&mut f, &mut commands);
    let mut shown = std::collections::BTreeSet::new();
    for (slot, expected) in HOST_TIPS.iter().enumerate() {
        f.io.take_operations();
        tick(&mut f, &clock, base + Duration::from_secs(60 * u64::try_from(slot).unwrap()));
        let rows = activity(&mut f);
        assert!(contains_tip(&rows, expected), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains(verb)));
        assert!(shown.insert(*expected), "a traversal must not repeat a tip");
        f.io.take_operations();
        tick(&mut f, &clock, base + Duration::from_secs(60 * u64::try_from(slot).unwrap() + 1));
        assert!(!f.io.take_operations().iter().any(|op| matches!(
            op,
            crate::app::terminal_runtime::chat_terminal::test_io::Operation::Draw(_)
        )));
    }
    assert_eq!(shown, HOST_TIPS.iter().copied().collect());
    let cycle = 60 * u64::try_from(HOST_TIPS.len()).unwrap();
    setting(&mut f, "spinnerTipsEnabled", false);
    f.settle();
    f.io.take_operations();
    tick(&mut f, &clock, base + Duration::from_secs(cycle + 61));
    assert!(!activity(&mut f).iter().any(|row| row.contains("Tip:")));
    assert!(!f.io.take_operations().iter().any(|op| matches!(
        op,
        crate::app::terminal_runtime::chat_terminal::test_io::Operation::Draw(_)
    )));
    setting(&mut f, "spinnerTipsEnabled", true);
    f.settle();
    assert!(contains_tip(&activity(&mut f), HOST_TIPS[1]));
    let mut response = permission(&mut f, "wait");
    f.io.take_operations();
    tick(&mut f, &clock, base + Duration::from_secs(cycle + 121));
    assert_no_activity(&mut f, Some(verb));
    assert!(!f.io.take_operations().iter().any(|op| matches!(
        op,
        crate::app::terminal_runtime::chat_terminal::test_io::Operation::Draw(_)
    )));
    if f.app.focus_owner() != crate::app::FocusOwner::Permission {
        crate::app::test_support::press(&mut f.app, crossterm::event::KeyCode::Tab);
    }
    assert_eq!(f.app.focus_owner(), crate::app::FocusOwner::Permission);
    crate::app::test_support::press(&mut f.app, crossterm::event::KeyCode::Enter);
    assert!(response.try_recv().is_ok());
    event(
        &mut f,
        model::SessionUpdate::RuntimeSessionStateUpdate(model::RuntimeSessionState::Running),
    );
    f.settle();
    assert!(contains_tip(&activity(&mut f), HOST_TIPS[2]));
}

#[test]
fn activity_content_recovers_across_widths_and_tip_preferences_without_duplicate_headings() {
    let clock = Clock::at(Instant::now());
    let (mut f, mut commands) = fixture();
    let verb = admit(&mut f, &mut commands);
    event(&mut f, model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking));
    for width in [0, 1, 2, 12, 120, 12, 120] {
        let rows = crate::ui::activity_rows::build_activity_rows(
            &f.app,
            width,
            crate::app::activity::now(),
        );
        assert!(rows.iter().all(|row| row.width() <= usize::from(width)));
        if width == 0 {
            assert!(rows.is_empty());
        }
        if width >= 12 {
            let text: Vec<_> = rows.iter().map(ToString::to_string).collect();
            assert!(contains_tip(&text, HOST_TIPS[0]));
        }
        if width == 120 {
            assert!(rows[0].to_string().contains(verb));
        }
    }
    f.settle();
    assert!(f.live().iter().any(|row| row.contains("Thinking")));
    for enabled in [false, true] {
        setting(&mut f, "spinnerTipsEnabled", enabled);
        f.settle();
        let rows = activity(&mut f);
        assert!(rows[0].contains(verb));
        assert_eq!(contains_tip(&rows, HOST_TIPS[0]), enabled);
        assert!(!rows.iter().any(|row| row.contains("Thinking")));
        assert!(f.io.screen_rows().iter().any(|row| row.contains("UNSENT_DRAFT")));
    }
    f.complete_turn();
    f.settle();
    assert_no_activity(&mut f, Some(verb));
    drop(clock);
}

#[test]
fn compaction_guidance_and_admitted_cancellation_preserve_draft_and_history() {
    let _clock = Clock::at(Instant::now());
    let (mut f, mut commands) = fixture();
    f.app.input.set_text("UNSENT_DRAFT");
    let ids: Vec<_> = f.app.transcript.messages.iter().map(|m| m.id).collect();
    event(&mut f, model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Started));
    f.settle();
    let heading = crate::ui::inline_chat_rows::assistant_role_label_line().to_string();
    let compacting = activity(&mut f);
    assert_eq!(compacting[0].trim_end(), heading.trim_end());
    let rows = compacting.join(" ");
    assert!(rows.contains("Compacting") && rows.contains("drafting") && rows.contains("sending"));
    assert_eq!(f.app.transcript.messages.iter().map(|m| m.id).collect::<Vec<_>>(), ids);
    event(
        &mut f,
        model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Finished {
            result: model::CompactionResult::Success,
            error_code: None,
            error: None,
        }),
    );
    f.settle();
    assert_no_activity(&mut f, None);
    let verb = admit(&mut f, &mut commands);
    event(&mut f, model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking));
    event(&mut f, model::SessionUpdate::CompactionUpdate(model::CompactionUpdate::Started));
    f.settle();
    crate::app::input_submit::request_cancel(&mut f.app).unwrap();
    assert!(
        std::iter::from_fn(|| commands.try_recv().ok())
            .any(|c| matches!(c.command, BridgeCommand::CancelTurn { .. }))
    );
    f.settle();
    let rows = activity(&mut f).join(" ");
    assert!(rows.contains("Cancelling") && !rows.contains("Compacting") && !rows.contains("Tip:"));
    assert!(f.app.turn.compaction.is_active());
    assert!(!f.live().iter().any(|row| row.contains("Thinking")));
    f.complete_turn();
    f.settle();
    assert_no_activity(&mut f, Some(verb));
    assert_eq!(f.app.input.text(), "UNSENT_DRAFT");
    assert!(f.io.transcript().iter().any(|row| row.contains("KEPT_HISTORY")));
}

#[tokio::test]
async fn real_exit_events_remove_activity_and_temporary_thinking_and_allow_valid_restart() {
    for exit in ["error", "disconnect", "replacement"] {
        let _clock = Clock::at(Instant::now());
        let (mut f, mut commands) = fixture();
        let verb = admit(&mut f, &mut commands);
        event(
            &mut f,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        );
        f.settle();
        assert!(f.live().iter().any(|row| row.contains("Thinking")));
        let event = match exit {
            "error" => ClientEvent::TurnErrorClassified {
                class: crate::agent::error_handling::TurnErrorClass::PlanLimit,
                session_id: "test-session".into(),
                message: "EXIT_CAUSE".into(),
                queued_turn_count: Some(0),
                api_error_status: None,
                terminal_reason: None,
            },
            "disconnect" => ClientEvent::ConnectionFailed("EXIT_CAUSE".into()),
            _ => ClientEvent::SessionUpdate {
                session_id: "test-session".into(),
                update: model::SessionUpdate::ConversationReset {
                    new_conversation_id: "replacement".into(),
                    trigger: None,
                    timestamp: None,
                    user_message_uuid: None,
                },
            },
        };
        crate::app::handle_client_event(&mut f.app, event);
        if exit == "replacement" {
            // Conversation reset resets terminal geometry. Reconcile the owned
            // test terminal just as the runtime does before the next draw.
            f.app.chat_render.line_wrap_disabled = true;
            f.resize(120, 45);
        } else {
            f.settle();
        }
        assert_no_activity(&mut f, Some(verb));
        assert!(!f.io.screen_rows().iter().any(|row| row.contains("Thinking")));
        assert_eq!(f.app.input.text(), "UNSENT_DRAFT");
        if exit == "error" {
            assert!(f.io.transcript().iter().any(|row| row.contains("EXIT_CAUSE")));
        }
        if exit != "replacement" {
            assert!(f.io.transcript().iter().any(|row| row.contains("KEPT_HISTORY")));
        }
        if exit != "disconnect" {
            admit(&mut f, &mut commands);
            assert!(!activity(&mut f).is_empty());
        }
    }
}
