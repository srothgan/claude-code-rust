// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

/// SDK provenance and delivery reports; shared by the wire and runtime model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum SdkNotification {
    SdkNotice {
        session_id: String,
        uuid: String,
        key: String,
        text: String,
        priority: String,
        color: Option<String>,
        timeout_ms: Option<f64>,
    },
    ModelTool {
        session_id: String,
        tool_use_id: String,
        text: String,
        push_sent: Option<bool>,
        local_sent: Option<bool>,
        disabled_reason: Option<String>,
        sent_at: Option<String>,
    },
}

impl SdkNotification {
    pub fn session_id(&self) -> &str {
        match self {
            Self::SdkNotice { session_id, .. } | Self::ModelTool { session_id, .. } => session_id,
        }
    }
}
