// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Boolean,
    String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsApplication {
    Host,
    NextSession,
    Blocked,
}
impl SettingsApplication {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Host => "immediately",
            Self::NextSession => "next session",
            Self::Blocked => "not applied",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsPersistence {
    Saved,
    Unchanged,
    Conflict,
    Failure,
    NotRequested,
}
impl SettingsPersistence {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Unchanged => "unchanged",
            Self::Conflict => "conflict",
            Self::Failure => "failure",
            Self::NotRequested => "not requested",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsOperation {
    Set,
    Remove,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsScope {
    #[default]
    User,
    Project,
    Local,
}
impl SettingsScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::User => Self::Project,
            Self::Project => Self::Local,
            Self::Local => Self::User,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingDescriptor {
    pub id: String,
    pub label: String,
    pub description: String,
    pub key_path: Vec<String>,
    pub kind: SettingKind,
    pub options: Vec<Value>,
    pub writable_scopes: Vec<SettingsScope>,
    pub allows_custom: bool,
    pub reset: String,
    pub application: SettingsApplication,
    pub unavailable: Option<String>,
}
impl SettingDescriptor {
    pub fn writable_at(&self, scope: SettingsScope) -> bool {
        self.writable_scopes.contains(&scope)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedSetting {
    pub id: String,
    pub revision: String,
    pub value: Option<Value>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsSource {
    pub scope: SettingsScope,
    pub path: String,
    pub status: String,
    pub error: Option<String>,
    pub values: Vec<ScopedSetting>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSetting {
    pub id: String,
    pub value: Option<Value>,
    pub contributors: Vec<String>,
    pub policy_restricted: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsSnapshot {
    pub cwd: String,
    pub context: String,
    pub catalog: Vec<SettingDescriptor>,
    pub sources: Vec<SettingsSource>,
    pub values: Vec<SavedSetting>,
    pub resolution_sources: Vec<ResolutionSource>,
    pub provenance: std::collections::BTreeMap<String, SettingProvenance>,
    pub diagnostics: Vec<String>,
}
impl SettingsSnapshot {
    pub fn value(&self, id: &str) -> Option<&Value> {
        self.values.iter().find(|entry| entry.id == id).and_then(|entry| entry.value.as_ref())
    }
    pub fn scoped(&self, id: &str, scope: SettingsScope) -> Option<&ScopedSetting> {
        self.sources
            .iter()
            .find(|source| source.scope == scope)?
            .values
            .iter()
            .find(|entry| entry.id == id)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsMutation {
    pub context: String,
    pub id: String,
    pub scope: SettingsScope,
    pub expected_revision: String,
    pub operation: SettingsOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsResult {
    pub persistence: SettingsPersistence,
    pub application: SettingsApplication,
    pub snapshot: Option<SettingsSnapshot>,
    pub error: Option<String>,
}

#[cfg(test)]
impl SettingsSnapshot {
    pub(crate) fn test_value(id: &str, value: Value) -> Self {
        Self {
            values: vec![SavedSetting {
                id: id.to_owned(),
                value: Some(value),
                contributors: vec!["user".to_owned()],
                policy_restricted: false,
            }],
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionSource {
    pub source: String,
    pub path: Option<String>,
    pub policy_origin: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingProvenance {
    pub source: String,
    pub path: Option<String>,
    pub policy_origin: Option<String>,
}
