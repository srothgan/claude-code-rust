// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use serde::{Deserialize, Serialize};

use super::catalog::{AvailableAgent, AvailableCommandsUpdate, CurrentModel};
use super::content::ContentChunk;
use super::tasks::TaskStateUpdate;
use super::tools::{ToolCall, ToolCallUpdate};

/// Verified SDK session setting, independent of the thinking effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UltracodeState {
    available: bool,
    requested: bool,
    effective: bool,
}

impl UltracodeState {
    #[must_use]
    pub const fn new(available: bool, requested: bool, effective: bool) -> Option<Self> {
        if effective == (available && requested) {
            Some(Self { available, requested, effective })
        } else {
            None
        }
    }

    #[must_use]
    pub const fn available(self) -> bool {
        self.available
    }

    #[must_use]
    pub const fn requested(self) -> bool {
        self.requested
    }

    #[must_use]
    pub const fn effective(self) -> bool {
        self.effective
    }
}

impl<'de> Deserialize<'de> for UltracodeState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Snapshot {
            available: bool,
            requested: bool,
            effective: bool,
        }
        let raw = Snapshot::deserialize(deserializer)?;
        Self::new(raw.available, raw.requested, raw.effective)
            .ok_or_else(|| serde::de::Error::custom("inconsistent Ultracode snapshot"))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableAgentsUpdate {
    pub available_agents: Vec<AvailableAgent>,
}

impl AvailableAgentsUpdate {
    #[must_use]
    pub fn new(available_agents: Vec<AvailableAgent>) -> Self {
        Self { available_agents }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentModelUpdate {
    pub current_model: CurrentModel,
}

impl CurrentModelUpdate {
    #[must_use]
    pub fn new(current_model: CurrentModel) -> Self {
        Self { current_model }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigOptionUpdate {
    pub option_id: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastModeState {
    Unknown,
    Off,
    Cooldown,
    On,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastModeSnapshot {
    pub state: FastModeState,
    pub disabled_reason: Option<String>,
}

impl FastModeSnapshot {
    #[must_use]
    pub fn new(state: FastModeState, disabled_reason: Option<String>) -> Self {
        Self { state, disabled_reason }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitStatus {
    Allowed,
    AllowedWarning,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiRetryError {
    AuthenticationFailed,
    AccountOnHold,
    OauthOrgNotAllowed,
    BillingError,
    RateLimit,
    Overloaded,
    InvalidRequest,
    ModelNotFound,
    ServerError,
    VerificationRequired,
    CloudCredentialError,
    MaxOutputTokens,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeSessionState {
    Idle,
    Running,
    RequiresAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemNoticeSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitUpdate {
    pub status: RateLimitStatus,
    pub error_code: Option<String>,
    pub resets_at: Option<f64>,
    pub utilization: Option<f64>,
    pub rate_limit_type: Option<String>,
    pub limit_scope: Option<String>,
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
    Requesting,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionBoundary {
    pub trigger: CompactionTrigger,
    pub pre_tokens: u64,
    pub post_tokens: Option<u64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionResult {
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionFailureCode {
    TooFewGroups,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionUpdate {
    Started,
    Boundary(CompactionBoundary),
    Finished {
        result: CompactionResult,
        error_code: Option<CompactionFailureCode>,
        error: Option<String>,
    },
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
    pub scope: Option<String>,
    pub api_refusal_category: Option<String>,
    pub api_refusal_explanation: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageOrigin {
    pub kind: String,
    pub subkind: Option<String>,
    pub from: Option<String>,
    pub name: Option<String>,
    pub from_session: Option<String>,
    pub sender_task_id: Option<String>,
    pub verified_peer_pid: Option<u64>,
    pub from_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalMessageUpdate {
    pub content: String,
    pub source_message_uuid: Option<String>,
    pub origin: MessageOrigin,
}

/// Observed live main-agent phase, interpreted by the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityPhase {
    Working,
    Thinking,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    AgentMessageChunk(ContentChunk),
    UserMessageChunk(ContentChunk),
    ExternalMessageUpdate(ExternalMessageUpdate),
    AgentResponseStarted,
    AgentActivityUpdate(AgentActivityPhase),
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    TranscriptRetraction(TranscriptRetraction),
    TaskStateUpdate(TaskStateUpdate),
    AvailableCommandsUpdate(AvailableCommandsUpdate),
    AvailableAgentsUpdate(AvailableAgentsUpdate),
    ModeStateUpdate(crate::app::ModeState),
    CurrentModelUpdate(CurrentModelUpdate),
    ConfigOptionUpdate(ConfigOptionUpdate),
    UltracodeUpdate {
        ultracode: Option<UltracodeState>,
    },
    FastModeUpdate {
        state: FastModeState,
        disabled_reason: Option<String>,
    },
    RateLimitUpdate(RateLimitUpdate),
    ApiRetryUpdate {
        attempt: u64,
        max_retries: u64,
        retry_delay_ms: f64,
        error_status: Option<u16>,
        error: ApiRetryError,
    },
    PromptSuggestionUpdate(String),
    /// Claude Code's title for the session; `None` when nothing printable is left.
    SessionTitleUpdate(Option<String>),
    RuntimeSessionStateUpdate(RuntimeSessionState),
    SettingsParseError {
        file: Option<String>,
        path: String,
        message: String,
    },
    SessionStatusUpdate(SessionStatus),
    NotificationUpdate {
        notification: crate::agent::notifications::SdkNotification,
        replay: bool,
    },
    SystemNoticeUpdate {
        severity: SystemNoticeSeverity,
        message: String,
    },
    CompactionUpdate(CompactionUpdate),
}
