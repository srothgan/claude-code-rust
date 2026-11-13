// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::{Value, json};

fn decode(raw: Value) -> EventEnvelope {
    serde_json::from_value(raw).expect("bridge-shaped event")
}

#[test]
fn tool_result_payloads_preserve_source_bytes_and_detached_metadata() {
    let source = "# Notes\r\n\r\n\t**literal**  \r\n";
    let cases = [
        (
            "read-1",
            json!({"status":"completed","raw_output":source,
            "content":[{"type":"content","content":{"type":"text","text":source}}],
            "output_metadata":{"read":{"file_path":"notes.md","start_line":60}}}),
        ),
        (
            "detached-2",
            json!({"status":"detached","output_metadata":{"structured_content_omitted":true}}),
        ),
    ];
    for (id, fields) in cases {
        let envelope = decode(json!({"event":"session_update","session_id":"tool-session",
            "update":{"type":"tool_call_update","tool_call_update":{"tool_call_id":id,"fields":fields}}}));
        let BridgeEvent::SessionUpdate {
            session_id,
            update: types::SessionUpdate::ToolCallUpdate { tool_call_update },
        } = envelope.event
        else {
            panic!("tool result")
        };
        assert_eq!(session_id, "tool-session");
        assert_eq!(tool_call_update.tool_call_id, id);
        let fields = tool_call_update.fields;
        if id == "read-1" {
            assert_eq!(fields.status.as_deref(), Some("completed"));
            assert_eq!(fields.raw_output.as_deref(), Some(source));
            assert_eq!(
                serde_json::to_value(fields.content).unwrap(),
                json!([{"type":"content","content":{"type":"text","text":source}}])
            );
            assert_eq!(
                fields.output_metadata.unwrap().read,
                Some(types::ReadOutputMetadata { file_path: "notes.md".into(), start_line: 60 })
            );
        } else {
            assert_eq!(fields.status.as_deref(), Some("detached"));
            assert!(fields.output_metadata.unwrap().structured_content_omitted);
        }
    }
}

#[test]
fn startup_failures_preserve_known_future_and_absent_guidance() {
    for (metadata, reason, errors) in [
        (None, None, vec![]),
        (
            Some(json!({"reason":"provider_not_allowed","errors":["Choose an allowed provider"]})),
            Some("provider_not_allowed"),
            vec!["Choose an allowed provider"],
        ),
        (Some(json!({"reason":"future_reason"})), Some("future_reason"), vec![]),
    ] {
        let mut raw = json!({"event":"connection_failed","message":"startup failed","request_id":"connect-operation"});
        if let Some(metadata) = metadata {
            raw["startup_failure"] = metadata;
        }
        let envelope = decode(raw);
        assert_eq!(envelope.request_id.as_deref(), Some("connect-operation"));
        let BridgeEvent::ConnectionFailed { message, startup_failure } = envelope.event else {
            panic!("connection failure")
        };
        assert_eq!(message, "startup failed");
        assert_eq!(startup_failure.as_ref().map(|f| f.reason.as_str()), reason);
        if let Some(failure) = startup_failure {
            assert_eq!(failure.errors, errors);
        }
    }
}

fn state_event(kind: &str, state: Option<Value>) -> Value {
    let mut raw = if kind == "session_update" {
        json!({"request_id":"state-operation","event":"session_update","session_id":"state-session",
            "update":{"type":"ultracode_update"}})
    } else {
        json!({"request_id":"state-operation","event":kind,"session_id":"state-session","cwd":"/state-project",
            "current_model":{"requested_id":null,"resolved_id":"opus","display_name_short":"Opus",
                "display_name_long":"Claude Opus","catalog_id":null,"supports_effort":false,
                "supported_effort_levels":[],"supports_fast_mode":false,"supports_auto_mode":false,
                "supports_adaptive_thinking":false,"is_authoritative":true},
            "fast_mode_state":"off","history_updates":[]})
    };
    if kind == "session_replaced" {
        raw["restored_input"] = json!("Recovered draft");
    }
    if let Some(state) = state {
        if kind == "session_update" {
            raw["update"]["ultracode"] = state;
        } else {
            raw["ultracode"] = state;
        }
    }
    raw
}

#[test]
fn ultracode_snapshots_preserve_values_or_unknown_at_each_event_boundary() {
    let cases = [
        (
            "unavailable",
            Some(json!({"available":false,"requested":false,"effective":false})),
            Some((false, false, false)),
        ),
        (
            "requested unavailable",
            Some(json!({"available":false,"requested":true,"effective":false})),
            Some((false, true, false)),
        ),
        (
            "available off",
            Some(json!({"available":true,"requested":false,"effective":false})),
            Some((true, false, false)),
        ),
        (
            "enabled",
            Some(json!({"available":true,"requested":true,"effective":true})),
            Some((true, true, true)),
        ),
        (
            "inconsistent unavailable off",
            Some(json!({"available":false,"requested":false,"effective":true})),
            None,
        ),
        (
            "inconsistent unavailable requested",
            Some(json!({"available":false,"requested":true,"effective":true})),
            None,
        ),
        (
            "inconsistent unrequested",
            Some(json!({"available":true,"requested":false,"effective":true})),
            None,
        ),
        (
            "inconsistent enabled",
            Some(json!({"available":true,"requested":true,"effective":false})),
            None,
        ),
        ("missing field", Some(json!({"available":true,"requested":true})), None),
        ("wrong type", Some(json!({"available":true,"requested":true,"effective":"true"})), None),
        ("null", Some(Value::Null), None),
        ("omitted", None, None),
    ];
    for (label, state, expected) in cases {
        for kind in ["session_update", "connected", "session_replaced"] {
            let envelope = decode(state_event(kind, state.clone()));
            assert_eq!(envelope.request_id.as_deref(), Some("state-operation"));
            let (session_id, actual) = match envelope.event {
                BridgeEvent::SessionUpdate {
                    session_id,
                    update: types::SessionUpdate::UltracodeUpdate { ultracode },
                } => {
                    assert_eq!(kind, "session_update");
                    (session_id, ultracode)
                }
                BridgeEvent::Connected { session_id, cwd, current_model, ultracode, .. } => {
                    assert_eq!(kind, "connected");
                    assert_eq!(cwd, "/state-project");
                    assert_eq!(current_model.resolved_id, "opus");
                    (session_id, ultracode)
                }
                BridgeEvent::SessionReplaced {
                    session_id,
                    cwd,
                    current_model,
                    ultracode,
                    restored_input,
                    ..
                } => {
                    assert_eq!(kind, "session_replaced");
                    assert_eq!(cwd, "/state-project");
                    assert_eq!(current_model.resolved_id, "opus");
                    assert_eq!(restored_input.as_deref(), Some("Recovered draft"));
                    (session_id, ultracode)
                }
                other => panic!("lost containing event: {other:?}"),
            };
            assert_eq!(session_id, "state-session");
            assert_eq!(
                actual.map(|s| (s.available(), s.requested(), s.effective())),
                expected,
                "{label}: {kind}"
            );
        }
    }
}

#[test]
fn ultracode_commands_preserve_enable_disable_and_refresh_payloads() {
    for (request_id, command, expected) in [
        (
            Some("enable-operation"),
            BridgeCommand::SetUltracode { session_id: "on-session".into(), enabled: true },
            json!({"request_id":"enable-operation","command":"set_ultracode","session_id":"on-session","enabled":true}),
        ),
        (
            Some("disable-operation"),
            BridgeCommand::SetUltracode { session_id: "off-session".into(), enabled: false },
            json!({"request_id":"disable-operation","command":"set_ultracode","session_id":"off-session","enabled":false}),
        ),
        (
            None,
            BridgeCommand::RefreshUltracode { session_id: "refresh-session".into() },
            json!({"command":"refresh_ultracode","session_id":"refresh-session"}),
        ),
    ] {
        assert_eq!(
            serde_json::to_value(CommandEnvelope {
                request_id: request_id.map(str::to_owned),
                command
            })
            .unwrap(),
            expected
        );
    }
}

#[test]
fn side_question_results_preserve_fallback_content_and_identity() {
    for supplied in [false, true] {
        let mut metadata = json!({"synthetic":supplied});
        if supplied {
            metadata["refusal_fallback"] = json!({"original_model":"opus","fallback_model":"sonnet","content":{"reason":"policy"}});
        }
        let decoded = decode(
            json!({"event":"btw_result","session_id":"question-session","btw_id":"question-17",
            "question":"Question?","answer":"Answer.","metadata":metadata}),
        );
        let BridgeEvent::BtwResult { session_id, btw_id, question, answer, metadata } =
            decoded.event
        else {
            panic!("BTW result")
        };
        assert_eq!(session_id, "question-session");
        assert_eq!(btw_id, "question-17");
        assert_eq!(question, "Question?");
        assert_eq!(answer, "Answer.");
        assert_eq!(metadata.synthetic, supplied);
        assert_eq!(
            metadata.refusal_fallback,
            if supplied {
                Some(
                    json!({"original_model":"opus","fallback_model":"sonnet","content":{"reason":"policy"}}),
                )
            } else {
                None
            }
        );
    }
}

#[test]
fn message_receipts_and_activity_preserve_distinct_envelope_payloads() {
    use crate::agent::model::AgentActivityPhase;
    for (raw, expected) in [
        (
            json!({"event":"user_message_queued","session_id":"queue-session","message_uuid":"queued-user"}),
            BridgeEvent::UserMessageQueued {
                session_id: "queue-session".into(),
                message_uuid: "queued-user".into(),
            },
        ),
        (
            json!({"event":"user_message_started","session_id":"start-session","message_uuid":"started-user","source":"command_lifecycle"}),
            BridgeEvent::UserMessageStarted {
                session_id: "start-session".into(),
                message_uuid: "started-user".into(),
                source: types::UserMessageStartSource::CommandLifecycle,
            },
        ),
        (
            json!({"event":"turn_interrupt_receipt","session_id":"interrupt-session","still_queued":["later-user","last-user"]}),
            BridgeEvent::TurnInterruptReceipt {
                session_id: "interrupt-session".into(),
                still_queued: vec!["later-user".into(), "last-user".into()],
            },
        ),
        (
            json!({"event":"session_update","session_id":"autonomous-session","update":{"type":"agent_response_started"}}),
            BridgeEvent::SessionUpdate {
                session_id: "autonomous-session".into(),
                update: types::SessionUpdate::AgentResponseStarted,
            },
        ),
        (
            json!({"event":"session_update","session_id":"working-session","update":{"type":"agent_activity_update","phase":"working"}}),
            BridgeEvent::SessionUpdate {
                session_id: "working-session".into(),
                update: types::SessionUpdate::AgentActivityUpdate {
                    phase: AgentActivityPhase::Working,
                },
            },
        ),
        (
            json!({"event":"session_update","session_id":"thinking-session","update":{"type":"agent_activity_update","phase":"thinking"}}),
            BridgeEvent::SessionUpdate {
                session_id: "thinking-session".into(),
                update: types::SessionUpdate::AgentActivityUpdate {
                    phase: AgentActivityPhase::Thinking,
                },
            },
        ),
    ] {
        let envelope = decode(raw);
        assert_eq!(envelope.request_id, None);
        assert_eq!(envelope.event, expected);
    }
}

#[test]
fn terminal_results_preserve_queue_counts_reasons_and_error_metadata() {
    for (count, expected) in [(Some(json!(2)), Some(2)), (Some(json!(0)), Some(0)), (None, None)] {
        let mut raw = json!({"request_id":"complete-op","event":"turn_complete","session_id":"complete-session","terminal_reason":"completed"});
        if let Some(count) = count {
            raw["queued_turn_count"] = count;
        }
        let envelope = decode(raw);
        assert_eq!(envelope.request_id.as_deref(), Some("complete-op"));
        assert_eq!(
            envelope.event,
            BridgeEvent::TurnComplete {
                session_id: "complete-session".into(),
                queued_turn_count: expected,
                terminal_reason: Some(types::TerminalReason::Completed)
            }
        );
    }
    for (status, expected_status, count, expected_count) in [
        (Some(json!(429)), Some(429), Some(json!(1)), Some(1)),
        (Some(json!(529)), Some(529), Some(json!(0)), Some(0)),
        (Some(Value::Null), None, None, None),
        (None, None, None, None),
    ] {
        let mut raw = json!({"request_id":"error-op","event":"turn_error","session_id":"error-session",
            "message":"service overloaded","error_kind":"transient_service","sdk_result_subtype":"error_during_execution",
            "assistant_error":"overloaded_error","terminal_reason":"api_error"});
        if let Some(status) = status {
            raw["api_error_status"] = status;
        }
        if let Some(count) = count {
            raw["queued_turn_count"] = count;
        }
        let envelope = decode(raw);
        assert_eq!(envelope.request_id.as_deref(), Some("error-op"));
        assert_eq!(
            envelope.event,
            BridgeEvent::TurnError {
                session_id: "error-session".into(),
                message: "service overloaded".into(),
                queued_turn_count: expected_count,
                error_kind: Some("transient_service".into()),
                sdk_result_subtype: Some("error_during_execution".into()),
                assistant_error: Some("overloaded_error".into()),
                api_error_status: expected_status,
                terminal_reason: Some(types::TerminalReason::ApiError)
            }
        );
    }
}

#[test]
fn reload_envelopes_preserve_partial_outcomes_and_queue_expiry() {
    for (mut raw, expected) in [
        (
            json!({"event":"skills_and_styles_reloaded","session_id":"reload-session","skills":{"status":"reloaded","names":["deploy","lint"]},"output_styles":{"status":"timed_out","after_ms":10000}}),
            BridgeEvent::SkillsAndStylesReloaded {
                session_id: "reload-session".into(),
                skills: types::ReloadOutcome::Reloaded {
                    names: vec!["deploy".into(), "lint".into()],
                },
                output_styles: types::ReloadOutcome::TimedOut { after_ms: 10000 },
            },
        ),
        (
            json!({"event":"skills_and_styles_reloaded","session_id":"partial-session","skills":{"status":"reloaded","names":["release"]},"output_styles":{"status":"failed","message":"style directory unreadable"}}),
            BridgeEvent::SkillsAndStylesReloaded {
                session_id: "partial-session".into(),
                skills: types::ReloadOutcome::Reloaded { names: vec!["release".into()] },
                output_styles: types::ReloadOutcome::Failed {
                    message: "style directory unreadable".into(),
                },
            },
        ),
        (
            json!({"event":"skills_and_styles_reload_expired","session_id":"expired-session","after_ms":2345}),
            BridgeEvent::SkillsAndStylesReloadExpired {
                session_id: "expired-session".into(),
                after_ms: 2345,
            },
        ),
    ] {
        raw["request_id"] = json!("reload-operation");
        let envelope = decode(raw);
        assert_eq!(envelope.request_id.as_deref(), Some("reload-operation"));
        assert_eq!(envelope.event, expected);
    }
}

#[test]
fn history_export_envelopes_preserve_source_chunks_and_failure_cause() {
    for (last, omissions) in [(false, 0), (true, 3)] {
        let envelope = decode(
            json!({"request_id":"export-operation","event":"session_history_export","session_id":"export-session",
            "updates":[{"type":"user_message_chunk","content":{"type":"text","text":"First line\nSecond line"},"source_message_uuid":"source-user"}],
            "last":last,"omitted_updates":omissions}),
        );
        assert_eq!(envelope.request_id.as_deref(), Some("export-operation"));
        let BridgeEvent::SessionHistoryExport {
            session_id,
            updates,
            last: actual_last,
            omitted_updates,
            error,
        } = envelope.event
        else {
            panic!("export chunk")
        };
        assert_eq!(session_id, "export-session");
        assert_eq!(
            serde_json::to_value(updates).unwrap(),
            json!([{"type":"user_message_chunk","content":{"type":"text","text":"First line\nSecond line"},"source_message_uuid":"source-user"}])
        );
        assert_eq!(actual_last, last);
        assert_eq!(omitted_updates, omissions);
        assert_eq!(error, None);
    }
    let envelope = decode(
        json!({"request_id":"failed-export-operation","event":"session_history_export","session_id":"failed-export-session",
        "updates":[],"last":true,"omitted_updates":0,"error":"failed to load session history: boom"}),
    );
    assert_eq!(envelope.request_id.as_deref(), Some("failed-export-operation"));
    assert_eq!(
        envelope.event,
        BridgeEvent::SessionHistoryExport {
            session_id: "failed-export-session".into(),
            updates: vec![],
            last: true,
            omitted_updates: 0,
            error: Some("failed to load session history: boom".into())
        }
    );
}

#[test]
fn partial_usage_snapshot_preserves_supplied_values_and_unknown_sections() {
    let envelope = decode(
        json!({"request_id":"usage-operation","event":"usage_snapshot","session_id":"usage-session",
        "snapshot":{"rate_limits_available":true,"five_hour":{"utilization":42.0,"resets_at":"2026-08-12T10:00:00Z"},
            "session":{"total_cost_usd":0.125,"total_lines_added":12.0,"model_count":2},
            "activity_day":{"request_count":4,"session_count":2}}}),
    );
    assert_eq!(envelope.request_id.as_deref(), Some("usage-operation"));
    let BridgeEvent::UsageSnapshot { session_id, snapshot: Some(snapshot), error } = envelope.event
    else {
        panic!("usage snapshot")
    };
    assert_eq!(session_id, "usage-session");
    assert_eq!(error, None);
    assert_eq!(snapshot.rate_limits_available, Some(true));
    assert_eq!(
        snapshot.five_hour,
        Some(types::StructuredUsageWindow {
            utilization: 42.0,
            resets_at: Some("2026-08-12T10:00:00Z".into())
        })
    );
    assert_eq!(
        snapshot.session,
        Some(types::StructuredSessionUsage {
            total_cost_usd: Some(0.125),
            total_lines_added: Some(12.0),
            model_count: Some(2),
            total_api_duration_ms: None,
            total_duration_ms: None,
            total_lines_removed: None
        })
    );
    let activity = snapshot.activity_day.unwrap();
    assert_eq!((activity.request_count, activity.session_count), (4, 2));
    assert_eq!(snapshot.subscription_type, None);
    assert_eq!(snapshot.seven_day, None);
    assert_eq!(snapshot.seven_day_oauth_apps, None);
    assert_eq!(snapshot.seven_day_opus, None);
    assert_eq!(snapshot.seven_day_sonnet, None);
    assert_eq!(snapshot.extra_usage, None);
    assert_eq!(snapshot.activity_week, None);
    assert!(snapshot.model_scoped.is_empty());
}

#[test]
fn mcp_snapshot_preserves_provenance_and_explicit_or_absent_capabilities() {
    for (raw_capabilities, expected) in [
        (
            None,
            types::McpAuthCapabilities {
                authenticate: false,
                clear_auth: false,
                submit_oauth_callback_url: false,
            },
        ),
        (
            Some(json!({"authenticate":true,"clear_auth":false,"submit_oauth_callback_url":true})),
            types::McpAuthCapabilities {
                authenticate: true,
                clear_auth: false,
                submit_oauth_callback_url: true,
            },
        ),
    ] {
        let mut raw = json!({"request_id":"snapshot-operation","event":"mcp_snapshot","session_id":"snapshot-session","source":"reload_plugins","servers":[]});
        if let Some(capabilities) = raw_capabilities {
            raw["auth_capabilities"] = capabilities;
        }
        assert_eq!(
            decode(raw),
            EventEnvelope {
                request_id: Some("snapshot-operation".into()),
                event: BridgeEvent::McpSnapshot {
                    session_id: "snapshot-session".into(),
                    servers: vec![],
                    auth_capabilities: expected,
                    source: Some(types::McpSnapshotSource::ReloadPlugins),
                    error: None
                }
            }
        );
    }
}

#[test]
fn dialog_requests_preserve_every_option_and_optional_refusal_metadata() {
    for supplied in [false, true] {
        let mut payload =
            json!({"original_model":"claude-opus-4-8","fallback_model":"claude-sonnet-4-6"});
        if supplied {
            payload["guidance_text"] = json!("This request was declined.");
            payload["api_refusal_category"] = json!("policy");
            payload["retracted_message_uuids"] = json!(["retracted-1", "retracted-2"]);
        }
        let envelope = decode(
            json!({"event":"user_dialog_request","session_id":"dialog-session","request":{
            "request_id":"callback-9","dialog_kind":"refusal_fallback_prompt","payload":payload,
            "options":[{"option_id":"retry_fallback","label":"Switch to claude-sonnet-4-6"},
                {"option_id":"edit_prompt","label":"Edit prompt and retry with claude-opus-4-8"}]}}),
        );
        let BridgeEvent::UserDialogRequest { session_id, request } = envelope.event else {
            panic!("dialog request")
        };
        assert_eq!(session_id, "dialog-session");
        assert_eq!(request.request_id, "callback-9");
        assert_eq!(request.dialog_kind, "refusal_fallback_prompt");
        assert_eq!(
            request.payload,
            types::RefusalFallbackPayload {
                original_model: "claude-opus-4-8".into(),
                fallback_model: "claude-sonnet-4-6".into(),
                guidance_text: supplied.then(|| "This request was declined.".into()),
                api_refusal_category: supplied.then(|| "policy".into()),
                retracted_message_uuids: supplied
                    .then(|| vec!["retracted-1".into(), "retracted-2".into()])
            }
        );
        assert_eq!(
            request.options,
            vec![
                types::UserDialogOption {
                    option_id: "retry_fallback".into(),
                    label: "Switch to claude-sonnet-4-6".into()
                },
                types::UserDialogOption {
                    option_id: "edit_prompt".into(),
                    label: "Edit prompt and retry with claude-opus-4-8".into()
                },
            ]
        );
    }
}

#[test]
fn subagent_envelopes_preserve_transcripts_errors_and_stop_run_identity() {
    let commands = [
        BridgeCommand::ListSubagents { session_id: "s1".to_owned() },
        BridgeCommand::GetSubagentMessages {
            session_id: "s1".to_owned(),
            agent_id: "a1".to_owned(),
        },
        BridgeCommand::StopSubagent {
            session_id: "s1".to_owned(),
            task_id: "a1".to_owned(),
            expected_run_id: None,
        },
    ];
    let json: Vec<_> = commands
        .into_iter()
        .map(|command| serde_json::to_value(CommandEnvelope { request_id: None, command }))
        .collect::<Result<_, _>>()
        .expect("serialize");
    assert_eq!(
        json,
        [
            serde_json::json!({ "command": "list_subagents", "session_id": "s1" }),
            serde_json::json!({ "command": "get_subagent_messages", "session_id": "s1", "agent_id": "a1" }),
            serde_json::json!({ "command": "stop_subagent", "session_id": "s1", "task_id": "a1" }),
        ]
    );

    let event = |value| serde_json::from_value::<EventEnvelope>(value).expect("event").event;
    assert_eq!(
        event(
            serde_json::json!({ "event": "subagent_messages", "session_id": "s1", "agent_id": "a1",
            "messages": [{ "role": "user", "text": "Go" }, { "role": "system_reminder", "text": "Report back" },
                { "role": "tool", "text": "ls" }, { "role": "assistant", "text": "Done" }] })
        ),
        BridgeEvent::SubagentMessages {
            session_id: "s1".to_owned(),
            agent_id: "a1".to_owned(),
            messages: [
                (types::SubagentMessageRole::User, "Go"),
                (types::SubagentMessageRole::SystemReminder, "Report back"),
                (types::SubagentMessageRole::Tool, "ls"),
                (types::SubagentMessageRole::Assistant, "Done"),
            ]
            .map(|(role, text)| types::SubagentMessage { role, text: text.to_owned() })
            .to_vec(),
            error: None,
        }
    );
    assert_eq!(
        event(
            serde_json::json!({ "event": "subagents_listed", "session_id": "s1", "agent_ids": [], "error": "unreadable" })
        ),
        BridgeEvent::SubagentsListed {
            session_id: "s1".to_owned(),
            agent_ids: Vec::new(),
            error: Some("unreadable".to_owned()),
        }
    );
    assert_eq!(
        event(
            serde_json::json!({ "event": "subagent_stop_result", "session_id": "s1", "task_id": "a1" })
        ),
        BridgeEvent::SubagentStopResult {
            session_id: "s1".to_owned(),
            task_id: "a1".to_owned(),
            run_id: None,
            error: None,
        }
    );

    let command = CommandEnvelope {
        request_id: Some("stop-operation".into()),
        command: BridgeCommand::StopSubagent {
            session_id: "owner-session".into(),
            task_id: "worker-task".into(),
            expected_run_id: Some("execution-42".into()),
        },
    };
    assert_eq!(
        serde_json::to_value(command).unwrap(),
        json!({"request_id":"stop-operation","command":"stop_subagent","session_id":"owner-session","task_id":"worker-task","expected_run_id":"execution-42"})
    );
    let stopped = decode(
        json!({"request_id":"stop-operation","event":"subagent_stop_result","session_id":"owner-session","task_id":"worker-task","run_id":"execution-42"}),
    );
    assert_eq!(
        stopped,
        EventEnvelope {
            request_id: Some("stop-operation".into()),
            event: BridgeEvent::SubagentStopResult {
                session_id: "owner-session".into(),
                task_id: "worker-task".into(),
                run_id: Some("execution-42".into()),
                error: None
            }
        }
    );
}
