// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::content::{Content, ContentBlock, TextContent};
use super::mcp::McpResource;
use super::tasks::TaskMetadata;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Execute,
    Search,
    Fetch,
    Think,
    SwitchMode,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Killed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallLocation {
    pub path: PathBuf,
    pub line: Option<u32>,
}

impl ToolCallLocation {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into(), line: None }
    }

    #[must_use]
    pub fn line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalToolCallContent {
    pub terminal_id: String,
}

impl TerminalToolCallContent {
    #[must_use]
    pub fn new(terminal_id: impl Into<String>) -> Self {
        Self { terminal_id: terminal_id.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub path: PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
    pub repository: Option<String>,
}

impl Diff {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, new_text: impl Into<String>) -> Self {
        Self { path: path.into(), old_text: None, new_text: new_text.into(), repository: None }
    }

    #[must_use]
    pub fn old_text<T: Into<String>>(mut self, old_text: Option<T>) -> Self {
        self.old_text = old_text.map(Into::into);
        self
    }

    #[must_use]
    pub fn repository(mut self, repository: Option<String>) -> Self {
        self.repository = repository.filter(|repository| !repository.trim().is_empty());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCallContent {
    Content(Content),
    Diff(Diff),
    McpResource(McpResource),
    Terminal(TerminalToolCallContent),
}

impl From<&str> for ToolCallContent {
    fn from(value: &str) -> Self {
        Self::Content(Content::new(ContentBlock::Text(TextContent::new(value))))
    }
}

impl From<String> for ToolCallContent {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_field_names)]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolCallStatus,
    pub source_message_uuid: Option<String>,
    pub content: Vec<ToolCallContent>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<serde_json::Value>,
    pub output_metadata: Option<ToolOutputMetadata>,
    pub task_metadata: Option<TaskMetadata>,
    pub locations: Vec<ToolCallLocation>,
    pub meta: Option<serde_json::Value>,
}

impl ToolCall {
    #[must_use]
    pub fn new(tool_call_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            title: title.into(),
            kind: ToolKind::Think,
            status: ToolCallStatus::Pending,
            source_message_uuid: None,
            content: Vec::new(),
            raw_input: None,
            raw_output: None,
            output_metadata: None,
            task_metadata: None,
            locations: Vec::new(),
            meta: None,
        }
    }

    #[must_use]
    pub fn kind(mut self, kind: ToolKind) -> Self {
        self.kind = kind;
        self
    }

    #[must_use]
    pub fn status(mut self, status: ToolCallStatus) -> Self {
        self.status = status;
        self
    }

    #[must_use]
    pub fn source_message_uuid(mut self, source_message_uuid: Option<String>) -> Self {
        self.source_message_uuid = source_message_uuid.filter(|uuid| !uuid.trim().is_empty());
        self
    }

    #[must_use]
    pub fn content(mut self, content: Vec<ToolCallContent>) -> Self {
        self.content = content;
        self
    }

    #[must_use]
    pub fn raw_input(mut self, raw_input: serde_json::Value) -> Self {
        self.raw_input = Some(raw_input);
        self
    }

    #[must_use]
    pub fn raw_output(mut self, raw_output: serde_json::Value) -> Self {
        self.raw_output = Some(raw_output);
        self
    }

    #[must_use]
    pub fn output_metadata(mut self, output_metadata: ToolOutputMetadata) -> Self {
        self.output_metadata = Some(output_metadata);
        self
    }

    #[must_use]
    pub fn task_metadata(mut self, task_metadata: TaskMetadata) -> Self {
        self.task_metadata = Some(task_metadata);
        self
    }

    #[must_use]
    pub fn locations(mut self, locations: Vec<ToolCallLocation>) -> Self {
        self.locations = locations;
        self
    }

    #[must_use]
    pub fn meta(mut self, meta: impl Into<serde_json::Value>) -> Self {
        self.meta = Some(meta.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolCallUpdateFields {
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolCallStatus>,
    pub content: Option<Vec<ToolCallContent>>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<serde_json::Value>,
    pub output_metadata: Option<ToolOutputMetadata>,
    pub task_metadata: Option<TaskMetadata>,
    pub locations: Option<Vec<ToolCallLocation>>,
}

impl ToolCallUpdateFields {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn kind(mut self, kind: ToolKind) -> Self {
        self.kind = Some(kind);
        self
    }

    #[must_use]
    pub fn status(mut self, status: ToolCallStatus) -> Self {
        self.status = Some(status);
        self
    }

    #[must_use]
    pub fn content(mut self, content: Vec<ToolCallContent>) -> Self {
        self.content = Some(content);
        self
    }

    #[must_use]
    pub fn raw_input(mut self, raw_input: serde_json::Value) -> Self {
        self.raw_input = Some(raw_input);
        self
    }

    #[must_use]
    pub fn raw_output(mut self, raw_output: serde_json::Value) -> Self {
        self.raw_output = Some(raw_output);
        self
    }

    #[must_use]
    pub fn output_metadata(mut self, output_metadata: ToolOutputMetadata) -> Self {
        self.output_metadata = Some(output_metadata);
        self
    }

    #[must_use]
    pub fn task_metadata(mut self, task_metadata: TaskMetadata) -> Self {
        self.task_metadata = Some(task_metadata);
        self
    }

    #[must_use]
    pub fn locations(mut self, locations: Vec<ToolCallLocation>) -> Self {
        self.locations = Some(locations);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_field_names)]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    pub source_message_uuid: Option<String>,
    pub fields: ToolCallUpdateFields,
    pub meta: Option<serde_json::Value>,
}

impl ToolCallUpdate {
    #[must_use]
    pub fn new(tool_call_id: impl Into<String>, fields: ToolCallUpdateFields) -> Self {
        Self { tool_call_id: tool_call_id.into(), source_message_uuid: None, fields, meta: None }
    }

    #[must_use]
    pub fn source_message_uuid(mut self, source_message_uuid: Option<String>) -> Self {
        self.source_message_uuid = source_message_uuid.filter(|uuid| !uuid.trim().is_empty());
        self
    }

    #[must_use]
    pub fn meta(mut self, meta: impl Into<serde_json::Value>) -> Self {
        self.meta = Some(meta.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BashOutputMetadata {
    pub assistant_auto_backgrounded: Option<bool>,
    pub timed_out_after_ms: Option<u64>,
    pub background_cwd_hint: Option<String>,
}

impl BashOutputMetadata {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn assistant_auto_backgrounded(
        mut self,
        assistant_auto_backgrounded: Option<bool>,
    ) -> Self {
        self.assistant_auto_backgrounded = assistant_auto_backgrounded;
        self
    }

    #[must_use]
    pub fn timed_out_after_ms(mut self, timed_out_after_ms: Option<u64>) -> Self {
        self.timed_out_after_ms = timed_out_after_ms;
        self
    }

    #[must_use]
    pub fn background_cwd_hint(mut self, background_cwd_hint: Option<String>) -> Self {
        self.background_cwd_hint = background_cwd_hint;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolOutputMetadata {
    pub bash: Option<BashOutputMetadata>,
    pub agent: Option<AgentOutputMetadata>,
    pub web_fetch: Option<WebFetchOutputMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentOutputMetadata {
    pub resolved_model: Option<String>,
    pub models_used: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebFetchOutputMetadata {
    pub artifact_read: Option<WebFetchArtifactReadMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebFetchArtifactReadMetadata {
    pub slug: String,
    pub ver: String,
}

impl ToolOutputMetadata {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn bash(mut self, bash: Option<BashOutputMetadata>) -> Self {
        self.bash = bash;
        self
    }

    #[must_use]
    pub fn agent(mut self, agent: Option<AgentOutputMetadata>) -> Self {
        self.agent = agent;
        self
    }

    #[must_use]
    pub fn web_fetch(mut self, web_fetch: Option<WebFetchOutputMetadata>) -> Self {
        self.web_fetch = web_fetch;
        self
    }
}

impl AgentOutputMetadata {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn resolved_model(mut self, resolved_model: Option<String>) -> Self {
        self.resolved_model = resolved_model;
        self
    }

    #[must_use]
    pub fn models_used(mut self, models_used: Option<Vec<String>>) -> Self {
        self.models_used = models_used;
        self
    }
}

impl WebFetchOutputMetadata {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn artifact_read(mut self, artifact_read: Option<WebFetchArtifactReadMetadata>) -> Self {
        self.artifact_read = artifact_read;
        self
    }
}

impl WebFetchArtifactReadMetadata {
    #[must_use]
    pub fn new(slug: impl Into<String>, ver: impl Into<String>) -> Self {
        Self { slug: slug.into(), ver: ver.into() }
    }
}
