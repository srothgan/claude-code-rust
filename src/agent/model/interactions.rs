// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use super::ids::SessionId;
use super::tools::ToolCallUpdate;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowSession,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    QuestionChoice,
    PlanApprove,
    PlanReject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: PermissionOptionKind,
}

impl PermissionOption {
    #[must_use]
    pub fn new(
        option_id: impl Into<String>,
        name: impl Into<String>,
        kind: PermissionOptionKind,
    ) -> Self {
        Self { option_id: option_id.into(), name: name.into(), description: None, kind }
    }

    #[must_use]
    pub fn description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub option_id: String,
    pub label: String,
    pub description: Option<String>,
    pub preview: Option<String>,
}

impl QuestionOption {
    #[must_use]
    pub fn new(option_id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { option_id: option_id.into(), label: label.into(), description: None, preview: None }
    }

    #[must_use]
    pub fn description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    #[must_use]
    pub fn preview(mut self, preview: Option<String>) -> Self {
        self.preview = preview;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionPrompt {
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

impl QuestionPrompt {
    #[must_use]
    pub fn new(
        question: impl Into<String>,
        header: impl Into<String>,
        multi_select: bool,
        options: Vec<QuestionOption>,
    ) -> Self {
        Self { question: question.into(), header: header.into(), multi_select, options }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnnotation {
    pub preview: Option<String>,
    pub notes: Option<String>,
}

impl QuestionAnnotation {
    #[must_use]
    pub fn new() -> Self {
        Self { preview: None, notes: None }
    }

    #[must_use]
    pub fn preview(mut self, preview: Option<String>) -> Self {
        self.preview = preview;
        self
    }

    #[must_use]
    pub fn notes(mut self, notes: Option<String>) -> Self {
        self.notes = notes;
        self
    }
}

impl Default for QuestionAnnotation {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedPermissionOutcome {
    pub option_id: String,
}

impl SelectedPermissionOutcome {
    #[must_use]
    pub fn new(option_id: impl Into<String>) -> Self {
        Self { option_id: option_id.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestPermissionOutcome {
    Selected(SelectedPermissionOutcome),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnsweredQuestionOutcome {
    pub selected_option_ids: Vec<String>,
    pub annotation: Option<QuestionAnnotation>,
}

impl AnsweredQuestionOutcome {
    #[must_use]
    pub fn new(selected_option_ids: Vec<String>) -> Self {
        Self { selected_option_ids, annotation: None }
    }

    #[must_use]
    pub fn annotation(mut self, annotation: Option<QuestionAnnotation>) -> Self {
        self.annotation = annotation;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestQuestionOutcome {
    Answered(AnsweredQuestionOutcome),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPermissionResponse {
    pub outcome: RequestPermissionOutcome,
}

impl RequestPermissionResponse {
    #[must_use]
    pub fn new(outcome: RequestPermissionOutcome) -> Self {
        Self { outcome }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestQuestionResponse {
    pub outcome: RequestQuestionOutcome,
}

impl RequestQuestionResponse {
    #[must_use]
    pub fn new(outcome: RequestQuestionOutcome) -> Self {
        Self { outcome }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestPermissionRequest {
    pub session_id: SessionId,
    pub tool_call: ToolCallUpdate,
    pub options: Vec<PermissionOption>,
    pub display: Option<PermissionDisplay>,
}

impl RequestPermissionRequest {
    #[must_use]
    pub fn new(
        session_id: impl Into<SessionId>,
        tool_call: ToolCallUpdate,
        options: Vec<PermissionOption>,
        display: Option<PermissionDisplay>,
    ) -> Self {
        Self { session_id: session_id.into(), tool_call, options, display }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PermissionDisplay {
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

impl PermissionDisplay {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    #[must_use]
    pub fn display_name(mut self, display_name: Option<String>) -> Self {
        self.display_name = display_name;
        self
    }

    #[must_use]
    pub fn description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.as_ref().is_none_or(|value| value.trim().is_empty())
            && self.display_name.as_ref().is_none_or(|value| value.trim().is_empty())
            && self.description.as_ref().is_none_or(|value| value.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestQuestionRequest {
    pub session_id: SessionId,
    pub tool_call: ToolCallUpdate,
    pub prompt: QuestionPrompt,
    pub question_index: usize,
    pub total_questions: usize,
}

impl RequestQuestionRequest {
    #[must_use]
    pub fn new(
        session_id: impl Into<SessionId>,
        tool_call: ToolCallUpdate,
        prompt: QuestionPrompt,
        question_index: usize,
        total_questions: usize,
    ) -> Self {
        Self { session_id: session_id.into(), tool_call, prompt, question_index, total_questions }
    }
}
