// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

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
            Self::Prompt { .. } => "prompt",
            Self::CancelTurn { .. } => "cancel_turn",
            Self::SetModel { .. } => "set_model",
            Self::SetMode { .. } => "set_mode",
            Self::GenerateSessionTitle { .. } => "generate_session_title",
            Self::RenameSession { .. } => "rename_session",
            Self::NewSession { .. } => "new_session",
            Self::PermissionResponse { .. } => "permission_response",
            Self::QuestionResponse { .. } => "question_response",
            Self::ElicitationResponse { .. } => "elicitation_response",
            Self::GetStatusSnapshot { .. } => "get_status_snapshot",
            Self::GetContextUsage { .. } => "get_context_usage",
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
            | Self::Prompt { session_id, .. }
            | Self::CancelTurn { session_id }
            | Self::SetModel { session_id, .. }
            | Self::SetMode { session_id, .. }
            | Self::GenerateSessionTitle { session_id, .. }
            | Self::RenameSession { session_id, .. }
            | Self::PermissionResponse { session_id, .. }
            | Self::QuestionResponse { session_id, .. }
            | Self::ElicitationResponse { session_id, .. }
            | Self::GetStatusSnapshot { session_id }
            | Self::GetContextUsage { session_id }
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
            | Self::Prompt { .. }
            | Self::CancelTurn { .. }
            | Self::SetModel { .. }
            | Self::SetMode { .. }
            | Self::GenerateSessionTitle { .. }
            | Self::RenameSession { .. }
            | Self::NewSession { .. }
            | Self::ElicitationResponse { .. }
            | Self::GetStatusSnapshot { .. }
            | Self::GetContextUsage { .. }
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
        terminal_reason: Option<types::TerminalReason>,
    },
    SlashError {
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
        history_updates: Option<Vec<types::SessionUpdate>>,
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
    McpSnapshot {
        session_id: String,
        #[serde(default)]
        servers: Vec<types::McpServerStatus>,
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
            Self::ElicitationRequest { .. } => "elicitation_request",
            Self::ElicitationComplete { .. } => "elicitation_complete",
            Self::McpAuthRedirect { .. } => "mcp_auth_redirect",
            Self::McpOperationError { .. } => "mcp_operation_error",
            Self::TurnComplete { .. } => "turn_complete",
            Self::TurnError { .. } => "turn_error",
            Self::SlashError { .. } => "slash_error",
            Self::RuntimeReloadCompleted { .. } => "runtime_reload_completed",
            Self::RuntimeReloadFailed { .. } => "runtime_reload_failed",
            Self::SessionReplaced { .. } => "session_replaced",
            Self::Initialized { .. } => "initialized",
            Self::SessionsListed { .. } => "sessions_listed",
            Self::StatusSnapshot { .. } => "status_snapshot",
            Self::ContextUsage { .. } => "context_usage",
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
            | Self::ElicitationRequest { session_id, .. }
            | Self::ElicitationComplete { session_id, .. }
            | Self::McpAuthRedirect { session_id, .. }
            | Self::McpOperationError { session_id, .. }
            | Self::TurnComplete { session_id, .. }
            | Self::TurnError { session_id, .. }
            | Self::SlashError { session_id, .. }
            | Self::RuntimeReloadCompleted { session_id, .. }
            | Self::RuntimeReloadFailed { session_id, .. }
            | Self::SessionReplaced { session_id, .. }
            | Self::StatusSnapshot { session_id, .. }
            | Self::ContextUsage { session_id, .. }
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
            | Self::ElicitationRequest { .. }
            | Self::ElicitationComplete { .. }
            | Self::McpAuthRedirect { .. }
            | Self::McpOperationError { .. }
            | Self::TurnComplete { .. }
            | Self::TurnError { .. }
            | Self::SlashError { .. }
            | Self::RuntimeReloadCompleted { .. }
            | Self::RuntimeReloadFailed { .. }
            | Self::SessionReplaced { .. }
            | Self::Initialized { .. }
            | Self::SessionsListed { .. }
            | Self::StatusSnapshot { .. }
            | Self::ContextUsage { .. }
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
