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

pub(crate) fn press(app: &mut super::App, code: crossterm::event::KeyCode) {
    super::handle_terminal_event(
        app,
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        )),
    );
    // The event loop commits an armed submission after routing the key.
    super::finalize_deferred_submit(app);
}

pub(crate) fn deliver_update(app: &mut super::App, update: model::SessionUpdate) {
    super::handle_client_event(
        app,
        crate::agent::events::ClientEvent::SessionUpdate { session_id: "session-1".into(), update },
    );
}

pub(crate) fn admit_tool(app: &mut super::App, tool: model::ToolCall) {
    deliver_update(app, model::SessionUpdate::ToolCall(tool));
}

pub(crate) fn request_permission(
    app: &mut super::App,
    request: model::RequestPermissionRequest,
) -> tokio::sync::oneshot::Receiver<model::RequestPermissionResponse> {
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    super::handle_client_event(
        app,
        crate::agent::events::ClientEvent::PermissionRequest {
            session_id: request.session_id.to_string(),
            request,
            response_tx,
        },
    );
    response_rx
}

pub(crate) fn permission_options(prefix: &str) -> Vec<model::PermissionOption> {
    use model::PermissionOptionKind::{AllowAlways, AllowOnce, RejectOnce};
    [("once", AllowOnce), ("always", AllowAlways), ("deny", RejectOnce)]
        .into_iter()
        .map(|(suffix, kind)| {
            model::PermissionOption::new(
                format!("{prefix}-{suffix}"),
                format!("{prefix} {suffix}"),
                kind,
            )
        })
        .collect()
}

pub(crate) fn assert_permission_response(
    receiver: &mut tokio::sync::oneshot::Receiver<model::RequestPermissionResponse>,
    id: &str,
) {
    assert_eq!(
        receiver.try_recv().expect("permission response").outcome,
        model::RequestPermissionOutcome::Selected(model::SelectedPermissionOutcome::new(id)),
    );
}

pub(crate) fn assert_pending<T>(receiver: &mut tokio::sync::oneshot::Receiver<T>) {
    assert!(matches!(receiver.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
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

thread_local! {
    static EVENT_CLOCK: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn event_clock_now() -> Option<std::time::Instant> {
    EVENT_CLOCK.with(std::cell::Cell::get)
}

pub(crate) struct EventClock {
    start: std::time::Instant,
    previous: Option<std::time::Instant>,
}

impl EventClock {
    pub(crate) fn new() -> Self {
        let start = std::time::Instant::now();
        let previous = EVENT_CLOCK.with(|clock| clock.replace(Some(start)));
        Self { start, previous }
    }

    pub(crate) fn at(&self, seconds: u64) -> std::time::Instant {
        let now = self.start + std::time::Duration::from_secs(seconds);
        EVENT_CLOCK.with(|clock| clock.set(Some(now)));
        now
    }
}

impl Drop for EventClock {
    fn drop(&mut self) {
        EVENT_CLOCK.with(|clock| clock.set(self.previous));
    }
}

/// Deterministic incompressible RGBA fixture shared by the size-policy and error/retry owners.
pub(crate) fn clipboard_noise(width: usize, height: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_u32;
    (0..width * height * 4)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[1]
        })
        .collect()
}
