// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCommand {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

impl AvailableCommand {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { name: name.into(), description: description.into(), input_hint: None }
    }

    #[must_use]
    pub fn input_hint(mut self, input_hint: impl Into<String>) -> Self {
        self.input_hint = Some(input_hint.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCommandsUpdate {
    pub available_commands: Vec<AvailableCommand>,
    pub source: Option<String>,
    pub generation: Option<u64>,
}

impl AvailableCommandsUpdate {
    #[must_use]
    pub fn new(available_commands: Vec<AvailableCommand>) -> Self {
        Self { available_commands, source: None, generation: None }
    }

    #[must_use]
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    #[must_use]
    pub fn generation(mut self, generation: u64) -> Self {
        self.generation = Some(generation);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableAgent {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
}

impl AvailableAgent {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { name: name.into(), description: description.into(), model: None }
    }

    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
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

impl EffortLevel {
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Max];
    pub const PERSISTABLE_SETTINGS: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::XHigh];

    #[must_use]
    pub const fn as_stored(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "XHigh",
            Self::Max => "Max",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Low => "Fastest responses",
            Self::Medium => "Balanced speed and depth",
            Self::High => "Deeper reasoning",
            Self::XHigh => "Deeper than high",
            Self::Max => "Maximum effort",
        }
    }

    #[must_use]
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_persistable_setting(self) -> bool {
        !matches!(self, Self::Max)
    }

    #[must_use]
    pub fn from_persisted_setting(value: &str) -> Option<Self> {
        Self::from_stored(value).filter(|level| level.is_persistable_setting())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableModel {
    pub id: String,
    pub resolved_model: Option<String>,
    pub display_name: String,
    pub description: Option<String>,
    pub supports_effort: bool,
    pub supported_effort_levels: Vec<EffortLevel>,
    pub supports_adaptive_thinking: Option<bool>,
    pub supports_fast_mode: Option<bool>,
    pub supports_auto_mode: Option<bool>,
}

impl AvailableModel {
    #[must_use]
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            resolved_model: None,
            display_name: display_name.into(),
            description: None,
            supports_effort: false,
            supported_effort_levels: Vec::new(),
            supports_adaptive_thinking: None,
            supports_fast_mode: None,
            supports_auto_mode: None,
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn resolved_model(mut self, resolved_model: impl Into<String>) -> Self {
        self.resolved_model = Some(resolved_model.into());
        self
    }

    #[must_use]
    pub fn supports_effort(mut self, supports_effort: bool) -> Self {
        self.supports_effort = supports_effort;
        self
    }

    #[must_use]
    pub fn supported_effort_levels(mut self, supported_effort_levels: Vec<EffortLevel>) -> Self {
        self.supported_effort_levels = supported_effort_levels;
        self
    }

    #[must_use]
    pub fn supports_adaptive_thinking(mut self, supports_adaptive_thinking: Option<bool>) -> Self {
        self.supports_adaptive_thinking = supports_adaptive_thinking;
        self
    }

    #[must_use]
    pub fn supports_fast_mode(mut self, supports_fast_mode: Option<bool>) -> Self {
        self.supports_fast_mode = supports_fast_mode;
        self
    }

    #[must_use]
    pub fn supports_auto_mode(mut self, supports_auto_mode: Option<bool>) -> Self {
        self.supports_auto_mode = supports_auto_mode;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentModel {
    pub requested_id: Option<String>,
    pub resolved_id: String,
    pub display_name_short: String,
    pub display_name_long: String,
    pub catalog_id: Option<String>,
    pub supports_effort: bool,
    pub supported_effort_levels: Vec<EffortLevel>,
    pub supports_fast_mode: Option<bool>,
    pub supports_auto_mode: Option<bool>,
    pub supports_adaptive_thinking: Option<bool>,
    pub is_authoritative: bool,
}

impl CurrentModel {
    #[must_use]
    pub fn new(
        resolved_id: impl Into<String>,
        display_name_short: impl Into<String>,
        display_name_long: impl Into<String>,
    ) -> Self {
        Self {
            requested_id: None,
            resolved_id: resolved_id.into(),
            display_name_short: display_name_short.into(),
            display_name_long: display_name_long.into(),
            catalog_id: None,
            supports_effort: false,
            supported_effort_levels: Vec::new(),
            supports_fast_mode: None,
            supports_auto_mode: None,
            supports_adaptive_thinking: None,
            is_authoritative: false,
        }
    }

    #[must_use]
    pub fn requested_id(mut self, requested_id: impl Into<String>) -> Self {
        self.requested_id = Some(requested_id.into());
        self
    }

    #[must_use]
    pub fn catalog_id(mut self, catalog_id: impl Into<String>) -> Self {
        self.catalog_id = Some(catalog_id.into());
        self
    }

    #[must_use]
    pub fn supports_effort(mut self, supports_effort: bool) -> Self {
        self.supports_effort = supports_effort;
        self
    }

    #[must_use]
    pub fn supported_effort_levels(mut self, supported_effort_levels: Vec<EffortLevel>) -> Self {
        self.supported_effort_levels = supported_effort_levels;
        self
    }

    #[must_use]
    pub fn supports_adaptive_thinking(mut self, supports_adaptive_thinking: Option<bool>) -> Self {
        self.supports_adaptive_thinking = supports_adaptive_thinking;
        self
    }

    #[must_use]
    pub fn supports_fast_mode(mut self, supports_fast_mode: Option<bool>) -> Self {
        self.supports_fast_mode = supports_fast_mode;
        self
    }

    #[must_use]
    pub fn supports_auto_mode(mut self, supports_auto_mode: Option<bool>) -> Self {
        self.supports_auto_mode = supports_auto_mode;
        self
    }

    #[must_use]
    pub fn authoritative(mut self, is_authoritative: bool) -> Self {
        self.is_authoritative = is_authoritative;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountApiProvider {
    FirstParty,
    Bedrock,
    Vertex,
    Foundry,
    AnthropicAws,
    Mantle,
    Gateway,
    Other(String),
}

impl AccountApiProvider {
    #[must_use]
    pub fn from_wire(value: impl Into<String>) -> Self {
        match value.into().trim() {
            "firstParty" => Self::FirstParty,
            "bedrock" => Self::Bedrock,
            "vertex" => Self::Vertex,
            "foundry" => Self::Foundry,
            "anthropicAws" => Self::AnthropicAws,
            "mantle" => Self::Mantle,
            "gateway" => Self::Gateway,
            other => Self::Other(other.to_owned()),
        }
    }

    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::FirstParty => "First party",
            Self::Bedrock => "Bedrock",
            Self::Vertex => "Vertex",
            Self::Foundry => "Foundry",
            Self::AnthropicAws => "Anthropic AWS",
            Self::Mantle => "Mantle",
            Self::Gateway => "Gateway",
            Self::Other(provider) => provider.as_str(),
        }
    }

    #[must_use]
    pub fn is_external(&self) -> bool {
        !matches!(self, Self::FirstParty)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub organization: Option<String>,
    pub subscription_type: Option<String>,
    pub token_source: Option<String>,
    pub api_key_source: Option<String>,
    pub api_provider: Option<AccountApiProvider>,
}

impl AccountInfo {
    #[must_use]
    pub fn login_method_label(&self) -> String {
        if self.api_provider.as_ref().is_some_and(AccountApiProvider::is_external) {
            return "External provider".to_owned();
        }
        if let Some(source) = self.api_key_source.as_deref() {
            match source {
                "oauth" => return "Claude Max Account".to_owned(),
                "user" => return "User API key".to_owned(),
                "project" => return "Project API key".to_owned(),
                "org" => return "Organization API key".to_owned(),
                "temporary" => return "Temporary key".to_owned(),
                other if !other.is_empty() => return other.to_owned(),
                _ => {}
            }
        }
        if let Some(source) = self.token_source.as_deref()
            && !source.is_empty()
        {
            return source.to_owned();
        }
        "Unknown".to_owned()
    }
}
