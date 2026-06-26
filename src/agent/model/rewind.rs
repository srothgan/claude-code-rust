// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
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
    pub const fn label(self) -> &'static str {
        match self {
            Self::Both => "code and conversation",
            Self::Conversation => "conversation",
            Self::Code => "code",
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
    pub files_changed: Vec<String>,
    pub insertions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindResult {
    pub session_id: String,
    pub restore_mode: RewindRestoreMode,
    pub status: RewindResultStatus,
    pub file_result: Option<RewindFilesResult>,
    pub message: Option<String>,
}
