// SPDX-License-Identifier: Apache-2.0

use super::{BlockCache, ToolCallInfo};
use crate::agent::model;

/// Build the shared baseline for unit tests that do not care about tool-specific
/// rendering metadata. Tests should override only the fields relevant to their
/// behavior.
pub(crate) fn tool_call_info(id: &str, status: model::ToolCallStatus) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_owned(),
        source_message_uuids: Vec::new(),
        title: id.to_owned(),
        sdk_tool_name: "Read".to_owned(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status,
        content: Vec::new(),
        hidden: false,
        terminal_id: None,
        terminal_command: None,
        terminal_output: None,
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    }
}
