// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use super::model::{
    AgentOutputMetadata, ApiRetryError, AvailableAgent, AvailableCommand, AvailableModel,
    BashOutputMetadata, CompactionFailureCode, CompactionResult, CompactionTrigger, CurrentModel,
    EffortLevel, FastModeState, McpAuthCapabilities, McpServerConnectionStatus, McpServerInfo,
    McpServerOrgMaxPermission, McpServerProvenance, McpServerStatus, McpServerStatusConfig,
    McpServerToolPermissionPolicy, McpServerToolPolicy, McpTool, McpToolAnnotations, MessageOrigin,
    PermissionDisplay, QuestionAnnotation, QuestionOption, QuestionPrompt, RateLimitStatus,
    RateLimitUpdate, ReadOutputMetadata, RefusalFallbackPayload, RewindFilesResult,
    RewindRestoreMode, RewindResultStatus, RewindTarget, RuntimeSessionState, SessionStatus,
    SkillOutputMetadata, SubagentRetryUpdate, SystemNoticeSeverity, TaskItem, TaskMetadata,
    TaskStateUpdate, TaskStatus, TaskUpdateSource, ToolNonExecutionMetadata, ToolOutputMetadata,
    TranscriptRetraction, TranscriptRetractionReason, UltracodeState, UserDialogOption,
    WebFetchArtifactReadMetadata, WebFetchOutputMetadata,
};

/// Startup reasons are an open SDK vocabulary; unknown tokens must survive the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupFailure {
    pub reason: String,
    #[serde(default)]
    pub errors: Vec<String>,
}

pub(crate) fn deserialize_ultracode<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<UltracodeState>, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(raw) = raw else { return Ok(None) };
    match serde_json::from_value(raw.clone()) {
        Ok(state) => Ok(Some(state)),
        Err(error) => {
            tracing::warn!(target: crate::logging::targets::APP_SESSION,
                event_name = "invalid_ultracode_snapshot", raw = %raw, error = %error,
                "bridge supplied an invalid Ultracode snapshot");
            Ok(None)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeState {
    pub current_mode_id: String,
    pub current_mode_name: String,
    pub available_modes: Vec<ModeInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredUsageWindow {
    pub utilization: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredModelUsageWindow {
    pub display_name: String,
    pub utilization: f64,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredExtraUsage {
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredSessionUsage {
    pub total_cost_usd: Option<f64>,
    pub total_api_duration_ms: Option<f64>,
    pub total_duration_ms: Option<f64>,
    pub total_lines_added: Option<f64>,
    pub total_lines_removed: Option<f64>,
    pub model_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredActivityWindow {
    pub request_count: u64,
    pub session_count: u64,
    /// None means unavailable; an empty list means the SDK returned no recommendations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommendations: Option<Vec<StructuredUsageRecommendation>>,
    #[serde(default)]
    pub behaviors: Vec<StructuredBehaviorAttribution>,
    #[serde(default)]
    pub agents: Vec<StructuredNamedAttribution>,
    #[serde(default)]
    pub skills: Vec<StructuredNamedAttribution>,
    #[serde(default)]
    pub plugins: Vec<StructuredNamedAttribution>,
    #[serde(default)]
    pub mcp_servers: Vec<StructuredNamedAttribution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredUsageRecommendation {
    pub key: String,
    /// Share of weighted local usage affected, not projected savings.
    pub pct: f64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredBehaviorAttribution {
    pub key: String,
    pub pct: f64,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredNamedAttribution {
    pub name: String,
    pub pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredUsageSnapshot {
    pub subscription_type: Option<String>,
    pub rate_limits_available: Option<bool>,
    pub five_hour: Option<StructuredUsageWindow>,
    pub seven_day: Option<StructuredUsageWindow>,
    pub seven_day_oauth_apps: Option<StructuredUsageWindow>,
    pub seven_day_opus: Option<StructuredUsageWindow>,
    pub seven_day_sonnet: Option<StructuredUsageWindow>,
    #[serde(default)]
    pub model_scoped: Vec<StructuredModelUsageWindow>,
    pub extra_usage: Option<StructuredExtraUsage>,
    pub session: Option<StructuredSessionUsage>,
    pub activity_day: Option<StructuredActivityWindow>,
    pub activity_week: Option<StructuredActivityWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsParseErrorUpdate {
    pub file: Option<String>,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPlugin {
    pub name: String,
    pub path: String,
    pub source: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReloadCacheImpact {
    #[serde(default)]
    pub mcp_servers_added: Vec<String>,
    #[serde(default)]
    pub mcp_servers_removed: Vec<String>,
    pub lsp_tool_change: Option<String>,
    #[serde(default)]
    pub invalid_server_name_count: usize,
}

/// How one half of a skills-and-styles reload ended, as the bridge reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ReloadOutcome {
    Reloaded { names: Vec<String> },
    Failed { message: String },
    TimedOut { after_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum CompactionUpdate {
    Started,
    Boundary {
        trigger: CompactionTrigger,
        pre_tokens: u64,
        post_tokens: Option<u64>,
        duration_ms: Option<u64>,
    },
    Finished {
        result: CompactionResult,
        error_code: Option<CompactionFailureCode>,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Image { mime_type: Option<String>, uri: Option<String>, data: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_field_names)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub source_message_uuid: Option<String>,
    pub content: Vec<ToolCallContent>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<String>,
    pub output_metadata: Option<ToolOutputMetadata>,
    pub task_metadata: Option<TaskMetadata>,
    pub locations: Vec<ToolLocation>,
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    pub source_message_uuid: Option<String>,
    pub fields: ToolCallUpdateFields,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolCallUpdateFields {
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub content: Option<Vec<ToolCallContent>>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<String>,
    pub output_metadata: Option<ToolOutputMetadata>,
    pub task_metadata: Option<TaskMetadata>,
    pub locations: Option<Vec<ToolLocation>>,
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLocation {
    pub path: String,
    pub line: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Content {
        content: ContentBlock,
    },
    Diff {
        old_path: String,
        new_path: String,
        old: String,
        new: String,
        repository: Option<String>,
    },
    McpResource {
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
        blob_saved_to: Option<String>,
    },
    ResourceLink {
        uri: String,
        name: String,
        title: Option<String>,
        description: Option<String>,
        mime_type: Option<String>,
        size: Option<u64>,
        annotations: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentMessageRole {
    User,
    Assistant,
    Tool,
    /// A reminder the SDK added to the subagent's context, without its XML wrapper.
    SystemReminder,
}

/// One readable entry of a saved subagent transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentMessage {
    pub role: SubagentMessageRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionUpdate {
    MessageMetadata {
        role: String,
        timestamp: String,
        source_message_uuid: Option<String>,
    },
    TurnTiming {
        duration_ms: f64,
        api_duration_ms: Option<f64>,
    },
    ConversationReset {
        new_conversation_id: String,
        trigger: Option<String>,
        timestamp: Option<String>,
        user_message_uuid: Option<String>,
    },
    AgentMessageChunk {
        content: ContentBlock,
        source_message_uuid: Option<String>,
    },
    UserMessageChunk {
        content: ContentBlock,
        source_message_uuid: Option<String>,
    },
    ExternalMessageUpdate {
        content: String,
        source_message_uuid: Option<String>,
        origin: MessageOrigin,
    },
    AgentResponseStarted,
    AgentActivityUpdate {
        phase: super::model::AgentActivityPhase,
    },
    ToolCall {
        tool_call: ToolCall,
    },
    ToolCallUpdate {
        tool_call_update: ToolCallUpdate,
    },
    TranscriptRetraction {
        #[serde(flatten)]
        retraction: TranscriptRetraction,
    },
    TaskStateUpdate {
        #[serde(flatten)]
        update: TaskStateUpdate,
    },
    AvailableCommandsUpdate {
        commands: Vec<AvailableCommand>,
        source: Option<String>,
        generation: Option<u64>,
    },
    AvailableAgentsUpdate {
        agents: Vec<AvailableAgent>,
    },
    ModeStateUpdate {
        mode: ModeState,
    },
    CurrentModelUpdate {
        current_model: CurrentModel,
    },
    ConfigOptionUpdate {
        option_id: String,
        value: serde_json::Value,
    },
    UltracodeUpdate {
        #[serde(default, deserialize_with = "deserialize_ultracode")]
        ultracode: Option<UltracodeState>,
    },
    FastModeUpdate {
        fast_mode_state: FastModeState,
        fast_mode_disabled_reason: Option<String>,
    },
    RateLimitUpdate {
        status: RateLimitStatus,
        error_code: Option<String>,
        resets_at: Option<f64>,
        utilization: Option<f64>,
        rate_limit_type: Option<String>,
        limit_scope: Option<String>,
        overage_enabled: Option<bool>,
        overage_status: Option<RateLimitStatus>,
        overage_resets_at: Option<f64>,
        overage_disabled_reason: Option<String>,
        is_using_overage: Option<bool>,
        surpassed_threshold: Option<f64>,
        can_user_purchase_credits: Option<bool>,
        has_chargeable_saved_payment_method: Option<bool>,
    },
    ApiRetryUpdate {
        attempt: u64,
        max_retries: u64,
        #[serde(deserialize_with = "deserialize_retry_delay_ms")]
        retry_delay_ms: f64,
        error_status: Option<u16>,
        error: ApiRetryError,
    },
    PromptSuggestionUpdate {
        suggestion: String,
    },
    SessionTitleUpdate {
        title: String,
    },
    RuntimeSessionStateUpdate {
        state: RuntimeSessionState,
    },
    SettingsParseError {
        file: Option<String>,
        path: String,
        message: String,
    },
    SessionStatusUpdate {
        status: SessionStatus,
    },
    NotificationUpdate {
        notification: super::notifications::SdkNotification,
        replay: bool,
    },
    SystemNoticeUpdate {
        severity: SystemNoticeSeverity,
        message: String,
    },
    CompactionUpdate {
        #[serde(flatten)]
        update: CompactionUpdate,
    },
}

fn deserialize_retry_delay_ms<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let delay = f64::deserialize(deserializer)?;
    if !delay.is_finite() || delay < 0.0 {
        return Err(serde::de::Error::custom(
            "retry_delay_ms must be a finite non-negative number",
        ));
    }
    Ok(delay)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub tool_call: ToolCall,
    pub options: Vec<PermissionOption>,
    pub display: Option<PermissionDisplay>,
    pub mcp_server: Option<McpServerProvenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequest {
    /// Trusted-source inactivity interval; None waits for explicit confirmation.
    #[serde(default)]
    pub idle_timeout_ms: Option<u64>,
    pub tool_call: ToolCall,
    pub prompt: QuestionPrompt,
    pub question_index: u64,
    pub total_questions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionOutcome {
    Selected { option_id: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum QuestionOutcome {
    Answered { selected_option_ids: Vec<String>, annotation: Option<QuestionAnnotation> },
    Cancelled,
}

/// Host answer to a `refusal_fallback_prompt` user dialog. `option_id` carries
/// the dialog's string-enum choice (`retry_fallback` | `edit_prompt`);
/// declining maps to `Cancelled` (the CLI default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum UserDialogOutcome {
    Selected { option_id: String },
    Cancelled,
}

/// Turn-level `request_user_dialog` request the bridge emits for a refusal
/// fallback. Keyed by a bridge-generated `request_id` (no tool-call anchor).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDialogRequest {
    pub request_id: String,
    pub dialog_kind: String,
    pub payload: RefusalFallbackPayload,
    pub options: Vec<UserDialogOption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationMode {
    Form,
    Url,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElicitationRequest {
    pub request_id: String,
    pub server_name: String,
    pub message: String,
    pub mode: ElicitationMode,
    pub url: Option<String>,
    pub elicitation_id: Option<String>,
    pub requested_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub field_order: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    pub content: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAuthRedirect {
    pub server_name: String,
    pub auth_url: String,
    pub requires_user_action: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOperationError {
    pub server_name: Option<String>,
    pub operation: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalReason {
    BlockingLimit,
    RapidRefillBreaker,
    PromptTooLong,
    ImageError,
    ModelError,
    ApiError,
    MalformedToolUseExhausted,
    AbortedStreaming,
    AbortedTools,
    StopHookPrevented,
    HookStopped,
    ToolDeferred,
    MaxTurns,
    BackgroundRequested,
    BudgetExhausted,
    StructuredOutputRetryExhausted,
    ToolDeferredUnavailable,
    TurnSetupFailed,
    Completed,
}

impl TerminalReason {
    #[must_use]
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::BlockingLimit => "blocking_limit",
            Self::RapidRefillBreaker => "rapid_refill_breaker",
            Self::PromptTooLong => "prompt_too_long",
            Self::ImageError => "image_error",
            Self::ModelError => "model_error",
            Self::ApiError => "api_error",
            Self::MalformedToolUseExhausted => "malformed_tool_use_exhausted",
            Self::AbortedStreaming => "aborted_streaming",
            Self::AbortedTools => "aborted_tools",
            Self::StopHookPrevented => "stop_hook_prevented",
            Self::HookStopped => "hook_stopped",
            Self::ToolDeferred => "tool_deferred",
            Self::MaxTurns => "max_turns",
            Self::BackgroundRequested => "background_requested",
            Self::BudgetExhausted => "budget_exhausted",
            Self::StructuredOutputRetryExhausted => "structured_output_retry_exhausted",
            Self::ToolDeferredUnavailable => "tool_deferred_unavailable",
            Self::TurnSetupFailed => "turn_setup_failed",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct AgentCapabilities {
    pub prompt_image: bool,
    pub prompt_embedded_context: bool,
    pub supports_session_listing: bool,
    pub supports_resume_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializeResult {
    pub agent_name: String,
    pub agent_version: String,
    pub auth_methods: Vec<AuthMethod>,
    pub capabilities: AgentCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListEntry {
    pub session_id: String,
    pub summary: String,
    pub last_modified_ms: u64,
    pub file_size_bytes: u64,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub custom_title: Option<String>,
    pub first_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInit {
    pub session_id: String,
    pub model_name: String,
    pub mode: Option<ModeState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptChunk {
    pub kind: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserMessageStartSource {
    CommandLifecycle,
    StreamEvent,
    Assistant,
    Result,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub organization: Option<String>,
    pub subscription_type: Option<String>,
    pub token_source: Option<String>,
    pub api_key_source: Option<String>,
    pub api_provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_timeout_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        always_load: Option<bool>,
    },
    Sse {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<McpServerToolPolicy>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_timeout_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        always_load: Option<bool>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<McpServerToolPolicy>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_timeout_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        always_load: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpSetServersResult {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSnapshotSource {
    ReloadPlugins,
    McpStatus,
    McpSetServers,
    Init,
}

#[cfg(test)]
mod tests {
    use super::{
        McpServerOrgMaxPermission, McpServerStatus, McpServerStatusConfig,
        McpServerToolPermissionPolicy, TerminalReason,
    };

    #[test]
    fn mcp_status_deserializes_latest_config_fields() {
        use crate::agent::model::McpServerConnectionStatus;
        for (token, status) in [
            ("connected", McpServerConnectionStatus::Connected),
            ("needs-auth", McpServerConnectionStatus::NeedsAuth),
        ] {
            for load in [Some(true), Some(false), None] {
                let mut payload = serde_json::json!({
                    "name":"notion","status":token,"scope":"project","source":".mcp.json",
                    "config":{"type":"http","url":"https://mcp.notion.com/mcp","headers":{"Authorization":"Bearer token"},
                        "tools":[{"name":"search","permission_policy":"always_ask","org_max_permission":"ask"},{"name":"lookup"},{"name":"write","org_max_permission":"blocked"},{"name":"remove","permission_policy":"always_deny","org_max_permission":"blocked"}],
                        "timeout":5000,"request_timeout_ms":30000}
                });
                if let Some(load) = load {
                    payload["config"]["always_load"] = serde_json::json!(load);
                    payload["tools"] = serde_json::json!([{"name":"runtime-only","description":"Discovered runtime tool","annotations":{"read_only":true,"destructive":false}}]);
                }
                let server: McpServerStatus = serde_json::from_value(payload).expect("MCP status");
                assert_eq!(server.name, "notion");
                assert_eq!(server.status, status);
                assert_eq!(serde_json::to_value(server.status).unwrap(), serde_json::json!(token));
                assert_eq!(server.scope.as_deref(), Some("project"));
                assert_eq!(server.source.as_deref(), Some(".mcp.json"));
                if load.is_some() {
                    let runtime = server
                        .tools
                        .iter()
                        .find(|tool| tool.name == "runtime-only")
                        .expect("runtime tool");
                    assert_eq!(runtime.description.as_deref(), Some("Discovered runtime tool"));
                    let annotations = runtime.annotations.as_ref().unwrap();
                    assert_eq!(
                        (annotations.read_only, annotations.destructive, annotations.open_world),
                        (Some(true), Some(false), None)
                    );
                    assert!(!server.tools.iter().any(|tool| tool.name == "search"));
                } else {
                    assert!(server.tools.is_empty());
                }
                let Some(McpServerStatusConfig::Http {
                    url,
                    headers,
                    tools,
                    timeout,
                    request_timeout_ms,
                    always_load,
                }) = server.config
                else {
                    panic!("HTTP config")
                };
                assert_eq!(url, "https://mcp.notion.com/mcp");
                assert_eq!(headers.get("Authorization").map(String::as_str), Some("Bearer token"));
                assert_eq!(timeout, Some(5000));
                assert_eq!(request_timeout_ms, Some(30000));
                assert_eq!(always_load, load);
                for (name, policy, ceiling) in [
                    (
                        "search",
                        Some(McpServerToolPermissionPolicy::Ask),
                        Some(McpServerOrgMaxPermission::Ask),
                    ),
                    ("lookup", None, None),
                    ("write", None, Some(McpServerOrgMaxPermission::Blocked)),
                    (
                        "remove",
                        Some(McpServerToolPermissionPolicy::Deny),
                        Some(McpServerOrgMaxPermission::Blocked),
                    ),
                ] {
                    let tool =
                        tools.iter().find(|tool| tool.name == name).expect("configured tool");
                    assert_eq!(tool.permission_policy, policy, "{name}");
                    assert_eq!(tool.org_max_permission, ceiling, "{name}");
                }
            }
        }
    }

    #[test]
    fn new_terminal_reasons_deserialize_and_preserve_stored_names() {
        let cases = [
            ("api_error", TerminalReason::ApiError),
            ("malformed_tool_use_exhausted", TerminalReason::MalformedToolUseExhausted),
            ("background_requested", TerminalReason::BackgroundRequested),
            ("budget_exhausted", TerminalReason::BudgetExhausted),
            ("structured_output_retry_exhausted", TerminalReason::StructuredOutputRetryExhausted),
            ("tool_deferred_unavailable", TerminalReason::ToolDeferredUnavailable),
            ("turn_setup_failed", TerminalReason::TurnSetupFailed),
        ];

        for (stored, expected) in cases {
            let reason: TerminalReason =
                serde_json::from_value(serde_json::json!(stored)).expect("terminal reason");
            assert_eq!(reason, expected);
            assert_eq!(reason.as_stored(), stored);
        }
    }
}

#[cfg(test)]
#[path = "retry_wire_tests.rs"]
mod retry_wire_tests;
