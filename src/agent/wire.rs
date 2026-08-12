// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::agent::types;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLaunchSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_progress_summaries: Option<bool>,
}

impl SessionLaunchSettings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.language.is_none()
            && self.settings.is_none()
            && self.agent_progress_summaries.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(flatten)]
    pub command: BridgeCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum BridgeCommand {
    Initialize {
        cwd: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metadata: BTreeMap<String, serde_json::Value>,
    },
    CreateSession {
        cwd: String,
        resume: Option<String>,
        #[serde(default, skip_serializing_if = "SessionLaunchSettings::is_empty")]
        launch_settings: SessionLaunchSettings,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metadata: BTreeMap<String, serde_json::Value>,
    },
    ResumeSession {
        session_id: String,
        #[serde(default, skip_serializing_if = "SessionLaunchSettings::is_empty")]
        launch_settings: SessionLaunchSettings,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metadata: BTreeMap<String, serde_json::Value>,
    },
    ResumeSessionAt {
        session_id: String,
        target_user_message_id: String,
        #[serde(default, skip_serializing_if = "SessionLaunchSettings::is_empty")]
        launch_settings: SessionLaunchSettings,
    },
    Prompt {
        session_id: String,
        chunks: Vec<types::PromptChunk>,
    },
    CancelTurn {
        session_id: String,
    },
    SetModel {
        session_id: String,
        model: String,
    },
    SetMode {
        session_id: String,
        mode: String,
    },
    SetEffort {
        session_id: String,
        effort: String,
    },
    SetAgent {
        session_id: String,
        agent: Option<String>,
    },
    SetFastMode {
        session_id: String,
        enabled: bool,
    },
    GenerateSessionTitle {
        session_id: String,
        description: String,
    },
    RenameSession {
        session_id: String,
        title: String,
    },
    NewSession {
        cwd: String,
        #[serde(default, skip_serializing_if = "SessionLaunchSettings::is_empty")]
        launch_settings: SessionLaunchSettings,
    },
    PermissionResponse {
        session_id: String,
        tool_call_id: String,
        outcome: types::PermissionOutcome,
    },
    QuestionResponse {
        session_id: String,
        tool_call_id: String,
        outcome: types::QuestionOutcome,
    },
    UserDialogResponse {
        session_id: String,
        request_id: String,
        outcome: types::UserDialogOutcome,
    },
    ElicitationResponse {
        session_id: String,
        elicitation_request_id: String,
        action: types::ElicitationAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<serde_json::Value>,
    },
    GetStatusSnapshot {
        session_id: String,
    },
    GetContextUsage {
        session_id: String,
    },
    GetUsage {
        session_id: String,
    },
    GetRewindTargets {
        session_id: String,
    },
    Rewind {
        session_id: String,
        target_user_message_id: String,
        restore_mode: types::RewindRestoreMode,
        #[serde(default, skip_serializing_if = "SessionLaunchSettings::is_empty")]
        launch_settings: SessionLaunchSettings,
    },
    ReloadPlugins {
        session_id: String,
    },
    GetMcpSnapshot {
        session_id: String,
    },
    McpReconnect {
        session_id: String,
        server_name: String,
    },
    McpToggle {
        session_id: String,
        server_name: String,
        enabled: bool,
    },
    McpSetServers {
        session_id: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        servers: BTreeMap<String, types::McpServerConfig>,
    },
    McpAuthenticate {
        session_id: String,
        server_name: String,
    },
    McpClearAuth {
        session_id: String,
        server_name: String,
    },
    McpOauthCallbackUrl {
        session_id: String,
        server_name: String,
        callback_url: String,
    },
    Shutdown,
}

impl BridgeCommand {
    #[must_use]
    pub fn command_name(&self) -> &'static str {
        match self {
            Self::Initialize { .. } => "initialize",
            Self::CreateSession { .. } => "create_session",
            Self::ResumeSession { .. } => "resume_session",
            Self::ResumeSessionAt { .. } => "resume_session_at",
            Self::Prompt { .. } => "prompt",
            Self::CancelTurn { .. } => "cancel_turn",
            Self::SetModel { .. } => "set_model",
            Self::SetMode { .. } => "set_mode",
            Self::SetEffort { .. } => "set_effort",
            Self::SetAgent { .. } => "set_agent",
            Self::SetFastMode { .. } => "set_fast_mode",
            Self::GenerateSessionTitle { .. } => "generate_session_title",
            Self::RenameSession { .. } => "rename_session",
            Self::NewSession { .. } => "new_session",
            Self::PermissionResponse { .. } => "permission_response",
            Self::QuestionResponse { .. } => "question_response",
            Self::UserDialogResponse { .. } => "user_dialog_response",
            Self::ElicitationResponse { .. } => "elicitation_response",
            Self::GetStatusSnapshot { .. } => "get_status_snapshot",
            Self::GetContextUsage { .. } => "get_context_usage",
            Self::GetUsage { .. } => "get_usage",
            Self::GetRewindTargets { .. } => "get_rewind_targets",
            Self::Rewind { .. } => "rewind",
            Self::ReloadPlugins { .. } => "reload_plugins",
            Self::GetMcpSnapshot { .. } => "get_mcp_snapshot",
            Self::McpReconnect { .. } => "mcp_reconnect",
            Self::McpToggle { .. } => "mcp_toggle",
            Self::McpSetServers { .. } => "mcp_set_servers",
            Self::McpAuthenticate { .. } => "mcp_authenticate",
            Self::McpClearAuth { .. } => "mcp_clear_auth",
            Self::McpOauthCallbackUrl { .. } => "mcp_oauth_callback_url",
            Self::Shutdown => "shutdown",
        }
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::ResumeSession { session_id, .. }
            | Self::ResumeSessionAt { session_id, .. }
            | Self::Prompt { session_id, .. }
            | Self::CancelTurn { session_id }
            | Self::SetModel { session_id, .. }
            | Self::SetMode { session_id, .. }
            | Self::SetEffort { session_id, .. }
            | Self::SetAgent { session_id, .. }
            | Self::SetFastMode { session_id, .. }
            | Self::GenerateSessionTitle { session_id, .. }
            | Self::RenameSession { session_id, .. }
            | Self::PermissionResponse { session_id, .. }
            | Self::QuestionResponse { session_id, .. }
            | Self::UserDialogResponse { session_id, .. }
            | Self::ElicitationResponse { session_id, .. }
            | Self::GetStatusSnapshot { session_id }
            | Self::GetContextUsage { session_id }
            | Self::GetUsage { session_id }
            | Self::GetRewindTargets { session_id }
            | Self::Rewind { session_id, .. }
            | Self::ReloadPlugins { session_id }
            | Self::GetMcpSnapshot { session_id }
            | Self::McpReconnect { session_id, .. }
            | Self::McpToggle { session_id, .. }
            | Self::McpSetServers { session_id, .. }
            | Self::McpAuthenticate { session_id, .. }
            | Self::McpClearAuth { session_id, .. }
            | Self::McpOauthCallbackUrl { session_id, .. } => Some(session_id.as_str()),
            Self::CreateSession { resume, .. } => resume.as_deref(),
            Self::Initialize { .. } | Self::NewSession { .. } | Self::Shutdown => None,
        }
    }

    #[must_use]
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            Self::PermissionResponse { tool_call_id, .. }
            | Self::QuestionResponse { tool_call_id, .. } => Some(tool_call_id.as_str()),
            Self::Initialize { .. }
            | Self::CreateSession { .. }
            | Self::ResumeSession { .. }
            | Self::ResumeSessionAt { .. }
            | Self::Prompt { .. }
            | Self::CancelTurn { .. }
            | Self::SetModel { .. }
            | Self::SetMode { .. }
            | Self::SetEffort { .. }
            | Self::SetAgent { .. }
            | Self::SetFastMode { .. }
            | Self::GenerateSessionTitle { .. }
            | Self::RenameSession { .. }
            | Self::NewSession { .. }
            | Self::UserDialogResponse { .. }
            | Self::ElicitationResponse { .. }
            | Self::GetStatusSnapshot { .. }
            | Self::GetContextUsage { .. }
            | Self::GetUsage { .. }
            | Self::GetRewindTargets { .. }
            | Self::Rewind { .. }
            | Self::ReloadPlugins { .. }
            | Self::GetMcpSnapshot { .. }
            | Self::McpReconnect { .. }
            | Self::McpToggle { .. }
            | Self::McpSetServers { .. }
            | Self::McpAuthenticate { .. }
            | Self::McpClearAuth { .. }
            | Self::McpOauthCallbackUrl { .. }
            | Self::Shutdown => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(flatten)]
    pub event: BridgeEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BridgeEvent {
    Connected {
        session_id: String,
        cwd: String,
        current_model: types::CurrentModel,
        #[serde(default)]
        available_models: Vec<types::AvailableModel>,
        mode: Option<types::ModeState>,
        fast_mode_state: types::FastModeState,
        fast_mode_disabled_reason: Option<String>,
        history_updates: Option<Vec<types::SessionUpdate>>,
    },
    AuthRequired {
        method_name: String,
        method_description: String,
    },
    ConnectionFailed {
        message: String,
    },
    SessionUpdate {
        session_id: String,
        update: types::SessionUpdate,
    },
    PermissionRequest {
        session_id: String,
        request: types::PermissionRequest,
    },
    QuestionRequest {
        session_id: String,
        request: types::QuestionRequest,
    },
    UserDialogRequest {
        session_id: String,
        request: types::UserDialogRequest,
    },
    ElicitationRequest {
        session_id: String,
        request: types::ElicitationRequest,
    },
    ElicitationComplete {
        session_id: String,
        elicitation_id: String,
        server_name: Option<String>,
    },
    McpAuthRedirect {
        session_id: String,
        redirect: types::McpAuthRedirect,
    },
    McpOperationError {
        session_id: String,
        error: types::McpOperationError,
    },
    McpSetServersResult {
        session_id: String,
        result: types::McpSetServersResult,
    },
    TurnComplete {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_reason: Option<types::TerminalReason>,
    },
    TurnError {
        session_id: String,
        message: String,
        error_kind: Option<String>,
        sdk_result_subtype: Option<String>,
        assistant_error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_error_status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_reason: Option<types::TerminalReason>,
    },
    SlashError {
        session_id: String,
        message: String,
    },
    SessionResumeFailed {
        session_id: String,
        message: String,
    },
    RuntimeReloadCompleted {
        session_id: String,
    },
    RuntimeReloadFailed {
        session_id: String,
        message: String,
    },
    SessionReplaced {
        session_id: String,
        cwd: String,
        current_model: types::CurrentModel,
        #[serde(default)]
        available_models: Vec<types::AvailableModel>,
        mode: Option<types::ModeState>,
        fast_mode_state: types::FastModeState,
        fast_mode_disabled_reason: Option<String>,
        history_updates: Option<Vec<types::SessionUpdate>>,
        restored_input: Option<String>,
    },
    Initialized {
        result: types::InitializeResult,
    },
    SessionsListed {
        sessions: Vec<types::SessionListEntry>,
    },
    StatusSnapshot {
        session_id: String,
        account: types::AccountInfo,
    },
    ContextUsage {
        session_id: String,
        percentage: Option<u8>,
    },
    UsageSnapshot {
        session_id: String,
        snapshot: Option<types::StructuredUsageSnapshot>,
        error: Option<String>,
    },
    RewindTargets {
        session_id: String,
        targets: Vec<types::RewindTarget>,
        error: Option<String>,
    },
    RewindResult {
        session_id: String,
        restore_mode: types::RewindRestoreMode,
        status: types::RewindResultStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_result: Option<types::RewindFilesResult>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    McpSnapshot {
        session_id: String,
        #[serde(default)]
        servers: Vec<types::McpServerStatus>,
        #[serde(default)]
        auth_capabilities: types::McpAuthCapabilities,
        source: Option<types::McpSnapshotSource>,
        error: Option<String>,
    },
}

impl BridgeEvent {
    #[must_use]
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "connected",
            Self::AuthRequired { .. } => "auth_required",
            Self::ConnectionFailed { .. } => "connection_failed",
            Self::SessionUpdate { .. } => "session_update",
            Self::PermissionRequest { .. } => "permission_request",
            Self::QuestionRequest { .. } => "question_request",
            Self::UserDialogRequest { .. } => "user_dialog_request",
            Self::ElicitationRequest { .. } => "elicitation_request",
            Self::ElicitationComplete { .. } => "elicitation_complete",
            Self::McpAuthRedirect { .. } => "mcp_auth_redirect",
            Self::McpOperationError { .. } => "mcp_operation_error",
            Self::McpSetServersResult { .. } => "mcp_set_servers_result",
            Self::TurnComplete { .. } => "turn_complete",
            Self::TurnError { .. } => "turn_error",
            Self::SlashError { .. } => "slash_error",
            Self::SessionResumeFailed { .. } => "session_resume_failed",
            Self::RuntimeReloadCompleted { .. } => "runtime_reload_completed",
            Self::RuntimeReloadFailed { .. } => "runtime_reload_failed",
            Self::SessionReplaced { .. } => "session_replaced",
            Self::Initialized { .. } => "initialized",
            Self::SessionsListed { .. } => "sessions_listed",
            Self::StatusSnapshot { .. } => "status_snapshot",
            Self::ContextUsage { .. } => "context_usage",
            Self::UsageSnapshot { .. } => "usage_snapshot",
            Self::RewindTargets { .. } => "rewind_targets",
            Self::RewindResult { .. } => "rewind_result",
            Self::McpSnapshot { .. } => "mcp_snapshot",
        }
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Connected { session_id, .. }
            | Self::SessionUpdate { session_id, .. }
            | Self::PermissionRequest { session_id, .. }
            | Self::QuestionRequest { session_id, .. }
            | Self::UserDialogRequest { session_id, .. }
            | Self::ElicitationRequest { session_id, .. }
            | Self::ElicitationComplete { session_id, .. }
            | Self::McpAuthRedirect { session_id, .. }
            | Self::McpOperationError { session_id, .. }
            | Self::McpSetServersResult { session_id, .. }
            | Self::TurnComplete { session_id, .. }
            | Self::TurnError { session_id, .. }
            | Self::SlashError { session_id, .. }
            | Self::SessionResumeFailed { session_id, .. }
            | Self::RuntimeReloadCompleted { session_id, .. }
            | Self::RuntimeReloadFailed { session_id, .. }
            | Self::SessionReplaced { session_id, .. }
            | Self::StatusSnapshot { session_id, .. }
            | Self::ContextUsage { session_id, .. }
            | Self::UsageSnapshot { session_id, .. }
            | Self::RewindTargets { session_id, .. }
            | Self::RewindResult { session_id, .. }
            | Self::McpSnapshot { session_id, .. } => Some(session_id.as_str()),
            Self::AuthRequired { .. }
            | Self::ConnectionFailed { .. }
            | Self::Initialized { .. }
            | Self::SessionsListed { .. } => None,
        }
    }

    #[must_use]
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            Self::PermissionRequest { request, .. } => {
                Some(request.tool_call.tool_call_id.as_str())
            }
            Self::QuestionRequest { request, .. } => Some(request.tool_call.tool_call_id.as_str()),
            Self::Connected { .. }
            | Self::AuthRequired { .. }
            | Self::ConnectionFailed { .. }
            | Self::SessionUpdate { .. }
            | Self::UserDialogRequest { .. }
            | Self::ElicitationRequest { .. }
            | Self::ElicitationComplete { .. }
            | Self::McpAuthRedirect { .. }
            | Self::McpOperationError { .. }
            | Self::McpSetServersResult { .. }
            | Self::TurnComplete { .. }
            | Self::TurnError { .. }
            | Self::SlashError { .. }
            | Self::SessionResumeFailed { .. }
            | Self::RuntimeReloadCompleted { .. }
            | Self::RuntimeReloadFailed { .. }
            | Self::SessionReplaced { .. }
            | Self::Initialized { .. }
            | Self::SessionsListed { .. }
            | Self::StatusSnapshot { .. }
            | Self::ContextUsage { .. }
            | Self::UsageSnapshot { .. }
            | Self::RewindTargets { .. }
            | Self::RewindResult { .. }
            | Self::McpSnapshot { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BridgeCommand, BridgeEvent, CommandEnvelope, EventEnvelope, SessionLaunchSettings,
    };
    use crate::agent::types;
    use std::collections::BTreeMap;

    #[test]
    fn command_envelope_roundtrip_json() {
        let env = CommandEnvelope {
            request_id: Some("req-1".to_owned()),
            command: BridgeCommand::SetMode {
                session_id: "s1".to_owned(),
                mode: "plan".to_owned(),
            },
        };
        let json = serde_json::to_string(&env).expect("serialize");
        let decoded: CommandEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, env);
    }

    #[test]
    fn session_resume_failed_event_deserializes_with_operation_identity() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "request_id": "resume-at-1",
            "event": "session_resume_failed",
            "session_id": "source-session",
            "message": "resume rejected"
        }))
        .expect("deserialize resume failure");

        assert_eq!(decoded.request_id.as_deref(), Some("resume-at-1"));
        assert_eq!(
            decoded.event,
            BridgeEvent::SessionResumeFailed {
                session_id: "source-session".to_owned(),
                message: "resume rejected".to_owned(),
            }
        );
    }

    #[test]
    fn set_effort_command_serializes_snake_case() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetEffort {
                session_id: "s1".to_owned(),
                effort: "max".to_owned(),
            },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "set_effort",
                "session_id": "s1",
                "effort": "max"
            })
        );
    }

    #[test]
    fn set_agent_command_serializes_snake_case() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetAgent {
                session_id: "s1".to_owned(),
                agent: Some("reviewer".to_owned()),
            },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "set_agent",
                "session_id": "s1",
                "agent": "reviewer"
            })
        );
    }

    #[test]
    fn set_agent_reset_serializes_null_agent() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetAgent { session_id: "s1".to_owned(), agent: None },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "set_agent",
                "session_id": "s1",
                "agent": null
            })
        );
    }

    #[test]
    fn set_fast_mode_command_serializes_snake_case() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SetFastMode { session_id: "s1".to_owned(), enabled: true },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "set_fast_mode",
                "session_id": "s1",
                "enabled": true
            })
        );
    }

    #[test]
    fn cancel_turn_command_serializes_snake_case() {
        let env = CommandEnvelope {
            request_id: Some("request-1".to_owned()),
            command: BridgeCommand::CancelTurn { session_id: "s1".to_owned() },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "request_id": "request-1",
                "command": "cancel_turn",
                "session_id": "s1"
            })
        );
    }

    #[test]
    fn get_rewind_targets_command_serializes_snake_case() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::GetRewindTargets { session_id: "s1".to_owned() },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "get_rewind_targets",
                "session_id": "s1"
            })
        );
    }

    #[test]
    fn structured_usage_event_deserializes_tolerant_snapshot() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "usage_snapshot",
            "session_id": "session-1",
            "snapshot": {
                "rate_limits_available": true,
                "five_hour": {
                    "utilization": 42.0,
                    "resets_at": "2026-08-12T10:00:00Z"
                },
                "session": {
                    "total_cost_usd": 0.125,
                    "total_lines_added": 12.0,
                    "model_count": 2
                },
                "activity_day": {
                    "request_count": 4,
                    "session_count": 2
                }
            }
        }))
        .expect("deserialize structured usage");

        let BridgeEvent::UsageSnapshot { session_id, snapshot: Some(snapshot), error } =
            decoded.event
        else {
            panic!("expected usage snapshot event");
        };
        assert_eq!(session_id, "session-1");
        assert!(error.is_none());
        assert_eq!(snapshot.five_hour.map(|window| window.utilization), Some(42.0));
        assert_eq!(snapshot.session.and_then(|session| session.model_count), Some(2));
        assert_eq!(snapshot.activity_day.map(|window| window.request_count), Some(4));
    }

    #[test]
    fn rewind_command_serializes_snake_case() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::Rewind {
                session_id: "s1".to_owned(),
                target_user_message_id: "user-1".to_owned(),
                restore_mode: types::RewindRestoreMode::Both,
                launch_settings: SessionLaunchSettings {
                    language: Some("German".to_owned()),
                    ..SessionLaunchSettings::default()
                },
            },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "rewind",
                "session_id": "s1",
                "target_user_message_id": "user-1",
                "restore_mode": "both",
                "launch_settings": {
                    "language": "German"
                }
            })
        );
    }

    #[test]
    fn rewind_result_event_deserializes() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "rewind_result",
            "session_id": "session-1",
            "restore_mode": "code",
            "status": "success",
            "file_result": {
                "can_rewind": true,
                "files_changed": ["src/main.rs"],
                "insertions": 2,
                "deletions": 1,
                "skipped_links": 3
            }
        }))
        .expect("deserialize rewind result");

        assert_eq!(
            decoded.event,
            BridgeEvent::RewindResult {
                session_id: "session-1".to_owned(),
                restore_mode: types::RewindRestoreMode::Code,
                status: types::RewindResultStatus::Success,
                file_result: Some(types::RewindFilesResult {
                    can_rewind: true,
                    error: None,
                    files_changed: vec!["src/main.rs".to_owned()],
                    insertions: Some(2),
                    deletions: Some(1),
                    skipped_links: Some(3),
                }),
                message: None,
            }
        );
    }

    #[test]
    fn mcp_set_servers_command_serializes_latest_fields_as_snake_case() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::McpSetServers {
                session_id: "s1".to_owned(),
                servers: BTreeMap::from([(
                    "notion".to_owned(),
                    types::McpServerConfig::Http {
                        url: "https://mcp.notion.com/mcp".to_owned(),
                        headers: BTreeMap::new(),
                        tools: vec![
                            types::McpServerToolPolicy {
                                name: "search".to_owned(),
                                permission_policy: Some(
                                    types::McpServerToolPermissionPolicy::Allow,
                                ),
                                org_max_permission: Some(types::McpServerOrgMaxPermission::Ask),
                            },
                            types::McpServerToolPolicy {
                                name: "lookup".to_owned(),
                                permission_policy: None,
                                org_max_permission: Some(types::McpServerOrgMaxPermission::Blocked),
                            },
                        ],
                        timeout: Some(5000),
                        request_timeout_ms: Some(30000),
                        always_load: Some(true),
                    },
                )]),
            },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "mcp_set_servers",
                "session_id": "s1",
                "servers": {
                    "notion": {
                        "type": "http",
                        "url": "https://mcp.notion.com/mcp",
                        "headers": {},
                        "tools": [
                            {
                                "name": "search",
                                "permission_policy": "always_allow",
                                "org_max_permission": "ask"
                            },
                            {
                                "name": "lookup",
                                "org_max_permission": "blocked"
                            }
                        ],
                        "timeout": 5000,
                        "request_timeout_ms": 30000,
                        "always_load": true
                    }
                }
            })
        );
    }

    #[test]
    fn event_envelope_roundtrip_json() {
        let env = EventEnvelope {
            request_id: None,
            event: BridgeEvent::TurnComplete {
                session_id: "session-1".to_owned(),
                terminal_reason: Some(types::TerminalReason::Completed),
            },
        };
        let json = serde_json::to_string(&env).expect("serialize");
        let decoded: EventEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, env);
    }

    #[test]
    fn session_replaced_event_deserializes_restored_input() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "session_replaced",
            "session_id": "session-2",
            "cwd": "C:/work",
            "current_model": {
                "requested_id": null,
                "resolved_id": "opus",
                "display_name_short": "Opus",
                "display_name_long": "Claude Opus",
                "catalog_id": null,
                "supports_effort": false,
                "supported_effort_levels": [],
                "supports_fast_mode": false,
                "supports_auto_mode": false,
                "supports_adaptive_thinking": false,
                "is_authoritative": true
            },
            "available_models": [],
            "mode": null,
            "fast_mode_state": "on",
            "history_updates": [],
            "restored_input": "selected prompt"
        }))
        .expect("deserialize session replaced");

        let BridgeEvent::SessionReplaced { fast_mode_state, restored_input, .. } = decoded.event
        else {
            panic!("expected session_replaced");
        };
        assert_eq!(fast_mode_state, types::FastModeState::On);
        assert_eq!(restored_input.as_deref(), Some("selected prompt"));
    }

    #[test]
    fn mcp_set_servers_result_event_deserializes() {
        let json = serde_json::json!({
            "request_id": "req-mcp-set",
            "event": "mcp_set_servers_result",
            "session_id": "session-1",
            "result": {
                "added": ["docs"],
                "removed": ["plugin:Notion:notion"],
                "errors": {
                    "docs": "connection failed"
                }
            }
        });

        let decoded: EventEnvelope = serde_json::from_value(json).expect("deserialize");

        assert_eq!(
            decoded,
            EventEnvelope {
                request_id: Some("req-mcp-set".to_owned()),
                event: BridgeEvent::McpSetServersResult {
                    session_id: "session-1".to_owned(),
                    result: types::McpSetServersResult {
                        added: vec!["docs".to_owned()],
                        removed: vec!["plugin:Notion:notion".to_owned()],
                        errors: BTreeMap::from([(
                            "docs".to_owned(),
                            "connection failed".to_owned()
                        )]),
                    },
                },
            }
        );
    }

    #[test]
    fn mcp_snapshot_event_deserializes_source() {
        let json = serde_json::json!({
            "request_id": "req-mcp-snapshot",
            "event": "mcp_snapshot",
            "session_id": "session-1",
            "source": "reload_plugins",
            "servers": []
        });

        let decoded: EventEnvelope = serde_json::from_value(json).expect("deserialize");

        assert_eq!(
            decoded,
            EventEnvelope {
                request_id: Some("req-mcp-snapshot".to_owned()),
                event: BridgeEvent::McpSnapshot {
                    session_id: "session-1".to_owned(),
                    servers: Vec::new(),
                    auth_capabilities: types::McpAuthCapabilities::default(),
                    source: Some(types::McpSnapshotSource::ReloadPlugins),
                    error: None,
                },
            }
        );
    }

    #[test]
    fn mcp_snapshot_event_deserializes_auth_capabilities() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "mcp_snapshot",
            "session_id": "session-1",
            "servers": [],
            "auth_capabilities": {
                "authenticate": true,
                "clear_auth": false,
                "submit_oauth_callback_url": true
            }
        }))
        .expect("deserialize");

        let BridgeEvent::McpSnapshot { auth_capabilities, .. } = decoded.event else {
            panic!("expected mcp_snapshot");
        };
        assert_eq!(
            auth_capabilities,
            types::McpAuthCapabilities {
                authenticate: true,
                clear_auth: false,
                submit_oauth_callback_url: true,
            }
        );
    }

    #[test]
    fn rewind_targets_event_deserializes() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "rewind_targets",
            "session_id": "session-1",
            "targets": [
                {
                    "uuid": "user-2",
                    "first_text": "second prompt",
                    "input_text": "second prompt",
                    "previous_assistant_uuid": "assistant-1",
                    "resume_anchor_uuid": "assistant-1",
                    "index": 3
                },
                {
                    "uuid": "user-1",
                    "first_text": "first prompt",
                    "input_text": "first prompt",
                    "index": 0
                }
            ]
        }))
        .expect("deserialize rewind targets");

        assert_eq!(
            decoded.event,
            BridgeEvent::RewindTargets {
                session_id: "session-1".to_owned(),
                targets: vec![
                    types::RewindTarget {
                        uuid: "user-2".to_owned(),
                        first_text: "second prompt".to_owned(),
                        input_text: "second prompt".to_owned(),
                        index: 3,
                        previous_assistant_uuid: Some("assistant-1".to_owned()),
                        resume_anchor_uuid: Some("assistant-1".to_owned()),
                    },
                    types::RewindTarget {
                        uuid: "user-1".to_owned(),
                        first_text: "first prompt".to_owned(),
                        input_text: "first prompt".to_owned(),
                        index: 0,
                        previous_assistant_uuid: None,
                        resume_anchor_uuid: None,
                    },
                ],
                error: None,
            }
        );
    }

    #[test]
    fn user_dialog_response_command_serializes_selected_outcome() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::UserDialogResponse {
                session_id: "s1".to_owned(),
                request_id: "req-9".to_owned(),
                outcome: types::UserDialogOutcome::Selected {
                    option_id: "retry_fallback".to_owned(),
                },
            },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "user_dialog_response",
                "session_id": "s1",
                "request_id": "req-9",
                "outcome": { "outcome": "selected", "option_id": "retry_fallback" }
            })
        );
    }

    #[test]
    fn user_dialog_response_command_serializes_cancelled_outcome() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::UserDialogResponse {
                session_id: "s1".to_owned(),
                request_id: "req-9".to_owned(),
                outcome: types::UserDialogOutcome::Cancelled,
            },
        };

        let json = serde_json::to_value(&env).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "command": "user_dialog_response",
                "session_id": "s1",
                "request_id": "req-9",
                "outcome": { "outcome": "cancelled" }
            })
        );
    }

    #[test]
    fn user_dialog_request_event_deserializes_snake_case_payload() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "user_dialog_request",
            "session_id": "session-1",
            "request": {
                "request_id": "req-9",
                "dialog_kind": "refusal_fallback_prompt",
                "payload": {
                    "original_model": "claude-opus-4-8",
                    "fallback_model": "claude-sonnet-4-6",
                    "guidance_text": "This request was declined."
                },
                "options": [
                    { "option_id": "retry_fallback", "label": "Switch to claude-sonnet-4-6" },
                    { "option_id": "edit_prompt", "label": "Edit prompt and retry with claude-opus-4-8" }
                ]
            }
        }))
        .expect("deserialize user_dialog_request");

        let BridgeEvent::UserDialogRequest { session_id, request } = decoded.event else {
            panic!("expected user_dialog_request event");
        };
        assert_eq!(session_id, "session-1");
        assert_eq!(request.request_id, "req-9");
        assert_eq!(request.dialog_kind, "refusal_fallback_prompt");
        assert_eq!(request.payload.original_model, "claude-opus-4-8");
        assert_eq!(request.payload.fallback_model, "claude-sonnet-4-6");
        assert_eq!(request.payload.guidance_text.as_deref(), Some("This request was declined."));
        assert_eq!(request.payload.api_refusal_category, None);
        assert_eq!(request.options.len(), 2);
        assert_eq!(request.options[0].option_id, "retry_fallback");
    }

    #[test]
    fn turn_error_deserializes_target_api_error_status_shapes() {
        for (raw_status, expected) in [
            (Some(serde_json::json!(429)), Some(429)),
            (Some(serde_json::json!(529)), Some(529)),
            (Some(serde_json::Value::Null), None),
            (None, None),
        ] {
            let mut event = serde_json::json!({
                "event": "turn_error",
                "session_id": "session-1",
                "message": "service overloaded",
                "error_kind": "transient_service"
            });
            if let Some(raw_status) = raw_status {
                event["api_error_status"] = raw_status;
            }
            let decoded: EventEnvelope =
                serde_json::from_value(event).expect("deserialize turn error");

            let BridgeEvent::TurnError { api_error_status, error_kind, .. } = decoded.event else {
                panic!("expected turn_error event");
            };

            assert_eq!(api_error_status, expected);
            assert_eq!(error_kind.as_deref(), Some("transient_service"));
        }
    }

    #[test]
    fn fast_mode_update_deserializes_open_disabled_reason() {
        let decoded: EventEnvelope = serde_json::from_value(serde_json::json!({
            "event": "session_update",
            "session_id": "session-1",
            "update": {
                "type": "fast_mode_update",
                "fast_mode_state": "off",
                "fast_mode_disabled_reason": "future-reason"
            }
        }))
        .expect("deserialize fast mode update");

        let BridgeEvent::SessionUpdate {
            update:
                types::SessionUpdate::FastModeUpdate { fast_mode_state, fast_mode_disabled_reason },
            ..
        } = decoded.event
        else {
            panic!("expected fast mode update");
        };
        assert_eq!(fast_mode_state, types::FastModeState::Off);
        assert_eq!(fast_mode_disabled_reason.as_deref(), Some("future-reason"));
    }

    #[test]
    fn session_launch_settings_serializes_agent_progress_summaries() {
        let settings = SessionLaunchSettings {
            settings: Some(serde_json::json!({ "model": "haiku" })),
            agent_progress_summaries: Some(true),
            ..SessionLaunchSettings::default()
        };

        let json = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "settings": { "model": "haiku" },
                "agent_progress_summaries": true
            })
        );
    }
}
