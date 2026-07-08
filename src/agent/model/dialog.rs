// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use super::ids::SessionId;
/// Snake-case payload of a `refusal_fallback_prompt` dialog. `original_model`
/// and `fallback_model` are always present; the rest is optional metadata.
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

impl UserDialogOption {
    #[must_use]
    pub fn new(option_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { option_id: option_id.into(), label: label.into() }
    }
}

/// Turn-level `request_user_dialog` request surfaced to the app. Keyed by a
/// bridge-generated `request_id`; it has no tool-call anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestUserDialogRequest {
    pub session_id: SessionId,
    pub request_id: String,
    pub dialog_kind: String,
    pub payload: RefusalFallbackPayload,
    pub options: Vec<UserDialogOption>,
}

impl RequestUserDialogRequest {
    #[must_use]
    pub fn new(
        session_id: impl Into<SessionId>,
        request_id: impl Into<String>,
        dialog_kind: impl Into<String>,
        payload: RefusalFallbackPayload,
        options: Vec<UserDialogOption>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            request_id: request_id.into(),
            dialog_kind: dialog_kind.into(),
            payload,
            options,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedUserDialogOutcome {
    pub option_id: String,
}

impl SelectedUserDialogOutcome {
    #[must_use]
    pub fn new(option_id: impl Into<String>) -> Self {
        Self { option_id: option_id.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestUserDialogOutcome {
    Selected(SelectedUserDialogOutcome),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestUserDialogResponse {
    pub outcome: RequestUserDialogOutcome,
}

impl RequestUserDialogResponse {
    #[must_use]
    pub fn new(outcome: RequestUserDialogOutcome) -> Self {
        Self { outcome }
    }
}
