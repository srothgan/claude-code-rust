// SPDX-License-Identifier: Apache-2.0

use crate::agent::types;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLaunchSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<super::model::EffortLevel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fork_session: bool,
    /// Boxed so the session commands that carry launch settings stay small enough for
    /// clippy's `large_futures` limit in futures that hold a command across an await.
    #[serde(flatten)]
    pub config: Box<SessionConfig>,
}

impl SessionLaunchSettings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.permission_mode.is_none()
            && self.effort.is_none()
            && self.agent.is_none()
            && self.name.is_none()
            && !self.fork_session
            && self.config.is_empty()
    }
}

/// Launch configuration kept for every session the process starts, unlike the initial-session
/// fields above. The app has already resolved paths to absolute ones and checked that files
/// were readable and JSON parsed; the bridge maps each field to its Agent SDK option.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<String>,
    /// A settings object for the SDK's flag-settings layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Map<String, serde_json::Value>>,
    /// The `mcpServers` object of each `--mcp-config`, in command-line order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_configs: Vec<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strict_mcp_config: bool,
    /// Replaces the default system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Appended to the system prompt, after the bridge's own instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
}

impl SessionConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.additional_directories.is_empty()
            && self.settings.is_none()
            && self.mcp_configs.is_empty()
            && !self.strict_mcp_config
            && self.system_prompt.is_none()
            && self.append_system_prompt.is_none()
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
    InspectSettings {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_settings_path: Option<String>,
    },
    MutateSetting {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_settings_path: Option<String>,
        mutation: super::settings::SettingsMutation,
    },
    Initialize {
        cwd: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metadata: BTreeMap<String, serde_json::Value>,
    },
    CreateSession {
        cwd: String,
        resume: Option<String>,
        #[serde(default)]
        continue_session: bool,
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
        message_uuid: String,
        chunks: Vec<types::PromptChunk>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        inline_pastes: Vec<String>,
    },
    SideQuestion {
        session_id: String,
        btw_id: String,
        question: String,
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
        effort: Option<String>,
    },
    SetThinking {
        session_id: String,
        enabled: Option<bool>,
    },
    SetAgent {
        session_id: String,
        agent: Option<String>,
    },
    SetUltracode {
        session_id: String,
        enabled: bool,
    },
    RefreshUltracode {
        session_id: String,
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
    ListSubagents {
        session_id: String,
    },
    GetSubagentMessages {
        session_id: String,
        agent_id: String,
    },
    StopSubagent {
        session_id: String,
        task_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_run_id: Option<String>,
    },
    GetRewindTargets {
        session_id: String,
    },
    ExportSessionHistory {
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
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        add_plugins: Vec<String>,
    },
    ReloadSkillsAndStyles {
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
            Self::SideQuestion { .. } => "side_question",
            Self::CancelTurn { .. } => "cancel_turn",
            Self::SetModel { .. } => "set_model",
            Self::SetMode { .. } => "set_mode",
            Self::SetEffort { .. } => "set_effort",
            Self::SetThinking { .. } => "set_thinking",
            Self::SetAgent { .. } => "set_agent",
            Self::SetUltracode { .. } => "set_ultracode",
            Self::RefreshUltracode { .. } => "refresh_ultracode",
            Self::SetFastMode { .. } => "set_fast_mode",
            Self::GenerateSessionTitle { .. } => "generate_session_title",
            Self::RenameSession { .. } => "rename_session",
            Self::NewSession { .. } => "new_session",
            Self::PermissionResponse { .. } => "permission_response",
            Self::QuestionResponse { .. } => "question_response",
            Self::UserDialogResponse { .. } => "user_dialog_response",
            Self::ElicitationResponse { .. } => "elicitation_response",
            Self::InspectSettings { .. } => "inspect_settings",
            Self::MutateSetting { .. } => "mutate_setting",
            Self::GetStatusSnapshot { .. } => "get_status_snapshot",
            Self::GetContextUsage { .. } => "get_context_usage",
            Self::GetUsage { .. } => "get_usage",
            Self::ListSubagents { .. } => "list_subagents",
            Self::GetSubagentMessages { .. } => "get_subagent_messages",
            Self::StopSubagent { .. } => "stop_subagent",
            Self::GetRewindTargets { .. } => "get_rewind_targets",
            Self::ExportSessionHistory { .. } => "export_session_history",
            Self::Rewind { .. } => "rewind",
            Self::ReloadPlugins { .. } => "reload_plugins",
            Self::ReloadSkillsAndStyles { .. } => "reload_skills_and_styles",
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
            | Self::SideQuestion { session_id, .. }
            | Self::CancelTurn { session_id }
            | Self::SetModel { session_id, .. }
            | Self::SetMode { session_id, .. }
            | Self::SetEffort { session_id, .. }
            | Self::SetThinking { session_id, .. }
            | Self::SetAgent { session_id, .. }
            | Self::SetUltracode { session_id, .. }
            | Self::RefreshUltracode { session_id }
            | Self::SetFastMode { session_id, .. }
            | Self::GenerateSessionTitle { session_id, .. }
            | Self::RenameSession { session_id, .. }
            | Self::PermissionResponse { session_id, .. }
            | Self::QuestionResponse { session_id, .. }
            | Self::UserDialogResponse { session_id, .. }
            | Self::ElicitationResponse { session_id, .. }
            | Self::InspectSettings { session_id, .. }
            | Self::MutateSetting { session_id, .. }
            | Self::GetStatusSnapshot { session_id }
            | Self::GetContextUsage { session_id }
            | Self::GetUsage { session_id }
            | Self::ListSubagents { session_id }
            | Self::GetSubagentMessages { session_id, .. }
            | Self::StopSubagent { session_id, .. }
            | Self::GetRewindTargets { session_id }
            | Self::ExportSessionHistory { session_id }
            | Self::Rewind { session_id, .. }
            | Self::ReloadPlugins { session_id, .. }
            | Self::ReloadSkillsAndStyles { session_id }
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
            | Self::SideQuestion { .. }
            | Self::CancelTurn { .. }
            | Self::SetModel { .. }
            | Self::SetMode { .. }
            | Self::SetEffort { .. }
            | Self::SetThinking { .. }
            | Self::SetAgent { .. }
            | Self::SetUltracode { .. }
            | Self::RefreshUltracode { .. }
            | Self::SetFastMode { .. }
            | Self::GenerateSessionTitle { .. }
            | Self::RenameSession { .. }
            | Self::NewSession { .. }
            | Self::UserDialogResponse { .. }
            | Self::ElicitationResponse { .. }
            | Self::InspectSettings { .. }
            | Self::MutateSetting { .. }
            | Self::GetStatusSnapshot { .. }
            | Self::GetContextUsage { .. }
            | Self::GetUsage { .. }
            | Self::ListSubagents { .. }
            | Self::GetSubagentMessages { .. }
            | Self::StopSubagent { .. }
            | Self::GetRewindTargets { .. }
            | Self::ExportSessionHistory { .. }
            | Self::Rewind { .. }
            | Self::ReloadPlugins { .. }
            | Self::ReloadSkillsAndStyles { .. }
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
pub struct SideQuestionMetadata {
    pub synthetic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_fallback: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BridgeEvent {
    SettingsResult {
        session_id: String,
        result: super::settings::SettingsResult,
    },
    Connected {
        session_id: String,
        cwd: String,
        current_model: types::CurrentModel,
        #[serde(default)]
        available_models: Vec<types::AvailableModel>,
        mode: Option<types::ModeState>,
        fast_mode_state: types::FastModeState,
        fast_mode_disabled_reason: Option<String>,
        #[serde(default, deserialize_with = "types::deserialize_ultracode")]
        ultracode: Option<types::UltracodeState>,
        history_updates: Option<Vec<types::SessionUpdate>>,
    },
    AuthRequired {
        method_name: String,
        method_description: String,
    },
    ConnectionFailed {
        message: String,
        #[serde(default)]
        startup_failure: Option<types::StartupFailure>,
    },
    // Presentation metadata (SDK timestamps and per-turn timings) travels in the typed update.
    SessionUpdate {
        session_id: String,
        update: types::SessionUpdate,
    },
    BtwResult {
        session_id: String,
        btw_id: String,
        question: String,
        answer: String,
        metadata: SideQuestionMetadata,
    },
    BtwFailed {
        session_id: String,
        btw_id: String,
        question: String,
        error: String,
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
    InteractionCancelled {
        session_id: String,
        interaction_id: String,
    },
    ElicitationRequest {
        session_id: String,
        request: types::ElicitationRequest,
    },
    ElicitationSettled {
        session_id: String,
        settlement: ElicitationSettlement,
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
    UserMessageQueued {
        session_id: String,
        message_uuid: String,
    },
    UserMessageStarted {
        session_id: String,
        message_uuid: String,
        source: types::UserMessageStartSource,
    },
    UserMessageRejected {
        session_id: String,
        message_uuid: String,
        reason: String,
    },
    TurnInterruptReceipt {
        session_id: String,
        still_queued: Vec<String>,
    },
    TurnComplete {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queued_turn_count: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_reason: Option<types::TerminalReason>,
    },
    TurnError {
        session_id: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queued_turn_count: Option<usize>,
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
    PluginInventory {
        session_id: String,
        plugins: Vec<types::SessionPlugin>,
    },
    RuntimeReloadCompleted {
        session_id: String,
        #[serde(default)]
        error_count: u32,
        #[serde(default)]
        reconciliation_error: Option<String>,
    },
    RuntimeReloadHeld {
        session_id: String,
        cache_impact: types::RuntimeReloadCacheImpact,
        #[serde(default)]
        error_count: u32,
    },
    RuntimeReloadFailed {
        session_id: String,
        message: String,
    },
    SkillsAndStylesReloaded {
        session_id: String,
        skills: types::ReloadOutcome,
        output_styles: types::ReloadOutcome,
    },
    SkillsAndStylesReloadExpired {
        session_id: String,
        after_ms: u64,
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
        #[serde(default, deserialize_with = "types::deserialize_ultracode")]
        ultracode: Option<types::UltracodeState>,
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
    SubagentsListed {
        session_id: String,
        agent_ids: Vec<String>,
        error: Option<String>,
    },
    SubagentMessages {
        session_id: String,
        agent_id: String,
        messages: Vec<types::SubagentMessage>,
        error: Option<String>,
    },
    SubagentStopResult {
        session_id: String,
        task_id: String,
        run_id: Option<String>,
        error: Option<String>,
    },
    RewindTargets {
        session_id: String,
        targets: Vec<types::RewindTarget>,
        error: Option<String>,
    },
    /// One chunk of a session's complete saved history, in resume order.
    SessionHistoryExport {
        session_id: String,
        updates: Vec<types::SessionUpdate>,
        last: bool,
        #[serde(default)]
        omitted_updates: u64,
        #[serde(default)]
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
            Self::BtwResult { .. } => "btw_result",
            Self::BtwFailed { .. } => "btw_failed",
            Self::PermissionRequest { .. } => "permission_request",
            Self::QuestionRequest { .. } => "question_request",
            Self::UserDialogRequest { .. } => "user_dialog_request",
            Self::InteractionCancelled { .. } => "interaction_cancelled",
            Self::ElicitationRequest { .. } => "elicitation_request",
            Self::ElicitationSettled { .. } => "elicitation_settled",
            Self::McpAuthRedirect { .. } => "mcp_auth_redirect",
            Self::McpOperationError { .. } => "mcp_operation_error",
            Self::McpSetServersResult { .. } => "mcp_set_servers_result",
            Self::UserMessageQueued { .. } => "user_message_queued",
            Self::UserMessageStarted { .. } => "user_message_started",
            Self::UserMessageRejected { .. } => "user_message_rejected",
            Self::TurnInterruptReceipt { .. } => "turn_interrupt_receipt",
            Self::TurnComplete { .. } => "turn_complete",
            Self::TurnError { .. } => "turn_error",
            Self::SlashError { .. } => "slash_error",
            Self::SessionResumeFailed { .. } => "session_resume_failed",
            Self::RuntimeReloadCompleted { .. } => "runtime_reload_completed",
            Self::PluginInventory { .. } => "plugin_inventory",
            Self::RuntimeReloadHeld { .. } => "runtime_reload_held",
            Self::RuntimeReloadFailed { .. } => "runtime_reload_failed",
            Self::SkillsAndStylesReloaded { .. } => "skills_and_styles_reloaded",
            Self::SkillsAndStylesReloadExpired { .. } => "skills_and_styles_reload_expired",
            Self::SessionReplaced { .. } => "session_replaced",
            Self::Initialized { .. } => "initialized",
            Self::SessionsListed { .. } => "sessions_listed",
            Self::SettingsResult { .. } => "settings_result",
            Self::StatusSnapshot { .. } => "status_snapshot",
            Self::ContextUsage { .. } => "context_usage",
            Self::UsageSnapshot { .. } => "usage_snapshot",
            Self::SubagentsListed { .. } => "subagents_listed",
            Self::SubagentMessages { .. } => "subagent_messages",
            Self::SubagentStopResult { .. } => "subagent_stop_result",
            Self::RewindTargets { .. } => "rewind_targets",
            Self::SessionHistoryExport { .. } => "session_history_export",
            Self::RewindResult { .. } => "rewind_result",
            Self::McpSnapshot { .. } => "mcp_snapshot",
        }
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::Connected { session_id, .. }
            | Self::SessionUpdate { session_id, .. }
            | Self::BtwResult { session_id, .. }
            | Self::BtwFailed { session_id, .. }
            | Self::PermissionRequest { session_id, .. }
            | Self::QuestionRequest { session_id, .. }
            | Self::UserDialogRequest { session_id, .. }
            | Self::InteractionCancelled { session_id, .. }
            | Self::ElicitationRequest { session_id, .. }
            | Self::ElicitationSettled { session_id, .. }
            | Self::McpAuthRedirect { session_id, .. }
            | Self::McpOperationError { session_id, .. }
            | Self::McpSetServersResult { session_id, .. }
            | Self::UserMessageQueued { session_id, .. }
            | Self::UserMessageStarted { session_id, .. }
            | Self::UserMessageRejected { session_id, .. }
            | Self::TurnInterruptReceipt { session_id, .. }
            | Self::TurnComplete { session_id, .. }
            | Self::TurnError { session_id, .. }
            | Self::SlashError { session_id, .. }
            | Self::SessionResumeFailed { session_id, .. }
            | Self::RuntimeReloadCompleted { session_id, .. }
            | Self::PluginInventory { session_id, .. }
            | Self::RuntimeReloadHeld { session_id, .. }
            | Self::RuntimeReloadFailed { session_id, .. }
            | Self::SkillsAndStylesReloaded { session_id, .. }
            | Self::SkillsAndStylesReloadExpired { session_id, .. }
            | Self::SessionReplaced { session_id, .. }
            | Self::SettingsResult { session_id, .. }
            | Self::StatusSnapshot { session_id, .. }
            | Self::ContextUsage { session_id, .. }
            | Self::UsageSnapshot { session_id, .. }
            | Self::SubagentsListed { session_id, .. }
            | Self::SubagentMessages { session_id, .. }
            | Self::SubagentStopResult { session_id, .. }
            | Self::RewindTargets { session_id, .. }
            | Self::SessionHistoryExport { session_id, .. }
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
            | Self::BtwResult { .. }
            | Self::BtwFailed { .. }
            | Self::UserDialogRequest { .. }
            | Self::InteractionCancelled { .. }
            | Self::ElicitationRequest { .. }
            | Self::ElicitationSettled { .. }
            | Self::McpAuthRedirect { .. }
            | Self::McpOperationError { .. }
            | Self::McpSetServersResult { .. }
            | Self::UserMessageQueued { .. }
            | Self::UserMessageStarted { .. }
            | Self::UserMessageRejected { .. }
            | Self::TurnInterruptReceipt { .. }
            | Self::TurnComplete { .. }
            | Self::TurnError { .. }
            | Self::SlashError { .. }
            | Self::SessionResumeFailed { .. }
            | Self::RuntimeReloadCompleted { .. }
            | Self::PluginInventory { .. }
            | Self::RuntimeReloadHeld { .. }
            | Self::RuntimeReloadFailed { .. }
            | Self::SkillsAndStylesReloaded { .. }
            | Self::SkillsAndStylesReloadExpired { .. }
            | Self::SessionReplaced { .. }
            | Self::Initialized { .. }
            | Self::SessionsListed { .. }
            | Self::SettingsResult { .. }
            | Self::StatusSnapshot { .. }
            | Self::ContextUsage { .. }
            | Self::UsageSnapshot { .. }
            | Self::SubagentsListed { .. }
            | Self::SubagentMessages { .. }
            | Self::SubagentStopResult { .. }
            | Self::RewindTargets { .. }
            | Self::SessionHistoryExport { .. }
            | Self::RewindResult { .. }
            | Self::McpSnapshot { .. } => None,
        }
    }
}

/// Correlates SDK callback settlement with its original host request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElicitationSettlement {
    pub request_id: String,
    #[serde(default)]
    pub session_closing: bool,
}

#[cfg(test)]
mod tests {
    use super::{
        BridgeCommand, BridgeEvent, CommandEnvelope, EventEnvelope, SessionLaunchSettings,
    };
    use crate::agent::types;
    use std::collections::BTreeMap;

    #[test]
    fn saved_scheduler_and_queue_sections_survive_the_wire_without_losing_item_identity() {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/tool-result-times.json"))
                .expect("independent SDK result fixtures");
        for fixture in fixtures.as_array().expect("fixtures") {
            let event: EventEnvelope = serde_json::from_value(serde_json::json!({
                "event": "session_update", "session_id": "times",
                "update": {"type": "tool_call_update", "tool_call_update": {
                    "tool_call_id": fixture["name"], "fields": {
                        "output_metadata": {"presentation": fixture["presentation"]}
                    }
                }}
            }))
            .expect("typed result wire event");
            let encoded = serde_json::to_value(event).expect("wire round trip");
            assert_eq!(
                encoded["update"]["tool_call_update"]["fields"]["output_metadata"]["presentation"],
                fixture["presentation"]
            );
        }
    }

    #[test]
    fn notification_payload_round_trips_original_sdk_metadata_and_replay_provenance() {
        let payload = serde_json::json!({
            "event": "session_update", "session_id": "s1",
            "update": { "type": "notification_update", "replay": true,
                "notification": { "origin": "sdk_notice", "session_id": "s1", "uuid": "n1",
                    "key": "replaceable", "text": "Ready", "priority": "immediate", "color": "yellow", "timeout_ms": 5000.0 }
            }
        });
        let envelope: EventEnvelope =
            serde_json::from_value(payload.clone()).expect("notification event");
        assert_eq!(serde_json::to_value(envelope).expect("serialize notification"), payload);
    }

    #[test]
    fn prompt_command_serializes_host_message_uuid() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::Prompt {
                session_id: "s1".to_owned(),
                message_uuid: "message-1".to_owned(),
                chunks: vec![types::PromptChunk {
                    kind: "text".to_owned(),
                    value: serde_json::json!("next"),
                }],
                inline_pastes: vec!["pasted text".to_owned()],
            },
        };

        assert_eq!(
            serde_json::to_value(env).expect("serialize"),
            serde_json::json!({
                "command": "prompt",
                "session_id": "s1",
                "message_uuid": "message-1",
                "chunks": [{ "kind": "text", "value": "next" }],
                "inline_pastes": ["pasted text"]
            })
        );
    }

    #[test]
    fn side_question_command_preserves_the_complete_question() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::SideQuestion {
                session_id: "s1".to_owned(),
                btw_id: "btw-1".to_owned(),
                question: "Why  two spaces?\nAnd this line?".to_owned(),
            },
        };

        assert_eq!(
            serde_json::to_value(env).expect("serialize"),
            serde_json::json!({
                "command": "side_question",
                "session_id": "s1",
                "btw_id": "btw-1",
                "question": "Why  two spaces?\nAnd this line?"
            })
        );
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
    fn rewind_command_serializes_snake_case() {
        let env = CommandEnvelope {
            request_id: None,
            command: BridgeCommand::Rewind {
                session_id: "s1".to_owned(),
                target_user_message_id: "user-1".to_owned(),
                restore_mode: types::RewindRestoreMode::Both,
                launch_settings: SessionLaunchSettings {
                    model: Some("haiku".to_owned()),
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
                    "model": "haiku"
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
    fn session_launch_choices_roundtrip() {
        let settings = SessionLaunchSettings {
            model: Some("haiku".to_owned()),
            permission_mode: Some("plan".to_owned()),
            effort: Some(super::super::model::EffortLevel::Max),
            agent: Some("reviewer".to_owned()),
            name: Some("Release prep".to_owned()),
            fork_session: true,
            config: Box::new(super::SessionConfig {
                additional_directories: vec!["/work/shared".to_owned()],
                settings: serde_json::from_str(r#"{"model":"haiku"}"#).ok(),
                mcp_configs: serde_json::from_str(r#"[{"docs":{"command":"docs-mcp"}}]"#)
                    .expect("MCP servers"),
                strict_mcp_config: true,
                system_prompt: Some("You are a release bot.".to_owned()),
                append_system_prompt: Some("End with DONE.".to_owned()),
            }),
        };
        let json = serde_json::to_value(&settings).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "model": "haiku", "permission_mode": "plan", "effort": "max", "agent": "reviewer",
                "name": "Release prep", "fork_session": true,
                "additional_directories": ["/work/shared"],
                "settings": {"model": "haiku"},
                "mcp_configs": [{"docs": {"command": "docs-mcp"}}],
                "strict_mcp_config": true, "system_prompt": "You are a release bot.",
                "append_system_prompt": "End with DONE."
            })
        );
        assert_eq!(
            serde_json::from_value::<SessionLaunchSettings>(json).expect("deserialize"),
            settings
        );
    }

    #[test]
    fn rate_limit_envelope_preserves_enabled_false_separately_from_unknown() {
        for enabled in [Some(true), Some(false), None] {
            let mut raw = serde_json::json!({
                "event": "session_update", "session_id": "session",
                "update": {"type": "rate_limit_update", "status": "allowed_warning", "is_using_overage": false}
            });
            if let Some(enabled) = enabled {
                raw["update"]["overage_enabled"] = serde_json::json!(enabled);
            }
            let envelope: EventEnvelope = serde_json::from_value(raw).expect("rate limit envelope");
            let BridgeEvent::SessionUpdate {
                update:
                    types::SessionUpdate::RateLimitUpdate { overage_enabled, is_using_overage, .. },
                ..
            } = envelope.event
            else {
                panic!("rate limit event")
            };
            assert_eq!(overage_enabled, enabled);
            assert_eq!(is_using_overage, Some(false));
        }
    }
}

#[cfg(test)]
#[path = "command_wire_tests.rs"]
mod command_wire_tests;

#[cfg(test)]
#[path = "event_wire_tests.rs"]
mod event_wire_tests;

#[cfg(test)]
#[path = "settings_wire_tests.rs"]
mod settings_wire_tests;
