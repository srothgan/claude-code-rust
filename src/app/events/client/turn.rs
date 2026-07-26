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
        ClientEvent::TurnComplete { session_id: _, terminal_reason } => {
            turn::handle_turn_complete_event(app, terminal_reason);
        }
        ClientEvent::TurnError { session_id: _, message, api_error_status, terminal_reason } => {
            turn::handle_turn_error_event(app, &message, None, api_error_status, terminal_reason);
        }
        ClientEvent::TurnErrorClassified {
            session_id: _,
            message,
            class,
            api_error_status,
            terminal_reason,
        } => {
            turn::handle_turn_error_event(
                app,
                &message,
                Some(class),
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
