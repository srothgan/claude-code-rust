// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use serde::{Deserialize, Serialize};
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

impl TaskMetadata {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn end_time(mut self, end_time: Option<u64>) -> Self {
        self.end_time = end_time;
        self
    }

    #[must_use]
    pub fn total_paused_ms(mut self, total_paused_ms: Option<u64>) -> Self {
        self.total_paused_ms = total_paused_ms;
        self
    }

    #[must_use]
    pub fn error(mut self, error: Option<String>) -> Self {
        self.error = error;
        self
    }

    #[must_use]
    pub fn backgrounded(mut self, is_backgrounded: Option<bool>) -> Self {
        self.is_backgrounded = is_backgrounded;
        self
    }

    #[must_use]
    pub fn request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    #[must_use]
    pub fn subagent_type(mut self, subagent_type: Option<String>) -> Self {
        self.subagent_type = subagent_type;
        self
    }

    #[must_use]
    pub fn task_description(mut self, task_description: Option<String>) -> Self {
        self.task_description = task_description;
        self
    }

    #[must_use]
    pub fn task_type(mut self, task_type: Option<String>) -> Self {
        self.task_type = task_type;
        self
    }

    #[must_use]
    pub fn workflow_name(mut self, workflow_name: Option<String>) -> Self {
        self.workflow_name = workflow_name;
        self
    }

    #[must_use]
    pub fn prompt(mut self, prompt: Option<String>) -> Self {
        self.prompt = prompt;
        self
    }

    #[must_use]
    pub fn output_file(mut self, output_file: Option<String>) -> Self {
        self.output_file = output_file;
        self
    }

    #[must_use]
    pub fn summary(mut self, summary: Option<String>) -> Self {
        self.summary = summary;
        self
    }

    #[must_use]
    pub fn terminal_status(mut self, terminal_status: Option<String>) -> Self {
        self.terminal_status = terminal_status;
        self
    }
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

impl TaskItem {
    #[must_use]
    pub fn new(task_id: impl Into<String>, subject: impl Into<String>, status: TaskStatus) -> Self {
        Self {
            task_id: task_id.into(),
            subject: subject.into(),
            description: None,
            active_form: None,
            status,
            owner: None,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: None,
            source_tool_call_id: None,
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub fn active_form(mut self, active_form: impl Into<String>) -> Self {
        self.active_form = Some(active_form.into());
        self
    }

    #[must_use]
    pub fn blocks(mut self, blocks: Vec<String>) -> Self {
        self.blocks = blocks;
        self
    }

    #[must_use]
    pub fn blocked_by(mut self, blocked_by: Vec<String>) -> Self {
        self.blocked_by = blocked_by;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStateUpdate {
    pub source: TaskUpdateSource,
    pub tasks: Vec<TaskItem>,
    pub removed_task_ids: Vec<String>,
    pub is_complete_snapshot: bool,
}

impl TaskStateUpdate {
    #[must_use]
    pub fn new(source: TaskUpdateSource) -> Self {
        Self {
            source,
            tasks: Vec::new(),
            removed_task_ids: Vec::new(),
            is_complete_snapshot: false,
        }
    }

    #[must_use]
    pub fn tasks(mut self, tasks: Vec<TaskItem>) -> Self {
        self.tasks = tasks;
        self
    }

    #[must_use]
    pub fn removed_task_ids(mut self, removed_task_ids: Vec<String>) -> Self {
        self.removed_task_ids = removed_task_ids;
        self
    }

    #[must_use]
    pub const fn complete_snapshot(mut self, is_complete_snapshot: bool) -> Self {
        self.is_complete_snapshot = is_complete_snapshot;
        self
    }
}
