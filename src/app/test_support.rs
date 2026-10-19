// SPDX-License-Identifier: Apache-2.0

use super::{BlockCache, ToolCallInfo};
use crate::agent::model;

/// Build the shared baseline for unit tests that do not care about tool-specific
/// rendering metadata. Tests should override only the fields relevant to their
/// behavior.
pub(crate) fn tool_call_info(id: &str, status: model::ToolCallStatus) -> ToolCallInfo {
    named_tool_call_info(id, "Read", status)
}

pub(crate) fn named_tool_call_info(
    id: &str,
    name: &str,
    status: model::ToolCallStatus,
) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_owned(),
        source_message_uuids: Vec::new(),
        title: id.to_owned(),
        sdk_tool_name: name.to_owned(),
        kind: test_tool_kind(name),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status,
        content: Vec::new(),
        parent_tool_use_id: None,
        terminal_id: None,
        terminal_command: None,
        terminal_output: None,
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
        history: crate::app::ToolCallHistory::Live,
    }
}

pub(crate) fn test_tool_kind(name: &str) -> model::ToolKind {
    match name {
        "Bash" | "PowerShell" => model::ToolKind::Execute,
        "Read" => model::ToolKind::Read,
        _ => model::ToolKind::Other,
    }
}

pub(crate) fn app_with_connection() -> (super::App, crate::agent::client::CommandReceiver) {
    let mut app = super::App::test_default();
    let (connection, rx) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    (app, rx)
}

pub(crate) fn set_tool_scope(app: &mut super::App, id: &str, scope: super::ToolCallScope) {
    let (mi, bi) = app.lookup_tool_call(id).expect("indexed fixture tool");
    let tool = app.tool_call_at_mut(mi, bi).expect("fixture tool");
    match scope {
        super::ToolCallScope::MainAgent => tool.parent_tool_use_id = None,
        super::ToolCallScope::SubagentRoot => {
            tool.sdk_tool_name = "Agent".into();
            tool.parent_tool_use_id = None;
        }
        super::ToolCallScope::SubagentChild { parent_tool_use_id } => {
            tool.parent_tool_use_id = Some(parent_tool_use_id);
        }
    }
}
