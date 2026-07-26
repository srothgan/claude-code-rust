// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::agent::error_handling::TurnErrorClass;
use crate::agent::model;
use crate::app::plugins::{PluginsCliActionSuccess, PluginsInventorySnapshot};
use crate::app::{ReleaseReason, UsageSnapshot, UsageSourceKind};
use crate::error::AppError;
use std::path::PathBuf;
use std::rc::Rc;

/// Messages sent from the backend bridge path to the App/UI layer.
pub enum ClientEvent {
    /// Session update notification (streaming text, tool calls, etc.)
    SessionUpdate { session_id: String, update: model::SessionUpdate },
    /// Permission request that needs user input.
    PermissionRequest {
        session_id: String,
        request: model::RequestPermissionRequest,
        response_tx: tokio::sync::oneshot::Sender<model::RequestPermissionResponse>,
    },
    /// Question request from `AskUserQuestion` that needs structured user input.
    QuestionRequest {
        session_id: String,
        request: model::RequestQuestionRequest,
        response_tx: tokio::sync::oneshot::Sender<model::RequestQuestionResponse>,
    },
    /// Turn-level `request_user_dialog` (e.g. `refusal_fallback_prompt`) that
    /// needs a user decision. Not anchored to a tool call.
    UserDialogRequest {
        session_id: String,
        request: model::RequestUserDialogRequest,
        response_tx: tokio::sync::oneshot::Sender<model::RequestUserDialogResponse>,
    },
    /// MCP elicitation request that needs auth or other MCP input.
    McpElicitationRequest { session_id: String, request: crate::agent::types::ElicitationRequest },
    /// MCP elicitation completed in the SDK.
    McpElicitationCompleted {
        session_id: String,
        elicitation_id: String,
        server_name: Option<String>,
    },
    /// MCP auth redirect returned directly by the SDK auth call.
    McpAuthRedirect { session_id: String, redirect: crate::agent::types::McpAuthRedirect },
    /// MCP operation failed and should be surfaced in the MCP config UI.
    McpOperationError { session_id: String, error: crate::agent::types::McpOperationError },
    /// Dynamic MCP server replacement completed through the SDK.
    McpSetServersResult { session_id: String, result: crate::agent::types::McpSetServersResult },
    /// Claude CLI removed an MCP server from a persisted config scope.
    McpConfigRemoveSucceeded {
        cwd_raw: String,
        server_name: String,
        scope: String,
        claude_path: PathBuf,
    },
    /// Claude CLI failed to remove an MCP server from persisted config.
    McpConfigRemoveFailed { cwd_raw: String, server_name: String, scope: String, message: String },
    /// A prompt turn completed successfully.
    TurnComplete {
        session_id: String,
        terminal_reason: Option<crate::agent::types::TerminalReason>,
    },
    /// `cancel` notification was accepted by the bridge.
    TurnCancelled { session_id: model::SessionId },
    /// A prompt turn failed with an error.
    TurnError {
        session_id: String,
        message: String,
        api_error_status: Option<u16>,
        terminal_reason: Option<crate::agent::types::TerminalReason>,
    },
    /// A prompt turn failed with bridge-provided classification metadata.
    TurnErrorClassified {
        session_id: String,
        message: String,
        class: TurnErrorClass,
        api_error_status: Option<u16>,
        terminal_reason: Option<crate::agent::types::TerminalReason>,
    },
    /// Background connection completed successfully.
    Connected {
        session_id: model::SessionId,
        cwd: String,
        current_model: model::CurrentModel,
        available_models: Vec<model::AvailableModel>,
        mode: Option<crate::app::ModeState>,
        fast_mode_state: model::FastModeState,
        history_updates: Vec<model::SessionUpdate>,
    },
    /// Background connection failed.
    ConnectionFailed(String),
    /// Authentication is required before a session can be created.
    AuthRequired { method_name: String, method_description: String },
    /// Slash-command execution failed with a user-facing error.
    SlashCommandError { session_id: Option<String>, message: String },
    /// Terminal ownership was handed to a child process.
    TerminalReleasedToChild { reason: ReleaseReason },
    /// Terminal ownership returned from a child process.
    TerminalReturnedFromChild { reason: ReleaseReason },
    /// Session runtime plugin reload completed successfully.
    RuntimeReloadCompleted { session_id: String },
    /// Session runtime plugin reload failed after dispatch.
    RuntimeReloadFailed { session_id: String, message: String },
    /// Custom slash command replaced the active session.
    SessionReplaced {
        session_id: model::SessionId,
        cwd: String,
        current_model: model::CurrentModel,
        available_models: Vec<model::AvailableModel>,
        mode: Option<crate::app::ModeState>,
        fast_mode_state: model::FastModeState,
        history_updates: Vec<model::SessionUpdate>,
        restored_input: Option<String>,
    },
    /// Recent sessions discovered via SDK session listing.
    SessionsListed { sessions: Vec<crate::agent::types::SessionListEntry> },
    /// Startup update check found a newer published version.
    UpdateAvailable { latest_version: String, current_version: String },
    /// Startup Claude Code status check detected degraded/outage conditions.
    ServiceStatus { severity: ServiceStatusSeverity, message: String },
    /// /login completed via `claude auth login` -- credentials stored, ready to start a session.
    AuthCompleted { conn: Rc<crate::agent::client::AgentConnection> },
    /// /logout completed via `claude auth logout`.
    LogoutCompleted,
    /// Status snapshot received from bridge (account info).
    StatusSnapshotReceived { session_id: String, account: model::AccountInfo },
    /// Session context window usage received from bridge.
    ContextUsageReceived { session_id: String, percentage: Option<u8> },
    /// Rewind target candidates loaded from persisted SDK session history.
    RewindTargetsReceived { session_id: String, targets: Vec<model::RewindTarget> },
    /// Result of a rewind operation that touched files.
    RewindResultReceived { result: model::RewindResult },
    /// MCP server snapshot received from bridge.
    McpSnapshotReceived {
        session_id: String,
        servers: Vec<model::McpServerStatus>,
        source: Option<crate::agent::types::McpSnapshotSource>,
        error: Option<String>,
    },
    /// Usage refresh task started.
    UsageRefreshStarted { epoch: u64 },
    /// Usage refresh completed successfully.
    UsageSnapshotReceived { epoch: u64, snapshot: UsageSnapshot },
    /// Usage refresh failed.
    UsageRefreshFailed { epoch: u64, message: String, source: UsageSourceKind },
    /// Claude CLI plugin inventory refresh completed.
    PluginsInventoryUpdated {
        cwd_raw: String,
        snapshot: PluginsInventorySnapshot,
        claude_path: PathBuf,
    },
    /// Claude CLI plugin inventory refresh failed.
    PluginsInventoryRefreshFailed { cwd_raw: String, message: String },
    /// Plugin CLI action completed and returned a refreshed inventory snapshot.
    PluginsCliActionSucceeded { cwd_raw: String, result: PluginsCliActionSuccess },
    /// Plugin CLI action failed.
    PluginsCliActionFailed { cwd_raw: String, message: String },
    /// Fatal app error that should terminate and map to an exit code.
    FatalError(AppError),
}

impl ClientEvent {
    /// Return the session authority attached to active-session state mutations.
    ///
    /// Connection events are deliberately excluded because they establish or
    /// replace the active authority rather than operate under it.
    #[must_use]
    pub(crate) fn scoped_session_id(&self) -> Option<&str> {
        match self {
            Self::SessionUpdate { session_id, .. }
            | Self::PermissionRequest { session_id, .. }
            | Self::QuestionRequest { session_id, .. }
            | Self::UserDialogRequest { session_id, .. }
            | Self::McpElicitationRequest { session_id, .. }
            | Self::McpElicitationCompleted { session_id, .. }
            | Self::McpAuthRedirect { session_id, .. }
            | Self::McpOperationError { session_id, .. }
            | Self::McpSetServersResult { session_id, .. }
            | Self::TurnComplete { session_id, .. }
            | Self::TurnError { session_id, .. }
            | Self::TurnErrorClassified { session_id, .. }
            | Self::RuntimeReloadCompleted { session_id }
            | Self::RuntimeReloadFailed { session_id, .. }
            | Self::StatusSnapshotReceived { session_id, .. }
            | Self::ContextUsageReceived { session_id, .. }
            | Self::RewindTargetsReceived { session_id, .. }
            | Self::McpSnapshotReceived { session_id, .. } => Some(session_id),
            Self::SlashCommandError { session_id, .. } => session_id.as_deref(),
            Self::TurnCancelled { session_id } => Some(session_id.as_str()),
            Self::RewindResultReceived { result } => Some(result.session_id.as_str()),
            Self::Connected { .. }
            | Self::ConnectionFailed(_)
            | Self::AuthRequired { .. }
            | Self::McpConfigRemoveSucceeded { .. }
            | Self::McpConfigRemoveFailed { .. }
            | Self::TerminalReleasedToChild { .. }
            | Self::TerminalReturnedFromChild { .. }
            | Self::SessionReplaced { .. }
            | Self::SessionsListed { .. }
            | Self::UpdateAvailable { .. }
            | Self::ServiceStatus { .. }
            | Self::AuthCompleted { .. }
            | Self::LogoutCompleted
            | Self::UsageRefreshStarted { .. }
            | Self::UsageSnapshotReceived { .. }
            | Self::UsageRefreshFailed { .. }
            | Self::PluginsInventoryUpdated { .. }
            | Self::PluginsInventoryRefreshFailed { .. }
            | Self::PluginsCliActionSucceeded { .. }
            | Self::PluginsCliActionFailed { .. }
            | Self::FatalError(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatusSeverity {
    Warning,
    Error,
}
