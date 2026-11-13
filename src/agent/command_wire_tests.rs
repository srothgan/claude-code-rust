// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::json;

#[test]
fn session_control_commands_encode_values_and_explicit_resets() {
    for (label, command, expected) in [
        (
            "effort",
            BridgeCommand::SetEffort {
                session_id: "effort-session".into(),
                effort: Some("max".into()),
            },
            json!({"command":"set_effort","session_id":"effort-session","effort":"max"}),
        ),
        (
            "effort reset",
            BridgeCommand::SetEffort { session_id: "effort-session".into(), effort: None },
            json!({"command":"set_effort","session_id":"effort-session","effort":null}),
        ),
        (
            "agent",
            BridgeCommand::SetAgent {
                session_id: "agent-session".into(),
                agent: Some("reviewer".into()),
            },
            json!({"command":"set_agent","session_id":"agent-session","agent":"reviewer"}),
        ),
        (
            "agent reset",
            BridgeCommand::SetAgent { session_id: "agent-session".into(), agent: None },
            json!({"command":"set_agent","session_id":"agent-session","agent":null}),
        ),
        (
            "fast on",
            BridgeCommand::SetFastMode { session_id: "fast-session".into(), enabled: true },
            json!({"command":"set_fast_mode","session_id":"fast-session","enabled":true}),
        ),
        (
            "fast off",
            BridgeCommand::SetFastMode { session_id: "fast-session".into(), enabled: false },
            json!({"command":"set_fast_mode","session_id":"fast-session","enabled":false}),
        ),
    ] {
        assert_eq!(
            serde_json::to_value(CommandEnvelope { request_id: None, command }).unwrap(),
            expected,
            "{label}"
        );
    }
}

#[test]
fn session_requests_keep_operation_ids_at_the_envelope_boundary() {
    for (request, command, expected) in [
        (
            Some("cancel-operation"),
            BridgeCommand::CancelTurn { session_id: "cancel-session".into() },
            json!({"request_id":"cancel-operation","command":"cancel_turn","session_id":"cancel-session"}),
        ),
        (
            None,
            BridgeCommand::GetRewindTargets { session_id: "target-session".into() },
            json!({"command":"get_rewind_targets","session_id":"target-session"}),
        ),
        (
            None,
            BridgeCommand::ReloadSkillsAndStyles { session_id: "reload-session".into() },
            json!({"command":"reload_skills_and_styles","session_id":"reload-session"}),
        ),
        (
            Some("export-operation"),
            BridgeCommand::ExportSessionHistory { session_id: "export-session".into() },
            json!({"request_id":"export-operation","command":"export_session_history","session_id":"export-session"}),
        ),
    ] {
        assert_eq!(
            serde_json::to_value(CommandEnvelope {
                request_id: request.map(str::to_owned),
                command
            })
            .unwrap(),
            expected
        );
    }
}

#[test]
fn dialog_callbacks_preserve_selection_or_cancel_without_an_option() {
    for (outcome, expected) in [
        (
            types::UserDialogOutcome::Selected { option_id: "retry_fallback".into() },
            json!({"command":"user_dialog_response","session_id":"dialog-session","request_id":"callback-17","outcome":{"outcome":"selected","option_id":"retry_fallback"}}),
        ),
        (
            types::UserDialogOutcome::Cancelled,
            json!({"command":"user_dialog_response","session_id":"dialog-session","request_id":"callback-17","outcome":{"outcome":"cancelled"}}),
        ),
    ] {
        let envelope = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::UserDialogResponse {
                session_id: "dialog-session".into(),
                request_id: "callback-17".into(),
                outcome,
            },
        };
        assert_eq!(serde_json::to_value(envelope).unwrap(), expected);
    }
}
