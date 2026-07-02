// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob_saved_to: Option<PathBuf>,
}

impl McpResource {
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into(), mime_type: None, text: None, blob_saved_to: None }
    }

    #[must_use]
    pub fn mime_type(mut self, mime_type: Option<String>) -> Self {
        self.mime_type = mime_type.filter(|mime_type| !mime_type.trim().is_empty());
        self
    }

    #[must_use]
    pub fn text(mut self, text: Option<String>) -> Self {
        self.text = text.filter(|text| !text.trim().is_empty());
        self
    }

    #[must_use]
    pub fn blob_saved_to(mut self, blob_saved_to: Option<String>) -> Self {
        self.blob_saved_to =
            blob_saved_to.filter(|path| !path.trim().is_empty()).map(PathBuf::from);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub enum McpServerToolPermissionPolicy {
    Allow,
    Ask,
    Deny,
}

impl McpServerToolPermissionPolicy {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Allow => "always allow",
            Self::Ask => "always ask",
            Self::Deny => "always deny",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerOrgMaxPermission {
    Allow,
    Ask,
    Blocked,
}

impl McpServerOrgMaxPermission {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerToolPolicy {
    pub name: String,
    pub permission_policy: Option<McpServerToolPermissionPolicy>,
    pub org_max_permission: Option<McpServerOrgMaxPermission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerStatusConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        timeout: Option<u64>,
        request_timeout_ms: Option<u64>,
        always_load: Option<bool>,
    },
    Sse {
        url: String,
        headers: BTreeMap<String, String>,
        tools: Vec<McpServerToolPolicy>,
        timeout: Option<u64>,
        request_timeout_ms: Option<u64>,
        always_load: Option<bool>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
        tools: Vec<McpServerToolPolicy>,
        timeout: Option<u64>,
        request_timeout_ms: Option<u64>,
        always_load: Option<bool>,
    },
    Sdk {
        name: String,
    },
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
    pub tools: Vec<McpTool>,
}
