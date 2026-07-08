// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use serde::{Deserialize, Serialize};

use super::catalog::{AvailableAgent, AvailableCommandsUpdate, CurrentModel};
use super::content::ContentChunk;
use super::ids::SessionModeId;
use super::tasks::TaskStateUpdate;
use super::tools::{ToolCall, ToolCallUpdate};
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
pub struct CurrentModeUpdate {
    pub current_mode_id: SessionModeId,
}

impl CurrentModeUpdate {
    #[must_use]
    pub fn new(current_mode_id: impl Into<SessionModeId>) -> Self {
        Self { current_mode_id: current_mode_id.into() }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionBoundary {
    pub trigger: CompactionTrigger,
    pub pre_tokens: u64,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionUpdate {
    AgentMessageChunk(ContentChunk),
    UserMessageChunk(ContentChunk),
    AgentThoughtChunk(ContentChunk),
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallUpdate),
    TranscriptRetraction(TranscriptRetraction),
    TaskStateUpdate(TaskStateUpdate),
    AvailableCommandsUpdate(AvailableCommandsUpdate),
    AvailableAgentsUpdate(AvailableAgentsUpdate),
    ModeStateUpdate(crate::app::ModeState),
    CurrentModeUpdate(CurrentModeUpdate),
    CurrentModelUpdate(CurrentModelUpdate),
    ConfigOptionUpdate(ConfigOptionUpdate),
    FastModeUpdate(FastModeState),
    RateLimitUpdate(RateLimitUpdate),
    ApiRetryUpdate {
        attempt: u64,
        max_retries: u64,
        retry_delay_ms: f64,
        error_status: Option<u16>,
        error: ApiRetryError,
    },
    PromptSuggestionUpdate(String),
    RuntimeSessionStateUpdate(RuntimeSessionState),
    SettingsParseError {
        file: Option<String>,
        path: String,
        message: String,
    },
    SessionStatusUpdate(SessionStatus),
    SystemNoticeUpdate {
        severity: SystemNoticeSeverity,
        message: String,
    },
    CompactionBoundary(CompactionBoundary),
}
