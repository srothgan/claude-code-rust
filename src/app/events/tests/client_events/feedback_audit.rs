// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{client::CommandReceiver, types::ReloadOutcome, wire::BridgeCommand};
use pretty_assertions::assert_eq;
use serde_json::json;

fn submit(app: &mut App, text: &str) {
    app.input.set_text(text);
    crate::app::input_submit::submit_input(app);
}

async fn received(commands: &mut CommandReceiver) -> BridgeCommand {
    tokio::time::timeout(Duration::from_secs(2), commands.recv_envelope())
        .await
        .expect("command deadline")
        .expect("command receiver")
        .command
}

fn no_command(commands: &mut CommandReceiver) {
    assert!(matches!(commands.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
}

fn prompt(app: &mut App, commands: &mut CommandReceiver, text: &str) {
    submit(app, text);
    let BridgeCommand::Prompt { session_id, chunks, .. } =
        commands.try_recv().expect("usable Prompt").command
    else {
        panic!("Prompt")
    };
    assert_eq!(session_id, "test-session");
    assert!(
        matches!(chunks.as_slice(), [chunk] if chunk.kind == "text" && chunk.value == json!(text))
    );
    crate::app::test_support::drain_session_refreshes(commands, "test-session");
    assert!(app.input.is_empty());
}

fn region(app: &mut App, message_index: usize, block_index: Option<usize>) -> String {
    let rows = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
        app,
        160,
        &std::collections::BTreeSet::new(),
        None,
    );
    let segment = rows
        .segments()
        .iter()
        .find(|s| s.msg_idx == message_index && s.block_idx == block_index)
        .expect("target message/notice region");
    rows.segment_rows(segment)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn error_notice(app: &mut App, cause: &str, expected_owner: Option<crate::app::ChatMessageId>) {
    let matching = app
        .transcript
        .messages
        .iter()
        .enumerate()
        .flat_map(|(mi, m)| {
            m.blocks.iter().enumerate().filter_map(move |(bi, b)| match b {
                MessageBlock::Notice(n) if n.text.text == cause => {
                    Some((mi, bi, m.id, m.role.clone(), n.severity))
                }
                _ => None,
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1, "one notice for the supplied incident");
    let (mi, bi, owner, role, severity) = &matching[0];
    assert_eq!(*severity, SystemSeverity::Error);
    let boundary = if let Some(expected) = expected_owner {
        assert_eq!(*owner, expected);
        assert_eq!(*role, MessageRole::Assistant);
        Some(*bi)
    } else {
        assert_eq!(*role, MessageRole::System(Some(SystemSeverity::Error)));
        None
    };
    assert!(
        region(app, *mi, boundary).contains(cause),
        "cause must be visible at its owning region"
    );
}

#[test]
fn service_warning_and_error_preserve_drafts_and_deliver_severity_before_usable_submission() {
    for (severity, expected, cause) in [
        (
            ServiceStatusSeverity::Warning,
            SystemSeverity::Warning,
            "fixture service capacity reduced",
        ),
        (ServiceStatusSeverity::Error, SystemSeverity::Error, "fixture service unavailable"),
    ] {
        let (mut app, mut commands) = app_with_bridge_connection();
        app.input.set_text("user draft survives service status");
        handle_client_event(
            &mut app,
            ClientEvent::ServiceStatus { severity, message: cause.into() },
        );
        assert_eq!(app.input.text(), "user draft survives service status");
        assert_eq!(app.status, AppStatus::Ready);
        let index = app
            .transcript
            .messages
            .iter()
            .position(|m| {
                m.role == MessageRole::System(Some(expected))
                    && m.blocks
                        .iter()
                        .any(|b| matches!(b, MessageBlock::Text(t) if t.text == cause))
            })
            .expect("diagnostic and severity");
        assert!(region(&mut app, index, None).contains(cause));
        no_command(&mut commands);
        prompt(&mut app, &mut commands, "user draft survives service status");
    }
}

async fn begin_pending(app: &mut App, commands: &mut CommandReceiver, kind: &str) {
    match kind {
        "model" => {
            submit(app, "/model requested-model");
            assert_eq!(
                received(commands).await,
                BridgeCommand::SetModel {
                    session_id: "test-session".into(),
                    model: "requested-model".into()
                }
            );
        }
        "reload" => {
            submit(app, "/reload");
            assert_eq!(
                received(commands).await,
                BridgeCommand::ReloadSkillsAndStyles { session_id: "test-session".into() }
            );
        }
        "resume" => {
            handle_client_event(
                app,
                ClientEvent::SessionsListed {
                    sessions: vec![listed_session("resume-target", "Target title")],
                },
            );
            submit(app, "/resume");
            handle_terminal_event(
                app,
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            );
            assert!(
                matches!(received(commands).await, BridgeCommand::ResumeSession { session_id, .. } if session_id == "resume-target")
            );
            assert_eq!(app.pending_session_resume_id(), Some("resume-target"));
        }
        _ => unreachable!(),
    }
    assert_eq!(app.status, AppStatus::CommandPending);
    no_command(commands);
}

#[tokio::test(flavor = "current_thread")]
async fn idle_and_pending_slash_errors_show_the_cause_and_release_only_the_failed_submission() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            for kind in ["idle", "model", "reload", "resume"] {
                let (mut app, mut commands) = app_with_bridge_connection();
                if kind != "idle" {
                    begin_pending(&mut app, &mut commands, kind).await;
                }
                app.input.set_text("recoverable draft");
                if kind != "idle" {
                    crate::app::input_submit::submit_input(&mut app);
                    no_command(&mut commands);
                }
                let cause = format!("fixture {kind} request rejected");
                handle_client_event(
                    &mut app,
                    ClientEvent::SlashCommandError { session_id: None, message: cause.clone() },
                );
                assert_eq!(app.status, AppStatus::Ready);
                assert!(app.pending_session_resume.is_none());
                assert_eq!(app.input.text(), "recoverable draft");
                error_notice(&mut app, &cause, None);
                prompt(&mut app, &mut commands, "recoverable draft");
            }
        }))
        .await;
}

#[test]
fn active_slash_errors_stay_on_the_current_running_or_thinking_response_and_do_not_end_it() {
    for thinking in [false, true] {
        let (mut app, mut commands) = app_with_bridge_connection();
        app.push_message_tracked(assistant_msg(vec![MessageBlock::Text(
            TextBlock::from_complete("older finished response"),
        )]));
        let previous = app.transcript.messages[0].id;
        prompt(&mut app, &mut commands, "original request");
        if thinking {
            handle_client_event(
                &mut app,
                session_update(model::SessionUpdate::AgentActivityUpdate(
                    model::AgentActivityPhase::Thinking,
                )),
            );
        } else {
            handle_client_event(
                &mut app,
                session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                    model::ContentBlock::Text(model::TextContent::new("streaming answer")),
                ))),
            );
        }
        let expected_status = if thinking { AppStatus::Thinking } else { AppStatus::Running };
        assert_eq!(app.status, expected_status);
        let owner_index = app.active_turn_assistant_idx().expect("admitted response owner");
        let owner = app.transcript.messages[owner_index].id;
        app.input.set_text("next draft");
        handle_client_event(
            &mut app,
            ClientEvent::SlashCommandError {
                session_id: Some("test-session".into()),
                message: "fixture side command failed".into(),
            },
        );
        assert_eq!(app.status, expected_status);
        assert_eq!(app.input.text(), "next draft");
        error_notice(&mut app, "fixture side command failed", Some(owner));
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new(" continued response")),
            ))),
        );
        assert_eq!(app.transcript.messages[app.active_turn_assistant_idx().unwrap()].id, owner);
        let old = app.transcript.messages.iter().find(|m| m.id == previous).unwrap();
        assert!(
            matches!(old.blocks.as_slice(), [MessageBlock::Text(t)] if t.text == "older finished response")
        );
        let current = app.transcript.messages.iter().find(|m| m.id == owner).unwrap();
        let answer = current
            .blocks
            .iter()
            .filter_map(|block| match block {
                MessageBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(
            answer,
            if thinking { " continued response" } else { "streaming answer continued response" }
        );
        handle_client_event(&mut app, turn_complete(None));
        assert_eq!(app.status, AppStatus::Ready);
        prompt(&mut app, &mut commands, "next draft");
    }
}

fn half<'a>(text: &'a str, label: &str, other: &str) -> &'a str {
    let start = text.find(label).expect("reload half label") + label.len();
    let end = text[start..].find(other).map_or(text.len(), |offset| start + offset);
    &text[start..end]
}

fn reloaded_outcome(names: &[&str]) -> ReloadOutcome {
    ReloadOutcome::Reloaded { names: names.iter().map(|name| (*name).into()).collect() }
}

type ReloadRow = (
    ReloadOutcome,
    ReloadOutcome,
    SystemSeverity,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
);

fn reload_cases() -> [ReloadRow; 6] {
    [
        (
            reloaded_outcome(&["deploy", "lint"]),
            reloaded_outcome(&["default", "clear", "mine"]),
            SystemSeverity::Info,
            Some("2"),
            Some("3"),
            None,
            None,
            None,
        ),
        (
            ReloadOutcome::TimedOut { after_ms: 10000 },
            reloaded_outcome(&["default", "clear", "mine"]),
            SystemSeverity::Warning,
            Some("10"),
            Some("3"),
            None,
            None,
            Some("skills"),
        ),
        (
            reloaded_outcome(&["deploy", "lint"]),
            ReloadOutcome::TimedOut { after_ms: 10000 },
            SystemSeverity::Warning,
            Some("2"),
            Some("10"),
            None,
            None,
            Some("output styles"),
        ),
        (
            reloaded_outcome(&["deploy", "lint"]),
            ReloadOutcome::Failed { message: "style directory unreadable".into() },
            SystemSeverity::Warning,
            Some("2"),
            None,
            None,
            Some("style directory unreadable"),
            None,
        ),
        (
            ReloadOutcome::Failed { message: "skill query closed".into() },
            reloaded_outcome(&["default", "clear", "mine"]),
            SystemSeverity::Warning,
            None,
            Some("3"),
            Some("skill query closed"),
            None,
            None,
        ),
        (
            ReloadOutcome::Failed { message: "query closed".into() },
            ReloadOutcome::TimedOut { after_ms: 10000 },
            SystemSeverity::Error,
            None,
            Some("10"),
            Some("query closed"),
            None,
            Some("output styles"),
        ),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn reload_feedback_associates_each_half_count_cause_and_uncertainty_with_its_severity() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            for (
                skills,
                styles,
                severity,
                skill_number,
                style_number,
                skill_cause,
                style_cause,
                uncertain,
            ) in reload_cases()
            {
                let (mut app, mut commands) = app_with_bridge_connection();
                begin_pending(&mut app, &mut commands, "reload").await;
                let before =
                    app.transcript.messages.iter().map(|message| message.id).collect::<Vec<_>>();
                handle_client_event(
                    &mut app,
                    ClientEvent::SkillsAndStylesReloaded {
                        session_id: "test-session".into(),
                        skills,
                        output_styles: styles,
                    },
                );
                assert_eq!(app.status, AppStatus::Ready);
                let result_notices = app
                    .transcript
                    .messages
                    .iter()
                    .filter(|message| !before.contains(&message.id))
                    .flat_map(|message| &message.blocks)
                    .filter(|block| matches!(block, MessageBlock::Notice(_)))
                    .count();
                assert_eq!(result_notices, 1, "one result notice per reload request");
                let message = app.transcript.messages.last().expect("reload result");
                assert_eq!(message.role, MessageRole::System(Some(severity)));
                let MessageBlock::Notice(notice) = &message.blocks[0] else {
                    panic!("reload notice")
                };
                assert_eq!(notice.severity, severity);
                let index = app.transcript.messages.len() - 1;
                let text = region(&mut app, index, None)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                assert!(text.contains("skills") && text.contains("output styles"));
                if skill_number == Some("2") || style_number == Some("3") {
                    assert!(text.contains("reloaded"));
                }
                for (label, other, number, cause) in [
                    ("skills", "output styles", skill_number, skill_cause),
                    ("output styles", "skills", style_number, style_cause),
                ] {
                    let details = half(&text, label, other);
                    if let Some(number) = number {
                        assert_eq!(
                            details.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()),
                            Some(number),
                            "{label}: {details}"
                        );
                    }
                    if let Some(cause) = cause {
                        assert!(details.contains(cause), "{label}: {details}");
                    }
                }
                if let Some(label) = uncertain {
                    let details = half(
                        &text,
                        label,
                        if label == "skills" { "output styles" } else { "skills" },
                    );
                    assert!(
                        details.contains("may") && details.contains("still"),
                        "unanswered is uncertain: {details}"
                    );
                    if skill_cause.is_none() && style_cause.is_none() {
                        assert!(!text.contains("fail"));
                    }
                }
                prompt(&mut app, &mut commands, "after reload feedback");
            }
        }))
        .await;
}
