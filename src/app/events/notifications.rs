// SPDX-License-Identifier: Apache-2.0

use crate::agent::notifications::SdkNotification;
use crate::app::{App, SystemSeverity};

/// Historical observations retain transcript content and seed delivery identities.
pub(super) fn handle_sdk_notification(app: &mut App, notification: &SdkNotification, replay: bool) {
    if app.session_runtime.session_id.as_ref().map(crate::agent::model::SessionId::as_str)
        != Some(notification.session_id())
    {
        tracing::debug!(
            target: crate::logging::targets::APP_NOTIFY,
            event_name = "notification_session_mismatch",
            outcome = "dropped",
            reason = "session_mismatch",
            notification_session_id = notification.session_id(),
            active_session_id = app.session_runtime.session_id.as_ref().map(crate::agent::model::SessionId::as_str),
            replay,
            "notification provenance does not match the active session"
        );
        return;
    }
    let new = app.notifications.observe_sdk(&app.config, notification, replay);
    if new && let SdkNotification::SdkNotice { priority, text, .. } = notification {
        let severity = if matches!(priority.as_str(), "high" | "immediate") {
            SystemSeverity::Warning
        } else {
            SystemSeverity::Info
        };
        super::notices::emit_system_notice(app, severity, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{events::ClientEvent, model, settings::SettingsSnapshot};
    use crate::app::notify::{NotificationDelivery, NotificationManager, TerminalCapabilities};
    use serde_json::{Value, json};
    use std::cell::RefCell;

    thread_local! { static DELIVERIES: RefCell<Vec<NotificationDelivery>> = const { RefCell::new(Vec::new()) }; }
    fn collect(delivery: NotificationDelivery) {
        DELIVERIES.with(|items| items.borrow_mut().push(delivery));
    }
    fn deliveries() -> Vec<NotificationDelivery> {
        DELIVERIES.with(|items| std::mem::take(&mut *items.borrow_mut()))
    }

    fn notification_logs(action: impl FnOnce()) -> Vec<Value> {
        let mut records = crate::logging::test_capture::capture(action);
        records.retain(|record| record["target"] == crate::logging::targets::APP_NOTIFY);
        records
    }

    fn app() -> App {
        deliveries();
        let mut app = App::test_default();
        app.session_runtime.activate_session("session-1".into());
        app.notifications =
            NotificationManager::with_delivery(collect, TerminalCapabilities::Other);
        set(&mut app, "preferredNotifChannel", json!("terminal_bell"));
        app
    }
    fn set(app: &mut App, id: &str, value: Value) {
        let snapshot = app.config.snapshot.get_or_insert_with(SettingsSnapshot::default);
        snapshot.values.retain(|entry| entry.id != id);
        snapshot.values.extend(SettingsSnapshot::test_value(id, value).values);
    }
    fn update(app: &mut App, update: model::SessionUpdate) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::SessionUpdate { session_id: "session-1".into(), update },
        );
    }
    fn sdk(app: &mut App, notification: &Value, replay: bool) {
        let envelope: crate::agent::wire::EventEnvelope = serde_json::from_value(json!({
            "event": "session_update", "session_id": "session-1", "update": { "type": "notification_update", "notification": notification, "replay": replay }
        })).expect("SDK notification wire event");
        let crate::agent::wire::BridgeEvent::SessionUpdate { update: wire_update, .. } =
            envelope.event
        else {
            panic!("session update")
        };
        let crate::agent::types::SessionUpdate::NotificationUpdate { notification, replay } =
            wire_update
        else {
            panic!("notification update")
        };
        update(app, model::SessionUpdate::NotificationUpdate { notification, replay });
    }
    fn proactive(id: &str) -> Value {
        json!({ "origin": "model_tool", "session_id": "session-1", "tool_use_id": id, "text": "Review is ready", "push_sent": true, "local_sent": false, "disabled_reason": "no_transport", "sent_at": "2026-10-04T10:00:00Z" })
    }
    fn notice(uuid: &str) -> Value {
        json!({ "origin": "sdk_notice", "session_id": "session-1", "uuid": uuid, "key": "replaceable", "text": "Native notice", "priority": "immediate", "color": "yellow", "timeout_ms": 5000 })
    }

    #[test]
    fn live_sdk_delivery_respects_focus_category_identity_and_upstream_reports() {
        let logs = notification_logs(|| {
            let mut app = app();
            let acknowledged = app.config.snapshot.take();
            app.notifications.on_focus_lost();
            sdk(&mut app, &proactive("before-settings"), false);
            assert!(deliveries().is_empty());
            app.config.snapshot = acknowledged;
            app.notifications.on_focus_gained();
            sdk(&mut app, &proactive("default-focused"), false);
            assert!(deliveries().is_empty());
            app.notifications.on_focus_lost();
            sdk(&mut app, &proactive("default-on"), false);
            assert_eq!(deliveries().len(), 1);
            set(&mut app, "notifications.modelDirected", json!(false));
            sdk(&mut app, &proactive("off"), false);
            assert!(deliveries().is_empty());
            set(&mut app, "notifications.modelDirected", json!(true));
            sdk(&mut app, &proactive("off"), false); // Suppression consumes the old event.
            assert!(deliveries().is_empty());
            sdk(&mut app, &proactive("live"), false);
            sdk(&mut app, &proactive("live"), false);
            let sent = deliveries();
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].body, "Review is ready");
            assert!(sent[0].ring_bell && !sent[0].send_desktop);
            for (id, field, value) in [
                ("already-local", "local_sent", json!(true)),
                ("native-off", "disabled_reason", json!("config_off")),
                ("user-present", "disabled_reason", json!("user_present")),
                ("future-suppression", "disabled_reason", json!("future_reason")),
                ("missing-local-report", "local_sent", Value::Null),
                ("stale-session", "session_id", json!("old-session")),
            ] {
                let mut payload = proactive(id);
                payload[field] = value;
                sdk(&mut app, &payload, false);
            }
            assert!(deliveries().is_empty());
            app.notifications.on_focus_gained();
            sdk(&mut app, &proactive("focused"), false);
            app.notifications.on_focus_lost();
            sdk(&mut app, &proactive("focused"), false);
            assert!(deliveries().is_empty());
            set(&mut app, "preferredNotifChannel", json!("notifications_disabled"));
            sdk(&mut app, &proactive("disabled-transport"), false);
            assert!(deliveries().is_empty());
        });
        for reason in [
            "settings_unavailable",
            "terminal_focused",
            "category_disabled",
            "duplicate",
            "upstream_local_delivery",
            "sdk_suppressed",
            "missing_local_report",
            "session_mismatch",
            "notifications_disabled",
        ] {
            assert!(logs.iter().any(|record| record["reason"] == reason), "missing {reason}");
        }
        let delivered = logs
            .iter()
            .find(|record| {
                record["event_name"] == "notification_delivery_decided"
                    && record["outcome"] == "dispatch"
            })
            .expect("delivery decision");
        assert_eq!(delivered["span"]["session_id"], "session-1");
        assert_eq!(delivered["span"]["tool_call_id"], "default-on");
        assert_eq!(delivered["notification_method"], "TerminalBell");
        assert_eq!(delivered["ring_bell"], true);
        assert!(!serde_json::to_string(&logs).expect("diagnostics").contains("Review is ready"));
    }

    #[test]
    fn history_reconnect_and_sdk_replay_preserve_transcript_without_resending_alerts() {
        let mut app = app();
        app.notifications.on_focus_lost();
        let history = vec![
            model::SessionUpdate::NotificationUpdate {
                notification: serde_json::from_value(notice("history-notice")).expect("notice"),
                replay: false,
            },
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("old-tool", "Old notification")
                    .status(model::ToolCallStatus::Completed),
            ),
        ];
        super::super::session_reset::load_resume_history(&mut app, &history);
        sdk(&mut app, &proactive("old-tool"), false);
        sdk(&mut app, &proactive("sdk-replay"), true);
        sdk(&mut app, &proactive("sdk-replay"), false);
        assert!(deliveries().is_empty());
        sdk(&mut app, &notice("history-notice"), false);
        sdk(&mut app, &notice("new-notice"), false);
        sdk(&mut app, &notice("new-notice"), false);
        let text = |app: &App| {
            app.transcript
                .messages
                .iter()
                .flat_map(|message| &message.blocks)
                .filter_map(|block| match block {
                    crate::app::MessageBlock::Notice(notice) => Some(notice.text.text.as_str()),
                    _ => None,
                })
                .filter(|text| *text == "Native notice")
                .count()
        };
        assert_eq!(text(&app), 2); // Reused native key is a new notice with a new UUID.
        super::super::session_reset::load_resume_history(&mut app, &history);
        assert_eq!(text(&app), 1); // Reconnect rebuilds historical visibility.
        sdk(&mut app, &proactive("new-live-tool"), false);
        assert_eq!(deliveries().len(), 1);
    }

    #[test]
    fn waiting_questions_notify_once_and_respect_the_actions_category() {
        let logs = notification_logs(|| {
            let mut app = app();
            app.notifications.on_focus_lost();
            for (id, enabled) in [
                ("question-loading", None),
                ("question-off", Some(false)),
                ("question-on", Some(true)),
            ] {
                if let Some(enabled) = enabled {
                    set(&mut app, "notifications.actionsRequired", json!(enabled));
                } else {
                    app.config.snapshot = None;
                }
                let should_notify = enabled == Some(true);
                update(
                    &mut app,
                    model::SessionUpdate::ToolCall(model::ToolCall::new(id, "AskUserQuestion")),
                );
                for _ in 0..2 {
                    let (tx, _rx) = tokio::sync::oneshot::channel();
                    let request = model::RequestQuestionRequest::new(
                        "session-1",
                        model::ToolCallUpdate::new(id, model::ToolCallUpdateFields::new()),
                        model::QuestionPrompt::new(
                            "Proceed?",
                            "Choice",
                            false,
                            vec![model::QuestionOption::new("yes", "Yes")],
                        ),
                        0,
                        1,
                    );
                    crate::app::events::handle_client_event(
                        &mut app,
                        ClientEvent::QuestionRequest {
                            session_id: "session-1".into(),
                            request,
                            response_tx: tx,
                        },
                    );
                }
                let sent = deliveries();
                assert_eq!(sent.len(), usize::from(should_notify));
                if should_notify {
                    assert!(sent[0].body.contains("Question required"));
                }
                crate::app::events::handle_client_event(
                    &mut app,
                    ClientEvent::InteractionCancelled {
                        session_id: "session-1".into(),
                        interaction_id: id.into(),
                    },
                );
                assert!(app.turn.pending_interaction_ids.is_empty());
            }
        });
        let decisions: Vec<_> = logs
            .iter()
            .filter(|record| record["event_name"] == "notification_delivery_decided")
            .collect();
        assert_eq!(decisions.len(), 3);
        for (record, id, reason) in [
            (decisions[0], "question-loading", "settings_unavailable"),
            (decisions[1], "question-off", "category_disabled"),
            (decisions[2], "question-on", "transport_selected"),
        ] {
            assert_eq!(record["span"]["session_id"], "session-1");
            assert_eq!(record["span"]["interaction_id"], id);
            assert_eq!(record["category"], "QuestionRequired");
            assert_eq!(record["reason"], reason);
        }
        assert_eq!(decisions[2]["outcome"], "dispatch");
        assert!(!serde_json::to_string(&logs).expect("diagnostics").contains("Proceed?"));
    }

    #[test]
    fn actual_waiting_interactions_and_successful_completion_use_independent_categories() {
        let mut app = app();
        app.notifications.on_focus_lost();
        update(
            &mut app,
            model::SessionUpdate::ToolCall(model::ToolCall::new("permission-1", "Read a file")),
        );
        let permission = || {
            model::RequestPermissionRequest::new(
                "session-1",
                model::ToolCallUpdate::new("permission-1", model::ToolCallUpdateFields::new()),
                vec![model::PermissionOption::new(
                    "allow",
                    "Allow",
                    model::PermissionOptionKind::AllowOnce,
                )],
                None,
            )
        };
        for _ in 0..2 {
            let (tx, _rx) = tokio::sync::oneshot::channel();
            crate::app::events::handle_client_event(
                &mut app,
                ClientEvent::PermissionRequest {
                    session_id: "session-1".into(),
                    request: permission(),
                    response_tx: tx,
                },
            );
        }
        assert_eq!(deliveries().len(), 1);
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::InteractionCancelled {
                session_id: "session-1".into(),
                interaction_id: "permission-1".into(),
            },
        );
        assert!(app.turn.pending_interaction_ids.is_empty());
        set(&mut app, "notifications.actionsRequired", json!(false));
        let (tx, _rx) = tokio::sync::oneshot::channel();
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::PermissionRequest {
                session_id: "session-1".into(),
                request: permission(),
                response_tx: tx,
            },
        );
        assert!(deliveries().is_empty());
        let complete = || ClientEvent::TurnComplete {
            session_id: "session-1".into(),
            queued_turn_count: None,
            terminal_reason: None,
        };
        app.status = crate::app::AppStatus::Running;
        crate::app::events::handle_client_event(&mut app, complete());
        crate::app::events::handle_client_event(&mut app, complete());
        assert_eq!(deliveries().len(), 1);
        set(&mut app, "notifications.turnComplete", json!(false));
        app.status = crate::app::AppStatus::Running;
        crate::app::events::handle_client_event(&mut app, complete());
        assert!(deliveries().is_empty());
        set(&mut app, "notifications.turnComplete", json!(true));
        app.status = crate::app::AppStatus::Running;
        app.turn.cancel_requested = true;
        crate::app::events::handle_client_event(&mut app, complete());
        assert!(deliveries().is_empty());
        app.status = crate::app::AppStatus::Running;
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: "session-1".into(),
                queued_turn_count: None,
                terminal_reason: Some(crate::agent::types::TerminalReason::AbortedStreaming),
            },
        );
        assert!(deliveries().is_empty());
    }
}
