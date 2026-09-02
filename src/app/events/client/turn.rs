// SPDX-License-Identifier: Apache-2.0

use super::super::{App, session, turn};
use crate::agent::events::ClientEvent;

pub(super) fn handle(app: &mut App, event: ClientEvent) {
    match event {
        ClientEvent::SessionUpdate { session_id: _, update } => {
            super::super::handle_session_update_event(app, update);
        }
        ClientEvent::PermissionRequest { session_id: _, request, response_tx } => {
            turn::handle_permission_request_event(app, request, response_tx);
        }
        ClientEvent::QuestionRequest { session_id: _, request, response_tx } => {
            turn::handle_question_request_event(app, request, response_tx);
        }
        ClientEvent::UserDialogRequest { session_id: _, request, response_tx } => {
            turn::handle_user_dialog_request_event(app, request, response_tx);
        }
        ClientEvent::UserMessageQueued { session_id: _, message_uuid } => {
            turn::handle_user_message_queued_event(app, &message_uuid);
        }
        ClientEvent::UserMessageStarted { session_id: _, message_uuid, source } => {
            turn::handle_user_message_started_event(app, &message_uuid, source);
        }
        ClientEvent::UserMessageRejected { session_id: _, message_uuid, reason } => {
            turn::handle_user_message_rejected_event(app, &message_uuid, &reason);
        }
        ClientEvent::TurnInterruptReceipt { session_id: _, still_queued } => {
            turn::handle_turn_interrupt_receipt_event(app, &still_queued);
        }
        ClientEvent::TurnComplete { session_id: _, queued_turn_count, terminal_reason } => {
            turn::handle_turn_complete_event(app, queued_turn_count, terminal_reason);
        }
        ClientEvent::TurnError {
            session_id: _,
            message,
            queued_turn_count,
            api_error_status,
            terminal_reason,
        } => {
            turn::handle_turn_error_event(
                app,
                &message,
                None,
                queued_turn_count,
                api_error_status,
                terminal_reason,
            );
        }
        ClientEvent::TurnErrorClassified {
            session_id: _,
            message,
            class,
            queued_turn_count,
            api_error_status,
            terminal_reason,
        } => {
            turn::handle_turn_error_event(
                app,
                &message,
                Some(class),
                queued_turn_count,
                api_error_status,
                terminal_reason,
            );
        }
        ClientEvent::SlashCommandError { session_id: _, message } => {
            session::handle_slash_command_error_event(app, &message);
        }
        _ => unreachable!("client event family routed a non-turn event to the turn handler"),
    }
}
