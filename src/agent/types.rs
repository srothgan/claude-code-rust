// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindTarget {
    pub uuid: String,
    pub first_text: String,
    pub input_text: String,
    pub index: u64,
    pub previous_assistant_uuid: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindRestoreMode {
    Both,
    Conversation,
    Code,
}

impl RewindRestoreMode {
    #[must_use]
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Conversation => "conversation",
            Self::Code => "code",
        }
    }

    #[must_use]
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "both" => Some(Self::Both),
            "conversation" => Some(Self::Conversation),
            "code" => Some(Self::Code),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindResultStatus {
    Success,
    Failure,
    PartialFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RewindFilesResult {
    pub can_rewind: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub files_changed: Vec<String>,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableAgent {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableModel {
    pub id: String,
    pub resolved_model: Option<String>,
    pub display_name: String,
    pub description: Option<String>,
    pub supports_effort: bool,
    #[serde(default)]
    pub supported_effort_levels: Vec<EffortLevel>,
    pub supports_adaptive_thinking: Option<bool>,
    pub supports_fast_mode: Option<bool>,
    pub supports_auto_mode: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentModel {
    pub requested_id: Option<String>,
    pub resolved_id: String,
    pub display_name_short: String,
    pub display_name_long: String,
    pub catalog_id: Option<String>,
    pub supports_effort: bool,
    #[serde(default)]
    pub supported_effort_levels: Vec<EffortLevel>,
    pub supports_fast_mode: Option<bool>,
    pub supports_auto_mode: Option<bool>,
    pub supports_adaptive_thinking: Option<bool>,
    pub is_authoritative: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastModeState {
    Off,
    Cooldown,
    On,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitStatus {
    Allowed,
    AllowedWarning,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiRetryError {
    AuthenticationFailed,
    OauthOrgNotAllowed,
    BillingError,
    RateLimit,
    Overloaded,
    InvalidRequest,
    ModelNotFound,
    ServerError,
    MaxOutputTokens,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSessionState {
    Idle,
    Running,
    RequiresAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemNoticeSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsParseErrorUpdate {
    pub file: Option<String>,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitUpdate {
    pub status: RateLimitStatus,
    pub error_code: Option<String>,
    pub resets_at: Option<f64>,
    pub utilization: Option<f64>,
    pub rate_limit_type: Option<String>,
    pub overage_status: Option<RateLimitStatus>,
    pub overage_resets_at: Option<f64>,
    pub overage_disabled_reason: Option<String>,
    pub is_using_overage: Option<bool>,
    pub surpassed_threshold: Option<f64>,
    pub can_user_purchase_credits: Option<bool>,
    pub has_chargeable_saved_payment_method: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Compacting,
    Requesting,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Manual,
    Auto,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRetractionReason {
    ModelRefusalFallback,
    ModelFallback,
    AssistantSupersedes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptRetraction {
    pub message_uuids: Vec<String>,
    pub reason: TranscriptRetractionReason,
    pub request_id: Option<String>,
    pub trigger: Option<String>,
    pub direction: Option<String>,
    pub original_model: Option<String>,
    pub fallback_model: Option<String>,
    pub api_refusal_category: Option<String>,
    pub api_refusal_explanation: Option<String>,
    pub content: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BashOutputMetadata {
    pub assistant_auto_backgrounded: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolOutputMetadata {
    pub bash: Option<BashOutputMetadata>,
    pub agent: Option<AgentOutputMetadata>,
    pub web_fetch: Option<WebFetchOutputMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentOutputMetadata {
    pub resolved_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebFetchOutputMetadata {
    pub artifact_read: Option<WebFetchArtifactReadMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebFetchArtifactReadMetadata {
    pub slug: String,
    pub ver: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskMetadata {
    pub end_time: Option<u64>,
    pub total_paused_ms: Option<u64>,
    pub error: Option<String>,
    pub is_backgrounded: Option<bool>,
    pub request_id: Option<String>,
    pub subagent_type: Option<String>,
    pub task_description: Option<String>,
    pub task_type: Option<String>,
    pub workflow_name: Option<String>,
    pub prompt: Option<String>,
    pub output_file: Option<String>,
    pub summary: Option<String>,
    pub terminal_status: Option<String>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskUpdateSource {
    #[serde(rename = "task_create")]
    Create,
    #[serde(rename = "task_update")]
    Update,
    #[serde(rename = "task_get")]
    Get,
    #[serde(rename = "task_list")]
    List,
    #[serde(rename = "task_lifecycle")]
    Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    pub task_id: String,
    pub subject: String,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub status: TaskStatus,
    pub owner: Option<String>,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub source_tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStateUpdate {
    pub source: TaskUpdateSource,
    pub tasks: Vec<TaskItem>,
    pub removed_task_ids: Vec<String>,
    pub is_complete_snapshot: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionUpdate {
    AgentMessageChunk {
        content: ContentBlock,
        source_message_uuid: Option<String>,
    },
    UserMessageChunk {
        content: ContentBlock,
        source_message_uuid: Option<String>,
    },
    AgentThoughtChunk {
        content: ContentBlock,
        source_message_uuid: Option<String>,
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
    CurrentModeUpdate {
        current_mode_id: String,
    },
    CurrentModelUpdate {
        current_model: CurrentModel,
    },
    ConfigOptionUpdate {
        option_id: String,
        value: serde_json::Value,
    },
    FastModeUpdate {
        fast_mode_state: FastModeState,
    },
    RateLimitUpdate {
        status: RateLimitStatus,
        error_code: Option<String>,
        resets_at: Option<f64>,
        utilization: Option<f64>,
        rate_limit_type: Option<String>,
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
    SystemNoticeUpdate {
        severity: SystemNoticeSeverity,
        message: String,
    },
    CompactionBoundary {
        trigger: CompactionTrigger,
        pre_tokens: u64,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PermissionDisplay {
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub option_id: String,
    pub label: String,
    pub description: Option<String>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionPrompt {
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub tool_call: ToolCall,
    pub prompt: QuestionPrompt,
    pub question_index: u64,
    pub total_questions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnnotation {
    pub preview: Option<String>,
    pub notes: Option<String>,
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

/// Snake-case payload of a `refusal_fallback_prompt` dialog, as normalized by
/// the bridge. `original_model`/`fallback_model` are always present; the rest is
/// optional metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RefusalFallbackPayload {
    pub original_model: String,
    pub fallback_model: String,
    pub api_refusal_category: Option<String>,
    pub guidance_text: Option<String>,
    pub retracted_message_uuids: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDialogOption {
    pub option_id: String,
    pub label: String,
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
    AbortedStreaming,
    AbortedTools,
    StopHookPrevented,
    HookStopped,
    ToolDeferred,
    MaxTurns,
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
            Self::AbortedStreaming => "aborted_streaming",
            Self::AbortedTools => "aborted_tools",
            Self::StopHookPrevented => "stop_hook_prevented",
            Self::HookStopped => "hook_stopped",
            Self::ToolDeferred => "tool_deferred",
            Self::MaxTurns => "max_turns",
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub organization: Option<String>,
    pub subscription_type: Option<String>,
    pub token_source: Option<String>,
    pub api_key_source: Option<String>,
    pub api_provider: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpServerConnectionStatus {
    Connected,
    Failed,
    NeedsAuth,
    Pending,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpToolAnnotations {
    pub read_only: Option<bool>,
    pub destructive: Option<bool>,
    pub open_world: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub annotations: Option<McpToolAnnotations>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerToolPermissionPolicy {
    #[serde(rename = "always_allow")]
    Allow,
    #[serde(rename = "always_ask")]
    Ask,
    #[serde(rename = "always_deny")]
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerOrgMaxPermission {
    Allow,
    Ask,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerToolPolicy {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<McpServerToolPermissionPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_max_permission: Option<McpServerOrgMaxPermission>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServerStatusConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        timeout: Option<u64>,
        request_timeout_ms: Option<u64>,
        always_load: Option<bool>,
    },
    Sse {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        tools: Vec<McpServerToolPolicy>,
        timeout: Option<u64>,
        request_timeout_ms: Option<u64>,
        always_load: Option<bool>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        #[serde(default)]
        tools: Vec<McpServerToolPolicy>,
        timeout: Option<u64>,
        request_timeout_ms: Option<u64>,
        always_load: Option<bool>,
    },
    Sdk {
        name: String,
    },
    #[serde(rename = "claudeai-proxy")]
    ClaudeaiProxy {
        url: String,
        id: String,
        timeout: Option<u64>,
    },
    Unknown {
        raw_type: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: McpServerConnectionStatus,
    pub server_info: Option<McpServerInfo>,
    pub error: Option<String>,
    pub config: Option<McpServerStatusConfig>,
    pub scope: Option<String>,
    #[serde(default)]
    pub tools: Vec<McpTool>,
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
        AccountInfo, ApiRetryError, AvailableModel, EffortLevel, McpServerOrgMaxPermission,
        McpServerStatus, McpServerStatusConfig, McpServerToolPermissionPolicy, RateLimitStatus,
        SessionStatus, SessionUpdate, SystemNoticeSeverity, TranscriptRetractionReason,
    };

    #[test]
    fn available_model_deserializes_new_effort_levels() {
        let model: AvailableModel = serde_json::from_value(serde_json::json!({
            "id": "sonnet",
            "display_name": "Claude Sonnet",
            "description": null,
            "supports_effort": true,
            "supported_effort_levels": ["low", "medium", "high", "xhigh", "max"],
            "supports_adaptive_thinking": true,
            "supports_fast_mode": true,
            "supports_auto_mode": false
        }))
        .expect("deserialize available model");

        assert_eq!(
            model.supported_effort_levels,
            vec![
                EffortLevel::Low,
                EffortLevel::Medium,
                EffortLevel::High,
                EffortLevel::XHigh,
                EffortLevel::Max,
            ]
        );
    }

    #[test]
    fn account_info_deserializes_gateway_provider() {
        let account: AccountInfo = serde_json::from_value(serde_json::json!({
            "email": null,
            "organization": null,
            "subscription_type": null,
            "token_source": null,
            "api_key_source": null,
            "api_provider": "gateway"
        }))
        .expect("deserialize account info");

        assert_eq!(account.api_provider.as_deref(), Some("gateway"));
    }

    #[test]
    fn api_retry_update_deserializes_unknown_error_defensively() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "api_retry_update",
            "attempt": 1,
            "max_retries": 4,
            "retry_delay_ms": 1000,
            "error_status": null,
            "error": "transport_timeout"
        }))
        .expect("deserialize api retry update");

        assert!(matches!(
            update,
            SessionUpdate::ApiRetryUpdate { error: ApiRetryError::Unknown, .. }
        ));
    }

    #[test]
    fn api_retry_update_deserializes_new_known_errors() {
        for (raw, expected) in [
            ("model_not_found", ApiRetryError::ModelNotFound),
            ("oauth_org_not_allowed", ApiRetryError::OauthOrgNotAllowed),
            ("overloaded", ApiRetryError::Overloaded),
        ] {
            let update: SessionUpdate = serde_json::from_value(serde_json::json!({
                "type": "api_retry_update",
                "attempt": 1,
                "max_retries": 4,
                "retry_delay_ms": 1000,
                "error_status": null,
                "error": raw
            }))
            .expect("deserialize api retry update");

            assert!(matches!(
                update,
                SessionUpdate::ApiRetryUpdate { error, .. } if error == expected
            ));
        }
    }

    #[test]
    fn rate_limit_update_deserializes_credits_metadata() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "rate_limit_update",
            "status": "rejected",
            "error_code": "credits_required",
            "can_user_purchase_credits": true,
            "has_chargeable_saved_payment_method": false
        }))
        .expect("deserialize rate limit update");

        assert!(matches!(
            update,
            SessionUpdate::RateLimitUpdate {
                status: RateLimitStatus::Rejected,
                ref error_code,
                can_user_purchase_credits: Some(true),
                has_chargeable_saved_payment_method: Some(false),
                ..
            } if error_code.as_deref() == Some("credits_required")
        ));
    }

    #[test]
    fn session_status_update_deserializes_requesting() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "session_status_update",
            "status": "requesting"
        }))
        .expect("deserialize session status update");

        assert!(matches!(
            update,
            SessionUpdate::SessionStatusUpdate { status: SessionStatus::Requesting }
        ));
    }

    #[test]
    fn available_commands_update_deserializes_source_and_generation() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "available_commands_update",
            "commands": [
                {
                    "name": "adobe-retouch-portraits",
                    "description": "Retouch portraits",
                    "input_hint": "<file>"
                }
            ],
            "source": "commands_changed",
            "generation": 2
        }))
        .expect("deserialize available commands update");

        let SessionUpdate::AvailableCommandsUpdate { commands, source, generation } = update else {
            panic!("expected available commands update");
        };

        assert_eq!(source.as_deref(), Some("commands_changed"));
        assert_eq!(generation, Some(2));
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "adobe-retouch-portraits");
        assert_eq!(commands[0].input_hint.as_deref(), Some("<file>"));
    }

    #[test]
    fn system_notice_update_deserializes() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "system_notice_update",
            "severity": "warning",
            "message": "Plugin install failed."
        }))
        .expect("deserialize system notice update");

        assert!(matches!(
            update,
            SessionUpdate::SystemNoticeUpdate {
                severity: SystemNoticeSeverity::Warning,
                ref message,
            } if message == "Plugin install failed."
        ));
    }

    #[test]
    fn mcp_status_deserializes_latest_config_fields() {
        let server: McpServerStatus = serde_json::from_value(serde_json::json!({
            "name": "notion",
            "status": "connected",
            "config": {
                "type": "http",
                "url": "https://mcp.notion.com/mcp",
                "headers": { "Authorization": "Bearer token" },
                "tools": [
                    {
                        "name": "search",
                        "permission_policy": "always_ask",
                        "org_max_permission": "ask"
                    },
                    { "name": "lookup" },
                    {
                        "name": "write",
                        "org_max_permission": "blocked"
                    }
                ],
                "timeout": 5000,
                "request_timeout_ms": 30000,
                "always_load": true
            },
            "tools": []
        }))
        .expect("deserialize MCP server status");

        let Some(McpServerStatusConfig::Http {
            tools,
            timeout,
            request_timeout_ms,
            always_load,
            ..
        }) = server.config
        else {
            panic!("expected http MCP config");
        };
        assert_eq!(timeout, Some(5000));
        assert_eq!(request_timeout_ms, Some(30000));
        assert_eq!(always_load, Some(true));
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].permission_policy, Some(McpServerToolPermissionPolicy::Ask));
        assert_eq!(tools[0].org_max_permission, Some(McpServerOrgMaxPermission::Ask));
        assert_eq!(tools[1].permission_policy, None);
        assert_eq!(tools[1].org_max_permission, None);
        assert_eq!(tools[2].permission_policy, None);
        assert_eq!(tools[2].org_max_permission, Some(McpServerOrgMaxPermission::Blocked));
    }

    #[test]
    fn task_metadata_deserializes_correlation_fields() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "tool_call_update",
            "tool_call_update": {
                "tool_call_id": "tool-1",
                "fields": {
                    "task_metadata": {
                        "request_id": "request-1",
                        "subagent_type": "tester",
                        "task_description": "Validate the branch",
                        "task_type": "remote_agent",
                        "workflow_name": "review-flow",
                        "prompt": "Validate everything",
                        "output_file": "C:/tmp/output.md",
                        "summary": "Validation complete",
                        "terminal_status": "completed"
                    }
                }
            }
        }))
        .expect("deserialize task metadata");

        let SessionUpdate::ToolCallUpdate { tool_call_update } = update else {
            panic!("expected tool call update");
        };
        let metadata = tool_call_update.fields.task_metadata.expect("task metadata");
        assert_eq!(metadata.request_id.as_deref(), Some("request-1"));
        assert_eq!(metadata.subagent_type.as_deref(), Some("tester"));
        assert_eq!(metadata.task_description.as_deref(), Some("Validate the branch"));
        assert_eq!(metadata.task_type.as_deref(), Some("remote_agent"));
        assert_eq!(metadata.workflow_name.as_deref(), Some("review-flow"));
        assert_eq!(metadata.prompt.as_deref(), Some("Validate everything"));
        assert_eq!(metadata.output_file.as_deref(), Some("C:/tmp/output.md"));
        assert_eq!(metadata.summary.as_deref(), Some("Validation complete"));
        assert_eq!(metadata.terminal_status.as_deref(), Some("completed"));
    }

    #[test]
    fn tool_output_metadata_deserializes_agent_and_artifact_read_fields() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "tool_call_update",
            "tool_call_update": {
                "tool_call_id": "tool-1",
                "fields": {
                    "output_metadata": {
                        "agent": {
                            "resolved_model": "claude-sonnet-4-7"
                        },
                        "web_fetch": {
                            "artifact_read": {
                                "slug": "dashboard",
                                "ver": "v3"
                            }
                        }
                    }
                }
            }
        }))
        .expect("deserialize output metadata");

        let SessionUpdate::ToolCallUpdate { tool_call_update } = update else {
            panic!("expected tool call update");
        };
        let metadata = tool_call_update.fields.output_metadata.expect("output metadata");
        let resolved_model = metadata.agent.and_then(|agent| agent.resolved_model);
        assert_eq!(resolved_model.as_deref(), Some("claude-sonnet-4-7"));
        let artifact_read = metadata
            .web_fetch
            .and_then(|web_fetch| web_fetch.artifact_read)
            .expect("artifact read metadata");
        assert_eq!(artifact_read.slug, "dashboard");
        assert_eq!(artifact_read.ver, "v3");
    }

    #[test]
    fn transcript_retraction_deserializes_with_metadata() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "transcript_retraction",
            "message_uuids": ["assistant-old", "tool-result-old"],
            "reason": "model_refusal_fallback",
            "request_id": "req-1",
            "trigger": "refusal",
            "direction": "retry",
            "original_model": "claude-opus-4-1",
            "fallback_model": "claude-sonnet-4-5",
            "api_refusal_category": "cyber",
            "api_refusal_explanation": "policy text",
            "content": "Retried with fallback model"
        }))
        .expect("deserialize transcript retraction");

        let SessionUpdate::TranscriptRetraction { retraction } = update else {
            panic!("expected transcript retraction");
        };
        assert_eq!(retraction.message_uuids, vec!["assistant-old", "tool-result-old"]);
        assert_eq!(retraction.reason, TranscriptRetractionReason::ModelRefusalFallback);
        assert_eq!(retraction.request_id.as_deref(), Some("req-1"));
        assert_eq!(retraction.original_model.as_deref(), Some("claude-opus-4-1"));
        assert_eq!(retraction.fallback_model.as_deref(), Some("claude-sonnet-4-5"));
    }

    #[test]
    fn transcript_updates_deserialize_source_message_uuid() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "agent_message_chunk",
            "source_message_uuid": "assistant-1",
            "content": { "type": "text", "text": "hello" }
        }))
        .expect("deserialize source-tagged chunk");

        let SessionUpdate::AgentMessageChunk { source_message_uuid, .. } = update else {
            panic!("expected agent message chunk");
        };
        assert_eq!(source_message_uuid.as_deref(), Some("assistant-1"));
    }

    #[test]
    fn api_retry_update_deserializes_fractional_retry_delay() {
        let update: SessionUpdate = serde_json::from_value(serde_json::json!({
            "type": "api_retry_update",
            "attempt": 1,
            "max_retries": 10,
            "retry_delay_ms": 549.888_169_845_942_6,
            "error_status": null,
            "error": "unknown"
        }))
        .expect("deserialize fractional api retry update");

        let SessionUpdate::ApiRetryUpdate { retry_delay_ms, .. } = update else {
            panic!("expected api retry update");
        };
        assert!((retry_delay_ms - 549.888_169_845_942_6).abs() < f64::EPSILON);
    }

    #[test]
    fn api_retry_update_rejects_negative_retry_delay() {
        let error = serde_json::from_value::<SessionUpdate>(serde_json::json!({
            "type": "api_retry_update",
            "attempt": 1,
            "max_retries": 10,
            "retry_delay_ms": -1.0,
            "error_status": null,
            "error": "unknown"
        }))
        .expect_err("negative retry delay should be rejected");

        assert!(error.to_string().contains("retry_delay_ms must be a finite non-negative number"));
    }
}
