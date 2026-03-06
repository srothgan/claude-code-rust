// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

pub mod block_cache;
pub mod cache_metrics;
mod history_retention;
pub mod messages;
mod render_budget;
pub mod tool_call_info;
pub mod types;
pub mod viewport;

// Re-export all public types so external `use crate::app::state::X` paths still work.
pub use block_cache::BlockCache;
pub use cache_metrics::CacheMetrics;
pub use messages::{
    ChatMessage, IncrementalMarkdown, MessageBlock, MessageRole, SystemSeverity, WelcomeBlock,
};
pub use tool_call_info::{
    InlinePermission, TerminalSnapshotMode, ToolCallInfo, is_execute_tool_name,
};
pub use types::{
    AppStatus, CancelOrigin, HelpView, HistoryRetentionPolicy, HistoryRetentionStats, LoginHint,
    MessageUsage, ModeInfo, ModeState, PasteSessionState, PendingCommandAck, RecentSessionInfo,
    RenderCacheBudget, SUBAGENT_THINKING_DEBOUNCE, ScrollbarDragState, SelectionKind,
    SelectionPoint, SelectionState, SessionUsageState, TodoItem, TodoStatus, ToolCallScope,
};
pub use viewport::{ChatViewport, InvalidationLevel};

use crate::agent::events::ClientEvent;
use crate::agent::model;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;
use tokio::sync::mpsc;

use super::dialog;
use super::focus::{FocusContext, FocusManager, FocusOwner, FocusTarget};
use super::input::{InputState, parse_paste_placeholder_before_cursor};
use super::mention;
use super::slash;
use super::subagent;

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub messages: Vec<ChatMessage>,
    /// Single owner of all chat layout state: scroll, per-message heights, prefix sums.
    pub viewport: ChatViewport,
    pub input: InputState,
    pub status: AppStatus,
    /// Session id currently being resumed via `/resume`.
    pub resuming_session_id: Option<String>,
    /// Spinner label shown while a slash command is in flight (`CommandPending`).
    pub pending_command_label: Option<String>,
    /// Ack marker required to clear `CommandPending` for strict completion semantics.
    pub pending_command_ack: Option<PendingCommandAck>,
    pub should_quit: bool,
    /// Optional fatal app error that should be surfaced at CLI boundary.
    pub exit_error: Option<crate::error::AppError>,
    pub session_id: Option<model::SessionId>,
    /// Agent connection handle. `None` while connecting (before bridge is ready).
    pub conn: Option<Rc<crate::agent::client::AgentConnection>>,
    pub model_name: String,
    pub cwd: String,
    pub cwd_raw: String,
    pub files_accessed: usize,
    pub mode: Option<ModeState>,
    /// Latest config options observed from bridge `config_option_update` events.
    pub config_options: BTreeMap<String, serde_json::Value>,
    /// Login hint shown when authentication is required. Rendered above the input field.
    pub login_hint: Option<LoginHint>,
    /// When true, the current/next turn completion should clear local conversation history.
    /// Set by `/compact` once the command is accepted for bridge forwarding.
    pub pending_compact_clear: bool,
    /// Active help overlay view when `?` help is open.
    pub help_view: HelpView,
    /// Scroll/selection state for the Slash and Subagents help tabs.
    pub help_dialog: dialog::DialogState,
    /// Number of items that currently fit in the help viewport (updated each render).
    /// Used by key handlers for accurate scroll step size.
    pub help_visible_count: usize,
    /// Tool call IDs with pending permission prompts, ordered by arrival.
    /// The first entry is the "focused" permission that receives keyboard input.
    /// Up / Down arrow keys cycle focus through the list.
    pub pending_permission_ids: Vec<String>,
    /// Set when a cancel notification succeeds; consumed on `TurnComplete`
    /// to render a red interruption hint in chat.
    pub cancelled_turn_pending_hint: bool,
    /// Origin of the in-flight cancellation request, if any.
    pub pending_cancel_origin: Option<CancelOrigin>,
    /// Auto-submit the current input draft once cancellation transitions the app
    /// back to `Ready`.
    pub pending_auto_submit_after_cancel: bool,
    pub event_tx: mpsc::UnboundedSender<ClientEvent>,
    pub event_rx: mpsc::UnboundedReceiver<ClientEvent>,
    pub spinner_frame: usize,
    /// Session-level default for tool call collapsed state.
    /// Toggled by Ctrl+O - new tool calls inherit this value.
    pub tools_collapsed: bool,
    /// IDs of Task/Agent tool calls currently `InProgress` -- their children get hidden.
    /// Use `insert_active_task()`, `remove_active_task()`.
    pub active_task_ids: HashSet<String>,
    /// Tool scope keyed by tool call ID; used to distinguish main-agent from subagent tools.
    pub tool_call_scopes: HashMap<String, ToolCallScope>,
    /// IDs of non Task/Agent subagent tool calls currently `InProgress`/`Pending`.
    pub active_subagent_tool_ids: HashSet<String>,
    /// Timestamp when subagent entered an idle gap (no active child tool calls).
    pub subagent_idle_since: Option<Instant>,
    /// Shared terminal process map - used to snapshot output on completion.
    pub terminals: crate::agent::events::TerminalMap,
    /// Force a full terminal clear on next render frame.
    pub force_redraw: bool,
    /// O(1) lookup: `tool_call_id` -> `(message_index, block_index)`.
    /// Use `lookup_tool_call()`, `index_tool_call()`.
    pub tool_call_index: HashMap<String, (usize, usize)>,
    /// Current todo list from Claude's `TodoWrite` tool calls.
    pub todos: Vec<TodoItem>,
    /// Whether the header bar is visible.
    /// Toggled by Ctrl+H.
    pub show_header: bool,
    /// Whether the todo panel is expanded (true) or shows compact status line (false).
    /// Toggled by Ctrl+T.
    pub show_todo_panel: bool,
    /// Scroll offset for the expanded todo panel (capped at 5 visible lines).
    pub todo_scroll: usize,
    /// Selected todo index used for keyboard navigation in the open todo panel.
    pub todo_selected: usize,
    /// Focus manager for directional/navigation key ownership.
    pub focus: FocusManager,
    /// Commands advertised by the agent via `AvailableCommandsUpdate`.
    pub available_commands: Vec<model::AvailableCommand>,
    /// Subagents advertised by the agent via `AvailableAgentsUpdate`.
    pub available_agents: Vec<model::AvailableAgent>,
    /// Recently persisted session IDs discovered at startup.
    pub recent_sessions: Vec<RecentSessionInfo>,
    /// Last known frame area (for mouse selection mapping).
    pub cached_frame_area: ratatui::layout::Rect,
    /// Current selection state for mouse-based selection.
    pub selection: Option<SelectionState>,
    /// Active scrollbar drag state while left mouse button is held on the rail.
    pub scrollbar_drag: Option<ScrollbarDragState>,
    /// Cached rendered chat lines for selection/copy.
    pub rendered_chat_lines: Vec<String>,
    /// Area where chat content was rendered (for selection mapping).
    pub rendered_chat_area: ratatui::layout::Rect,
    /// Cached rendered input lines for selection/copy.
    pub rendered_input_lines: Vec<String>,
    /// Area where input content was rendered (for selection mapping).
    pub rendered_input_area: ratatui::layout::Rect,
    /// Active `@` file mention autocomplete state.
    pub mention: Option<mention::MentionState>,
    /// Active slash-command autocomplete state.
    pub slash: Option<slash::SlashState>,
    /// Active subagent autocomplete state (`&name`).
    pub subagent: Option<subagent::SubagentState>,
    /// Deferred submit: set `true` when Enter is pressed. If another key event
    /// arrives during the same drain cycle (paste), this is cleared and the Enter
    /// becomes a newline. After the drain, the main loop checks: if still `true`,
    /// strips the trailing newline and submits.
    pub pending_submit: bool,
    /// Timing-based paste burst detector. Detects rapid character streams
    /// (paste delivered as individual key events) and buffers them into a
    /// single paste payload. Fallback for terminals without bracketed paste.
    pub paste_burst: super::paste_burst::PasteBurstDetector,
    /// Buffered `Event::Paste` payload for this drain cycle.
    /// Some terminals split one clipboard paste into multiple chunks; we merge
    /// them and apply placeholder threshold to the merged content once per cycle.
    pub pending_paste_text: String,
    /// Pending paste session metadata for the currently queued `Event::Paste` payload.
    pub pending_paste_session: Option<PasteSessionState>,
    /// Most recent active placeholder paste session, used for safe chunk continuation.
    pub active_paste_session: Option<PasteSessionState>,
    /// Monotonic counter for paste session identifiers.
    pub next_paste_session_id: u64,
    /// Cached file list from cwd (scanned on first `@` trigger).
    pub file_cache: Option<Vec<mention::FileCandidate>>,
    /// Cached todo compact line (invalidated on `set_todos()`).
    pub cached_todo_compact: Option<ratatui::text::Line<'static>>,
    /// Current git branch (refreshed on focus gain + turn complete).
    pub git_branch: Option<String>,
    /// Cached header line (invalidated when git branch changes).
    pub cached_header_line: Option<ratatui::text::Line<'static>>,
    /// Cached footer line (invalidated on mode change).
    pub cached_footer_line: Option<ratatui::text::Line<'static>>,
    /// Optional startup update-check hint rendered at the footer's right edge.
    pub update_check_hint: Option<String>,
    /// True when startup service-status check reported an outage and input should remain blocked.
    pub startup_status_blocking_error: bool,
    /// Session-wide usage and cost telemetry from the bridge.
    pub session_usage: SessionUsageState,
    /// Fast mode state telemetry from the SDK.
    pub fast_mode_state: model::FastModeState,
    /// Latest rate-limit telemetry from the SDK.
    pub last_rate_limit_update: Option<model::RateLimitUpdate>,
    /// True while the SDK reports active compaction.
    pub is_compacting: bool,

    /// Indexed terminal tool calls: `(terminal_id, msg_idx, block_idx)`.
    /// Avoids O(n*m) scan of all messages/blocks every frame.
    pub terminal_tool_calls: Vec<(String, usize, usize)>,
    /// Dirty flag: skip `terminal.draw()` when nothing changed since last frame.
    pub needs_redraw: bool,
    /// Central notification manager (bell + desktop toast when unfocused).
    pub notifications: super::notify::NotificationManager,
    /// Performance logger. Present only when built with `--features perf`.
    /// Taken out (`Option::take`) during render, used, then put back to avoid
    /// borrow conflicts with `&mut App`.
    pub perf: Option<crate::perf::PerfLogger>,
    /// Global in-memory budget for rendered block caches (message + tool + welcome).
    pub render_cache_budget: RenderCacheBudget,
    /// Byte budget for source conversation history retained in memory.
    pub history_retention: HistoryRetentionPolicy,
    /// Last history-retention enforcement statistics.
    pub history_retention_stats: HistoryRetentionStats,
    /// Cross-cutting cache metrics accumulator (enforcement counts, watermarks, rate limits).
    pub cache_metrics: CacheMetrics,
    /// Smoothed frames-per-second (EMA of presented frame cadence).
    pub fps_ema: Option<f32>,
    /// Timestamp of the previous presented frame.
    pub last_frame_at: Option<Instant>,
}

impl App {
    /// Queue a paste payload for drain-cycle finalization.
    ///
    /// This is fed by paste payloads captured from terminal events.
    pub fn queue_paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.pending_submit = false;
        if self.pending_paste_text.is_empty() {
            let continued_session = self.active_paste_session.and_then(|session| {
                let current_line = self.input.lines().get(self.input.cursor_row())?;
                let idx =
                    parse_paste_placeholder_before_cursor(current_line, self.input.cursor_col())?;
                (session.placeholder_index == Some(idx)).then_some(session)
            });
            self.pending_paste_session = Some(continued_session.unwrap_or_else(|| {
                let id = self.next_paste_session_id;
                self.next_paste_session_id = self.next_paste_session_id.saturating_add(1);
                PasteSessionState {
                    id,
                    start: SelectionPoint {
                        row: self.input.cursor_row(),
                        col: self.input.cursor_col(),
                    },
                    placeholder_index: None,
                }
            }));
        }
        self.pending_paste_text.push_str(text);
    }

    /// Mark one presented frame at `now`, updating smoothed FPS.
    pub fn mark_frame_presented(&mut self, now: Instant) {
        let Some(prev) = self.last_frame_at.replace(now) else {
            return;
        };
        let dt = now.saturating_duration_since(prev).as_secs_f32();
        if dt <= f32::EPSILON {
            return;
        }
        let fps = (1.0 / dt).clamp(0.0, 240.0);
        self.fps_ema = Some(match self.fps_ema {
            Some(current) => current * 0.9 + fps * 0.1,
            None => fps,
        });
    }

    #[must_use]
    pub fn frame_fps(&self) -> Option<f32> {
        self.fps_ema
    }

    /// Ensure the synthetic welcome message exists at index 0.
    pub fn ensure_welcome_message(&mut self) {
        if self.messages.first().is_some_and(|m| matches!(m.role, MessageRole::Welcome)) {
            return;
        }
        self.messages.insert(
            0,
            ChatMessage::welcome_with_recent(&self.model_name, &self.cwd, &self.recent_sessions),
        );
        self.invalidate_layout(InvalidationLevel::From(0));
    }

    /// Update the welcome message's model name, but only before chat starts.
    pub fn update_welcome_model_if_pristine(&mut self) {
        if self.messages.len() != 1 {
            return;
        }
        let Some(first) = self.messages.first_mut() else {
            return;
        };
        if !matches!(first.role, MessageRole::Welcome) {
            return;
        }
        let Some(MessageBlock::Welcome(welcome)) = first.blocks.first_mut() else {
            return;
        };
        welcome.model_name.clone_from(&self.model_name);
        welcome.cache.invalidate();
        self.invalidate_layout(InvalidationLevel::From(0));
    }

    /// Update the welcome message with latest discovered recent sessions.
    pub fn sync_welcome_recent_sessions(&mut self) {
        let Some(first) = self.messages.first_mut() else {
            return;
        };
        if !matches!(first.role, MessageRole::Welcome) {
            return;
        }
        let Some(MessageBlock::Welcome(welcome)) = first.blocks.first_mut() else {
            return;
        };
        welcome.recent_sessions.clone_from(&self.recent_sessions);
        welcome.cache.invalidate();
        self.invalidate_layout(InvalidationLevel::From(0));
    }

    /// Track a Task/Agent tool call as active (in-progress subagent).
    pub fn insert_active_task(&mut self, id: String) {
        self.active_task_ids.insert(id);
    }

    /// Remove a Task/Agent tool call from the active set (completed/failed).
    pub fn remove_active_task(&mut self, id: &str) {
        self.active_task_ids.remove(id);
    }

    pub fn register_tool_call_scope(&mut self, id: String, scope: ToolCallScope) {
        self.tool_call_scopes.insert(id, scope);
    }

    #[must_use]
    pub fn tool_call_scope(&self, id: &str) -> Option<ToolCallScope> {
        self.tool_call_scopes.get(id).copied()
    }

    pub fn mark_subagent_tool_started(&mut self, id: &str) {
        self.active_subagent_tool_ids.insert(id.to_owned());
        self.subagent_idle_since = None;
    }

    pub fn mark_subagent_tool_finished(&mut self, id: &str, now: Instant) {
        self.active_subagent_tool_ids.remove(id);
        self.refresh_subagent_idle_since(now);
    }

    pub fn refresh_subagent_idle_since(&mut self, now: Instant) {
        if self.active_task_ids.is_empty() || !self.active_subagent_tool_ids.is_empty() {
            self.subagent_idle_since = None;
            return;
        }
        if self.subagent_idle_since.is_none() {
            self.subagent_idle_since = Some(now);
        }
    }

    #[must_use]
    pub fn should_show_subagent_thinking(&self, now: Instant) -> bool {
        if self.active_task_ids.is_empty() || !self.active_subagent_tool_ids.is_empty() {
            return false;
        }
        self.subagent_idle_since
            .is_some_and(|since| now.saturating_duration_since(since) >= SUBAGENT_THINKING_DEBOUNCE)
    }

    pub fn clear_tool_scope_tracking(&mut self) {
        self.tool_call_scopes.clear();
        self.active_task_ids.clear();
        self.active_subagent_tool_ids.clear();
        self.subagent_idle_since = None;
    }

    /// Look up the (`message_index`, `block_index`) for a tool call ID.
    #[must_use]
    pub fn lookup_tool_call(&self, id: &str) -> Option<(usize, usize)> {
        self.tool_call_index.get(id).copied()
    }

    /// Register a tool call's position in the message/block arrays.
    pub fn index_tool_call(&mut self, id: String, msg_idx: usize, block_idx: usize) {
        self.tool_call_index.insert(id, (msg_idx, block_idx));
    }

    /// Invalidate message layout caches at the given level.
    ///
    /// Single entry point for all layout invalidation. Replaces the former
    /// `mark_message_layout_dirty` / `mark_all_message_layout_dirty` methods.
    pub fn invalidate_layout(&mut self, level: InvalidationLevel) {
        match level {
            InvalidationLevel::Single(idx) => {
                self.viewport.mark_message_dirty(idx);
                // Non-tail single change: prefix sums from idx onward shift.
                if idx + 1 < self.messages.len() {
                    self.viewport.prefix_sums_width = 0;
                }
            }
            InvalidationLevel::From(idx) => {
                self.viewport.mark_message_dirty(idx);
                // Structural change: always force full prefix-sum rebuild.
                self.viewport.prefix_sums_width = 0;
            }
            InvalidationLevel::Global => {
                if self.messages.is_empty() {
                    return;
                }
                self.viewport.mark_message_dirty(0);
                self.viewport.prefix_sums_width = 0;
                self.viewport.bump_layout_generation();
            }
            InvalidationLevel::Resize => {
                // Resize is handled by viewport.on_frame(). This arm exists
                // for exhaustiveness; production code should not reach it.
                debug_assert!(false, "Resize should not be dispatched through invalidate_layout");
            }
        }
    }

    /// Enforce history retention and record metrics.
    ///
    /// Wrapper around [`enforce_history_retention`] that feeds the returned stats
    /// into `CacheMetrics` and emits rate-limited structured tracing. Call this
    /// instead of `enforce_history_retention()` at all non-test call sites.
    pub fn enforce_history_retention_tracked(&mut self) {
        let stats = self.enforce_history_retention();
        let should_log =
            self.cache_metrics.record_history_enforcement(&stats, self.history_retention);
        if should_log {
            let snap = cache_metrics::build_snapshot(
                &self.render_cache_budget,
                &self.history_retention_stats,
                self.history_retention,
                &self.cache_metrics,
                &self.viewport,
                0, // entry_count not needed for history-only log
                0,
                stats.dropped_messages,
                0, // protected_bytes not relevant for history-only log
            );
            cache_metrics::emit_history_metrics(&snap);
        }
    }

    /// Force-finish any lingering in-progress tool calls.
    /// Returns the number of tool calls that were transitioned.
    pub fn finalize_in_progress_tool_calls(&mut self, new_status: model::ToolCallStatus) -> usize {
        let mut changed = 0usize;
        let mut cleared_permission = false;
        let mut first_changed_idx: Option<usize> = None;

        for (msg_idx, msg) in self.messages.iter_mut().enumerate() {
            for block in &mut msg.blocks {
                if let MessageBlock::ToolCall(tc) = block {
                    let tc = tc.as_mut();
                    if matches!(
                        tc.status,
                        model::ToolCallStatus::InProgress | model::ToolCallStatus::Pending
                    ) {
                        tc.status = new_status;
                        tc.mark_tool_call_layout_dirty();
                        if tc.pending_permission.take().is_some() {
                            cleared_permission = true;
                        }
                        first_changed_idx =
                            Some(first_changed_idx.map_or(msg_idx, |prev| prev.min(msg_idx)));
                        changed += 1;
                    }
                }
            }
        }

        if changed > 0 || cleared_permission {
            if let Some(msg_idx) = first_changed_idx {
                self.invalidate_layout(InvalidationLevel::Single(msg_idx));
            }
            self.pending_permission_ids.clear();
            self.release_focus_target(FocusTarget::Permission);
        }

        changed
    }

    /// Build a minimal `App` for unit/integration tests.
    /// All fields get sensible defaults; the `mpsc` channel is wired up internally.
    #[doc(hidden)]
    #[must_use]
    pub fn test_default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            messages: Vec::new(),
            viewport: ChatViewport::new(),
            input: InputState::new(),
            status: AppStatus::Ready,
            resuming_session_id: None,
            pending_command_label: None,
            pending_command_ack: None,
            should_quit: false,
            exit_error: None,
            session_id: None,
            conn: None,
            model_name: "test-model".into(),
            cwd: "/test".into(),
            cwd_raw: "/test".into(),
            files_accessed: 0,
            mode: None,
            config_options: BTreeMap::new(),
            login_hint: None,
            pending_compact_clear: false,
            help_view: HelpView::Keys,
            help_dialog: dialog::DialogState::default(),
            help_visible_count: 5,
            pending_permission_ids: Vec::new(),
            cancelled_turn_pending_hint: false,
            pending_cancel_origin: None,
            pending_auto_submit_after_cancel: false,
            event_tx: tx,
            event_rx: rx,
            spinner_frame: 0,
            tools_collapsed: false,
            active_task_ids: HashSet::default(),
            tool_call_scopes: HashMap::default(),
            active_subagent_tool_ids: HashSet::default(),
            subagent_idle_since: None,
            terminals: std::rc::Rc::default(),
            force_redraw: false,
            tool_call_index: HashMap::default(),
            todos: Vec::new(),
            show_header: true,
            show_todo_panel: false,
            todo_scroll: 0,
            todo_selected: 0,
            focus: FocusManager::default(),
            available_commands: Vec::new(),
            available_agents: Vec::new(),
            recent_sessions: Vec::new(),
            cached_frame_area: ratatui::layout::Rect::default(),
            selection: None,
            scrollbar_drag: None,
            rendered_chat_lines: Vec::new(),
            rendered_chat_area: ratatui::layout::Rect::default(),
            rendered_input_lines: Vec::new(),
            rendered_input_area: ratatui::layout::Rect::default(),
            mention: None,
            slash: None,
            subagent: None,
            pending_submit: false,
            paste_burst: super::paste_burst::PasteBurstDetector::new(),
            pending_paste_text: String::new(),
            pending_paste_session: None,
            active_paste_session: None,
            next_paste_session_id: 1,
            file_cache: None,
            cached_todo_compact: None,
            git_branch: None,
            cached_header_line: None,
            cached_footer_line: None,
            update_check_hint: None,
            startup_status_blocking_error: false,
            session_usage: SessionUsageState::default(),
            fast_mode_state: model::FastModeState::Off,
            last_rate_limit_update: None,
            is_compacting: false,
            terminal_tool_calls: Vec::new(),
            needs_redraw: true,
            notifications: super::notify::NotificationManager::new(),
            perf: None,
            render_cache_budget: RenderCacheBudget::default(),
            history_retention: HistoryRetentionPolicy::default(),
            history_retention_stats: HistoryRetentionStats::default(),
            cache_metrics: CacheMetrics::default(),
            fps_ema: None,
            last_frame_at: None,
        }
    }

    /// Detect the current git branch and invalidate the header cache if it changed.
    pub fn refresh_git_branch(&mut self) {
        let new_branch = std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&self.cwd_raw)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                    if s.is_empty() { None } else { Some(s) }
                } else {
                    None
                }
            });
        if new_branch != self.git_branch {
            self.git_branch = new_branch;
            self.cached_header_line = None;
        }
    }

    /// Resolve the effective focus owner for Up/Down and other directional keys.
    #[must_use]
    pub fn focus_owner(&self) -> FocusOwner {
        self.focus.owner(self.focus_context())
    }

    #[must_use]
    pub fn is_help_active(&self) -> bool {
        self.input.text().trim() == "?"
    }

    /// Claim key routing for a navigation target.
    /// The latest claimant wins.
    pub fn claim_focus_target(&mut self, target: FocusTarget) {
        let context = self.focus_context();
        self.focus.claim(target, context);
    }

    /// Release key routing claim for a navigation target.
    pub fn release_focus_target(&mut self, target: FocusTarget) {
        let context = self.focus_context();
        self.focus.release(target, context);
    }

    /// Drop claims that are no longer valid for current state.
    pub fn normalize_focus_stack(&mut self) {
        let context = self.focus_context();
        self.focus.normalize(context);
    }

    #[must_use]
    fn focus_context(&self) -> FocusContext {
        FocusContext::new(
            self.show_todo_panel && !self.todos.is_empty(),
            self.mention.is_some() || self.slash.is_some() || self.subagent.is_some(),
            !self.pending_permission_ids.is_empty(),
        )
        .with_help(self.is_help_active())
    }
}

#[cfg(test)]
mod tests {
    // =====
    // TESTS: 26
    // =====

    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    // BlockCache

    #[test]
    fn cache_default_returns_none() {
        let cache = BlockCache::default();
        assert!(cache.get().is_none());
    }

    #[test]
    fn cache_store_then_get() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello")]);
        assert!(cache.get().is_some());
        assert_eq!(cache.get().unwrap().len(), 1);
    }

    #[test]
    fn cache_invalidate_then_get_returns_none() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("data")]);
        cache.invalidate();
        assert!(cache.get().is_none());
    }

    // BlockCache

    #[test]
    fn cache_store_after_invalidate() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("old")]);
        cache.invalidate();
        assert!(cache.get().is_none());
        cache.store(vec![Line::from("new")]);
        let lines = cache.get().unwrap();
        assert_eq!(lines.len(), 1);
        let span_content: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(span_content, "new");
    }

    #[test]
    fn cache_multiple_invalidations() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("data")]);
        cache.invalidate();
        cache.invalidate();
        cache.invalidate();
        assert!(cache.get().is_none());
        cache.store(vec![Line::from("fresh")]);
        assert!(cache.get().is_some());
    }

    #[test]
    fn cache_store_empty_lines() {
        let mut cache = BlockCache::default();
        cache.store(Vec::new());
        let lines = cache.get().unwrap();
        assert!(lines.is_empty());
    }

    /// Store twice without invalidating - second store overwrites first.
    #[test]
    fn cache_store_overwrite_without_invalidate() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("first")]);
        cache.store(vec![Line::from("second"), Line::from("line2")]);
        let lines = cache.get().unwrap();
        assert_eq!(lines.len(), 2);
        let content: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(content, "second");
    }

    /// `get()` called twice returns consistent data.
    #[test]
    fn cache_get_twice_consistent() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("stable")]);
        let first = cache.get().unwrap().len();
        let second = cache.get().unwrap().len();
        assert_eq!(first, second);
    }

    // BlockCache

    #[test]
    fn cache_store_many_lines() {
        let mut cache = BlockCache::default();
        let lines: Vec<Line<'static>> =
            (0..1000).map(|i| Line::from(Span::raw(format!("line {i}")))).collect();
        cache.store(lines);
        assert_eq!(cache.get().unwrap().len(), 1000);
    }

    #[test]
    fn cache_store_splits_into_kb_segments() {
        let mut cache = BlockCache::default();
        let long = "x".repeat(800);
        let lines: Vec<Line<'static>> = (0..12).map(|_| Line::from(long.clone())).collect();
        cache.store(lines);
        assert!(cache.segment_count() > 1);
        assert!(cache.cached_bytes() > 0);
    }

    #[test]
    fn cache_invalidate_without_store() {
        let mut cache = BlockCache::default();
        cache.invalidate();
        assert!(cache.get().is_none());
    }

    #[test]
    fn cache_rapid_store_invalidate_cycle() {
        let mut cache = BlockCache::default();
        for i in 0..50 {
            cache.store(vec![Line::from(format!("v{i}"))]);
            assert!(cache.get().is_some());
            cache.invalidate();
            assert!(cache.get().is_none());
        }
        cache.store(vec![Line::from("final")]);
        assert!(cache.get().is_some());
    }

    /// Store styled lines with multiple spans per line.
    #[test]
    fn cache_store_styled_lines() {
        let mut cache = BlockCache::default();
        let line = Line::from(vec![
            Span::styled("bold", Style::default().fg(Color::Red)),
            Span::raw(" normal "),
            Span::styled("blue", Style::default().fg(Color::Blue)),
        ]);
        cache.store(vec![line]);
        let lines = cache.get().unwrap();
        assert_eq!(lines[0].spans.len(), 3);
    }

    /// Version counter after many invalidations - verify it doesn't
    /// accidentally wrap to 0 (which would make stale data appear fresh).
    /// With u64, 10K invalidations is nowhere near overflow.
    #[test]
    fn cache_version_no_false_fresh_after_many_invalidations() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("data")]);
        for _ in 0..10_000 {
            cache.invalidate();
        }
        // Cache was invalidated 10K times without re-storing - must be stale
        assert!(cache.get().is_none());
    }

    /// Invalidate, store, invalidate, store - alternating pattern.
    #[test]
    fn cache_alternating_invalidate_store() {
        let mut cache = BlockCache::default();
        for i in 0..100 {
            cache.invalidate();
            assert!(cache.get().is_none(), "stale after invalidate at iter {i}");
            cache.store(vec![Line::from(format!("v{i}"))]);
            assert!(cache.get().is_some(), "fresh after store at iter {i}");
        }
    }

    // BlockCache height

    #[test]
    fn cache_height_default_returns_none() {
        let cache = BlockCache::default();
        assert!(cache.height_at(80).is_none());
    }

    #[test]
    fn cache_store_with_height_then_height_at() {
        let mut cache = BlockCache::default();
        cache.store_with_height(vec![Line::from("hello")], 1, 80);
        assert_eq!(cache.height_at(80), Some(1));
        assert!(cache.get().is_some());
    }

    #[test]
    fn cache_height_at_wrong_width_returns_none() {
        let mut cache = BlockCache::default();
        cache.store_with_height(vec![Line::from("hello")], 1, 80);
        assert!(cache.height_at(120).is_none());
    }

    #[test]
    fn cache_height_invalidated_returns_none() {
        let mut cache = BlockCache::default();
        cache.store_with_height(vec![Line::from("hello")], 1, 80);
        cache.invalidate();
        assert!(cache.height_at(80).is_none());
    }

    #[test]
    fn cache_store_without_height_has_no_height() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello")]);
        // store() without height leaves wrapped_width at 0
        assert!(cache.height_at(80).is_none());
    }

    #[test]
    fn cache_store_with_height_overwrite() {
        let mut cache = BlockCache::default();
        cache.store_with_height(vec![Line::from("old")], 1, 80);
        cache.invalidate();
        cache.store_with_height(vec![Line::from("new long line")], 3, 120);
        assert_eq!(cache.height_at(120), Some(3));
        assert!(cache.height_at(80).is_none());
    }

    // BlockCache set_height (separate from store)

    #[test]
    fn cache_set_height_after_store() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello")]);
        assert!(cache.height_at(80).is_none()); // no height yet
        cache.set_height(1, 80);
        assert_eq!(cache.height_at(80), Some(1));
        assert!(cache.get().is_some()); // lines still valid
    }

    #[test]
    fn cache_set_height_update_width() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello world")]);
        cache.set_height(1, 80);
        assert_eq!(cache.height_at(80), Some(1));
        // Re-measure at new width
        cache.set_height(2, 40);
        assert_eq!(cache.height_at(40), Some(2));
        assert!(cache.height_at(80).is_none()); // old width no longer valid
    }

    #[test]
    fn cache_set_height_invalidate_clears_height() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("data")]);
        cache.set_height(3, 80);
        cache.invalidate();
        assert!(cache.height_at(80).is_none()); // version mismatch
    }

    #[test]
    fn cache_set_height_on_invalidated_cache_returns_none() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("data")]);
        cache.invalidate(); // version != 0
        cache.set_height(5, 80);
        // height_at returns None because cache is stale (version != 0)
        assert!(cache.height_at(80).is_none());
    }

    #[test]
    fn cache_store_then_set_height_matches_store_with_height() {
        let mut cache_a = BlockCache::default();
        cache_a.store(vec![Line::from("test")]);
        cache_a.set_height(2, 100);

        let mut cache_b = BlockCache::default();
        cache_b.store_with_height(vec![Line::from("test")], 2, 100);

        assert_eq!(cache_a.height_at(100), cache_b.height_at(100));
        assert_eq!(cache_a.get().unwrap().len(), cache_b.get().unwrap().len());
    }

    #[test]
    fn cache_measure_and_set_height_from_segments() {
        let mut cache = BlockCache::default();
        let lines = vec![
            Line::from("alpha beta gamma delta epsilon"),
            Line::from("zeta eta theta iota kappa lambda"),
            Line::from("mu nu xi omicron pi rho sigma"),
        ];
        cache.store(lines.clone());
        let measured = cache.measure_and_set_height(16).expect("expected measured height");
        let expected = ratatui::widgets::Paragraph::new(ratatui::text::Text::from(lines))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .line_count(16);
        assert_eq!(measured, expected);
        assert_eq!(cache.height_at(16), Some(expected));
    }

    #[test]
    fn cache_get_updates_last_access_tick() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("tick")]);
        let before = cache.last_access_tick();
        let _ = cache.get();
        let after = cache.last_access_tick();
        assert!(after > before);
    }

    // App tool_call_index

    fn make_test_app() -> App {
        App::test_default()
    }

    fn assistant_text_block(text: &str) -> MessageBlock {
        MessageBlock::Text(
            text.to_owned(),
            BlockCache::default(),
            IncrementalMarkdown::from_complete(text),
        )
    }

    fn user_text_message(text: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::User,
            blocks: vec![assistant_text_block(text)],
            usage: None,
        }
    }

    fn assistant_tool_message(id: &str, status: model::ToolCallStatus) -> ChatMessage {
        ChatMessage {
            role: MessageRole::Assistant,
            blocks: vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
                id: id.to_owned(),
                title: format!("tool {id}"),
                sdk_tool_name: "Read".to_owned(),
                raw_input: None,
                status,
                content: Vec::new(),
                collapsed: false,
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: Some("x".repeat(1024)),
                terminal_output_len: 1024,
                terminal_bytes_seen: 1024,
                terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
                render_epoch: 0,
                layout_epoch: 0,
                last_measured_width: 0,
                last_measured_height: 0,
                last_measured_layout_epoch: 0,
                last_measured_layout_generation: 0,
                cache: BlockCache::default(),
                pending_permission: None,
            }))],
            usage: None,
        }
    }

    fn assistant_tool_message_with_pending_permission(id: &str) -> ChatMessage {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        ChatMessage {
            role: MessageRole::Assistant,
            blocks: vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
                id: id.to_owned(),
                title: format!("tool {id}"),
                sdk_tool_name: "Read".to_owned(),
                raw_input: None,
                status: model::ToolCallStatus::Completed,
                content: Vec::new(),
                collapsed: false,
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: Some("x".repeat(1024)),
                terminal_output_len: 1024,
                terminal_bytes_seen: 1024,
                terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
                render_epoch: 0,
                layout_epoch: 0,
                last_measured_width: 0,
                last_measured_height: 0,
                last_measured_layout_epoch: 0,
                last_measured_layout_generation: 0,
                cache: BlockCache::default(),
                pending_permission: Some(InlinePermission {
                    options: vec![model::PermissionOption::new(
                        "allow-once",
                        "Allow once",
                        model::PermissionOptionKind::AllowOnce,
                    )],
                    response_tx: tx,
                    selected_index: 0,
                    focused: false,
                }),
            }))],
            usage: None,
        }
    }

    #[test]
    fn enforce_render_cache_budget_evicts_lru_block() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage {
                role: MessageRole::Assistant,
                blocks: vec![assistant_text_block("a")],
                usage: None,
            },
            ChatMessage {
                role: MessageRole::Assistant,
                blocks: vec![assistant_text_block("b")],
                usage: None,
            },
        ];

        let bytes_a = if let MessageBlock::Text(_, cache, _) = &mut app.messages[0].blocks[0] {
            cache.store(vec![Line::from("x".repeat(2200))]);
            cache.cached_bytes()
        } else {
            0
        };
        let bytes_b = if let MessageBlock::Text(_, cache, _) = &mut app.messages[1].blocks[0] {
            cache.store(vec![Line::from("y".repeat(2200))]);
            let _ = cache.get();
            cache.cached_bytes()
        } else {
            0
        };

        app.render_cache_budget.max_bytes = bytes_b;
        let stats = app.enforce_render_cache_budget();
        assert!(stats.evicted_blocks >= 1);
        assert!(stats.evicted_bytes >= bytes_a);
        assert!(stats.total_after_bytes <= app.render_cache_budget.max_bytes);
        assert_eq!(stats.protected_bytes, 0);

        if let MessageBlock::Text(_, cache, _) = &app.messages[0].blocks[0] {
            assert_eq!(cache.cached_bytes(), 0);
        } else {
            panic!("expected text block");
        }
        if let MessageBlock::Text(_, cache, _) = &app.messages[1].blocks[0] {
            assert_eq!(cache.cached_bytes(), bytes_b);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_protects_streaming_tail_message() {
        let mut app = make_test_app();
        app.status = AppStatus::Thinking;
        app.messages = vec![ChatMessage {
            role: MessageRole::Assistant,
            blocks: vec![assistant_text_block("streaming tail")],
            usage: None,
        }];

        let before = if let MessageBlock::Text(_, cache, _) = &mut app.messages[0].blocks[0] {
            cache.store(vec![Line::from("z".repeat(4096))]);
            cache.cached_bytes()
        } else {
            0
        };
        app.render_cache_budget.max_bytes = 64;
        let stats = app.enforce_render_cache_budget();
        assert_eq!(stats.evicted_blocks, 0);
        assert_eq!(stats.evicted_bytes, 0);
        assert_eq!(stats.protected_bytes, before);

        if let MessageBlock::Text(_, cache, _) = &app.messages[0].blocks[0] {
            assert_eq!(cache.cached_bytes(), before);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_excludes_protected_from_budget() {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.messages = vec![
            ChatMessage {
                role: MessageRole::Assistant,
                blocks: vec![assistant_text_block("old message")],
                usage: None,
            },
            ChatMessage {
                role: MessageRole::Assistant,
                blocks: vec![assistant_text_block("streaming tail")],
                usage: None,
            },
        ];

        let bytes_a = if let MessageBlock::Text(_, cache, _) = &mut app.messages[0].blocks[0] {
            cache.store(vec![Line::from("x".repeat(2200))]);
            cache.cached_bytes()
        } else {
            0
        };
        let bytes_b = if let MessageBlock::Text(_, cache, _) = &mut app.messages[1].blocks[0] {
            cache.store(vec![Line::from("y".repeat(5000))]);
            cache.cached_bytes()
        } else {
            0
        };

        // Budget fits old message alone but not old + tail combined.
        app.render_cache_budget.max_bytes = bytes_a + 100;
        assert!(bytes_a + bytes_b > app.render_cache_budget.max_bytes);

        let stats = app.enforce_render_cache_budget();

        // Protected bytes should be the streaming tail.
        assert_eq!(stats.protected_bytes, bytes_b);
        // No eviction: budgeted bytes (bytes_a) are under max_bytes.
        assert_eq!(stats.evicted_blocks, 0);
        assert_eq!(stats.evicted_bytes, 0);
        // Old message cache intact.
        if let MessageBlock::Text(_, cache, _) = &app.messages[0].blocks[0] {
            assert_eq!(cache.cached_bytes(), bytes_a);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_evicts_when_budgeted_over_limit() {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.messages = vec![
            ChatMessage {
                role: MessageRole::Assistant,
                blocks: vec![assistant_text_block("old-a")],
                usage: None,
            },
            ChatMessage {
                role: MessageRole::Assistant,
                blocks: vec![assistant_text_block("old-b")],
                usage: None,
            },
            ChatMessage {
                role: MessageRole::Assistant,
                blocks: vec![assistant_text_block("streaming")],
                usage: None,
            },
        ];

        // Populate caches: messages 0 and 1 evictable, message 2 protected.
        if let MessageBlock::Text(_, cache, _) = &mut app.messages[0].blocks[0] {
            cache.store(vec![Line::from("x".repeat(3000))]);
        }
        let bytes_b = if let MessageBlock::Text(_, cache, _) = &mut app.messages[1].blocks[0] {
            cache.store(vec![Line::from("y".repeat(3000))]);
            let _ = cache.get(); // touch to make more recently accessed
            cache.cached_bytes()
        } else {
            0
        };
        let bytes_c = if let MessageBlock::Text(_, cache, _) = &mut app.messages[2].blocks[0] {
            cache.store(vec![Line::from("z".repeat(5000))]);
            cache.cached_bytes()
        } else {
            0
        };

        // Budget fits message B but not A+B (excludes C as protected).
        app.render_cache_budget.max_bytes = bytes_b + 100;

        let stats = app.enforce_render_cache_budget();

        assert_eq!(stats.protected_bytes, bytes_c);
        assert!(stats.evicted_blocks >= 1); // message A evicted (older access)
        // Message B should survive (more recent access).
        if let MessageBlock::Text(_, cache, _) = &app.messages[1].blocks[0] {
            assert_eq!(cache.cached_bytes(), bytes_b);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_protected_bytes_zero_when_not_streaming() {
        let mut app = make_test_app();
        app.status = AppStatus::Ready;
        app.messages = vec![ChatMessage {
            role: MessageRole::Assistant,
            blocks: vec![assistant_text_block("done")],
            usage: None,
        }];

        if let MessageBlock::Text(_, cache, _) = &mut app.messages[0].blocks[0] {
            cache.store(vec![Line::from("x".repeat(2000))]);
        }
        app.render_cache_budget.max_bytes = usize::MAX;

        let stats = app.enforce_render_cache_budget();
        assert_eq!(stats.protected_bytes, 0);
    }

    #[test]
    fn enforce_history_retention_noop_under_budget() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome("model", "/cwd"),
            user_text_message("small message"),
            user_text_message("another message"),
        ];
        app.history_retention.max_bytes = usize::MAX / 4;

        let stats = app.enforce_history_retention();
        assert_eq!(stats.dropped_messages, 0);
        assert_eq!(stats.total_dropped_messages, 0);
        assert!(!app.messages.iter().any(App::is_history_hidden_marker_message));
    }

    #[test]
    fn enforce_history_retention_drops_oldest_and_adds_marker() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome("model", "/cwd"),
            user_text_message("first old message"),
            user_text_message("second old message"),
            user_text_message("third old message"),
        ];
        app.history_retention.max_bytes = 1;

        let stats = app.enforce_history_retention();
        assert_eq!(stats.dropped_messages, 3);
        assert!(matches!(app.messages[0].role, MessageRole::Welcome));
        assert!(app.messages.iter().any(App::is_history_hidden_marker_message));
        assert_eq!(app.messages.len(), 2);
    }

    #[test]
    fn enforce_history_retention_preserves_in_progress_tool_message() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome("model", "/cwd"),
            user_text_message("droppable"),
            assistant_tool_message("tool-keep", model::ToolCallStatus::InProgress),
        ];
        app.history_retention.max_bytes = 1;

        let stats = app.enforce_history_retention();
        assert_eq!(stats.dropped_messages, 1);
        assert!(app.messages.iter().any(|msg| {
            msg.blocks.iter().any(|block| {
                matches!(
                    block,
                    MessageBlock::ToolCall(tc) if tc.id == "tool-keep"
                        && matches!(tc.status, model::ToolCallStatus::InProgress)
                )
            })
        }));
    }

    #[test]
    fn enforce_history_retention_preserves_pending_tool_message() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome("model", "/cwd"),
            user_text_message("droppable"),
            assistant_tool_message("tool-pending", model::ToolCallStatus::Pending),
        ];
        app.history_retention.max_bytes = 1;

        let stats = app.enforce_history_retention();
        assert_eq!(stats.dropped_messages, 1);
        assert!(app.messages.iter().any(|msg| {
            msg.blocks
                .iter()
                .any(|block| matches!(block, MessageBlock::ToolCall(tc) if tc.id == "tool-pending"))
        }));
    }

    #[test]
    fn enforce_history_retention_preserves_permission_tool_message() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome("model", "/cwd"),
            user_text_message("droppable"),
            assistant_tool_message_with_pending_permission("tool-perm"),
        ];
        app.history_retention.max_bytes = 1;

        let stats = app.enforce_history_retention();
        assert_eq!(stats.dropped_messages, 1);
        assert!(app.messages.iter().any(|msg| {
            msg.blocks
                .iter()
                .any(|block| matches!(block, MessageBlock::ToolCall(tc) if tc.id == "tool-perm"))
        }));
    }

    #[test]
    fn enforce_history_retention_rebuilds_tool_index_after_prune() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome("model", "/cwd"),
            user_text_message("drop this"),
            assistant_tool_message("tool-idx", model::ToolCallStatus::InProgress),
        ];
        app.index_tool_call("tool-idx".to_owned(), 99, 99);
        app.history_retention.max_bytes = 1;

        let _ = app.enforce_history_retention();
        assert_eq!(app.lookup_tool_call("tool-idx"), Some((2, 0)));
    }

    #[test]
    fn enforce_history_retention_keeps_single_marker_on_repeat() {
        let mut app = make_test_app();
        app.messages = vec![ChatMessage::welcome("model", "/cwd"), user_text_message("drop me")];
        app.history_retention.max_bytes = 1;

        let first = app.enforce_history_retention();
        let second = app.enforce_history_retention();
        let marker_count =
            app.messages.iter().filter(|msg| App::is_history_hidden_marker_message(msg)).count();

        assert_eq!(first.dropped_messages, 1);
        assert_eq!(second.dropped_messages, 0);
        assert_eq!(marker_count, 1);
    }

    #[test]
    fn lookup_missing_returns_none() {
        let app = make_test_app();
        assert!(app.lookup_tool_call("nonexistent").is_none());
    }

    #[test]
    fn index_and_lookup() {
        let mut app = make_test_app();
        app.index_tool_call("tc-123".into(), 2, 5);
        assert_eq!(app.lookup_tool_call("tc-123"), Some((2, 5)));
    }

    // App tool_call_index

    /// Index same ID twice - second write overwrites first.
    #[test]
    fn index_overwrite_existing() {
        let mut app = make_test_app();
        app.index_tool_call("tc-1".into(), 0, 0);
        app.index_tool_call("tc-1".into(), 5, 10);
        assert_eq!(app.lookup_tool_call("tc-1"), Some((5, 10)));
    }

    /// Empty string as tool call ID.
    #[test]
    fn index_empty_string_id() {
        let mut app = make_test_app();
        app.index_tool_call(String::new(), 1, 2);
        assert_eq!(app.lookup_tool_call(""), Some((1, 2)));
    }

    /// Stress: 1000 tool calls indexed and looked up.
    #[test]
    fn index_stress_1000_entries() {
        let mut app = make_test_app();
        for i in 0..1000 {
            app.index_tool_call(format!("tc-{i}"), i, i * 2);
        }
        // Spot check first, middle, last
        assert_eq!(app.lookup_tool_call("tc-0"), Some((0, 0)));
        assert_eq!(app.lookup_tool_call("tc-500"), Some((500, 1000)));
        assert_eq!(app.lookup_tool_call("tc-999"), Some((999, 1998)));
        // Non-existent still returns None
        assert!(app.lookup_tool_call("tc-1000").is_none());
    }

    /// Unicode in tool call ID.
    #[test]
    fn index_unicode_id() {
        let mut app = make_test_app();
        app.index_tool_call("\u{1F600}-tool".into(), 3, 7);
        assert_eq!(app.lookup_tool_call("\u{1F600}-tool"), Some((3, 7)));
    }

    // active_task_ids

    #[test]
    fn active_task_insert_remove() {
        let mut app = make_test_app();
        app.insert_active_task("task-1".into());
        assert!(app.active_task_ids.contains("task-1"));
        app.remove_active_task("task-1");
        assert!(!app.active_task_ids.contains("task-1"));
    }

    #[test]
    fn remove_nonexistent_task_is_noop() {
        let mut app = make_test_app();
        app.remove_active_task("does-not-exist");
        assert!(app.active_task_ids.is_empty());
    }

    // active_task_ids

    /// Insert same ID twice - set deduplicates; one remove clears it.
    #[test]
    fn active_task_insert_duplicate() {
        let mut app = make_test_app();
        app.insert_active_task("task-1".into());
        app.insert_active_task("task-1".into());
        assert_eq!(app.active_task_ids.len(), 1);
        app.remove_active_task("task-1");
        assert!(app.active_task_ids.is_empty());
    }

    /// Insert many tasks, remove in different order.
    #[test]
    fn active_task_insert_many_remove_out_of_order() {
        let mut app = make_test_app();
        for i in 0..100 {
            app.insert_active_task(format!("task-{i}"));
        }
        assert_eq!(app.active_task_ids.len(), 100);
        // Remove in reverse order
        for i in (0..100).rev() {
            app.remove_active_task(&format!("task-{i}"));
        }
        assert!(app.active_task_ids.is_empty());
    }

    /// Mixed insert/remove interleaving.
    #[test]
    fn active_task_interleaved_insert_remove() {
        let mut app = make_test_app();
        app.insert_active_task("a".into());
        app.insert_active_task("b".into());
        app.remove_active_task("a");
        app.insert_active_task("c".into());
        assert!(!app.active_task_ids.contains("a"));
        assert!(app.active_task_ids.contains("b"));
        assert!(app.active_task_ids.contains("c"));
        assert_eq!(app.active_task_ids.len(), 2);
    }

    /// Remove from empty set multiple times - no panic.
    #[test]
    fn active_task_remove_from_empty_repeatedly() {
        let mut app = make_test_app();
        for i in 0..100 {
            app.remove_active_task(&format!("ghost-{i}"));
        }
        assert!(app.active_task_ids.is_empty());
    }

    /// `clear_tool_scope_tracking` must also clear `active_task_ids`.
    /// Regression test: before the fix, a leaked task ID from a cancelled turn
    /// caused main-agent tools on the next turn to be misclassified as Subagent scope.
    #[test]
    fn clear_tool_scope_tracking_also_clears_active_task_ids() {
        let mut app = make_test_app();
        app.insert_active_task("task-leaked".into());
        assert!(!app.active_task_ids.is_empty());
        app.clear_tool_scope_tracking();
        assert!(app.active_task_ids.is_empty(), "active_task_ids must be cleared at turn end");
        assert!(app.active_subagent_tool_ids.is_empty());
        assert!(app.subagent_idle_since.is_none());
    }

    // IncrementalMarkdown

    /// Simple render function for tests: wraps each line in a `Line`.
    fn test_render(src: &str) -> Vec<Line<'static>> {
        src.lines().map(|l| Line::from(l.to_owned())).collect()
    }

    #[test]
    fn incr_default_empty() {
        let incr = IncrementalMarkdown::default();
        assert!(incr.full_text().is_empty());
    }

    #[test]
    fn incr_from_complete() {
        let incr = IncrementalMarkdown::from_complete("hello world");
        assert_eq!(incr.full_text(), "hello world");
    }

    #[test]
    fn incr_append_single_chunk() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("hello");
        assert_eq!(incr.full_text(), "hello");
    }

    #[test]
    fn incr_append_accumulates_chunks() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("line1");
        incr.append("\nline2");
        incr.append("\nline3");
        assert_eq!(incr.full_text(), "line1\nline2\nline3");
    }

    #[test]
    fn incr_append_preserves_paragraph_delimiters() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("para1\n\npara2");
        assert_eq!(incr.full_text(), "para1\n\npara2");
    }

    #[test]
    fn incr_full_text_reconstruction() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("p1\n\np2\n\np3");
        assert_eq!(incr.full_text(), "p1\n\np2\n\np3");
    }

    #[test]
    fn incr_lines_renders_all() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("line1\n\nline2\n\nline3");
        let lines = incr.lines(&test_render);
        // test_render maps each source line to one output line
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn incr_ensure_rendered_noop_preserves_text() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("p1\n\np2\n\ntail");
        incr.ensure_rendered(&test_render);
        assert_eq!(incr.full_text(), "p1\n\np2\n\ntail");
    }

    #[test]
    fn incr_invalidate_renders_noop_preserves_text() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("p1\n\np2\n\ntail");
        incr.invalidate_renders();
        assert_eq!(incr.full_text(), "p1\n\np2\n\ntail");
    }

    #[test]
    fn incr_streaming_simulation() {
        // Simulate a realistic streaming scenario
        let mut incr = IncrementalMarkdown::default();
        let chunks = ["Here is ", "some text.\n", "\nNext para", "graph here.\n\n", "Final."];
        for chunk in chunks {
            incr.append(chunk);
        }
        assert_eq!(incr.full_text(), "Here is some text.\n\nNext paragraph here.\n\nFinal.");
    }

    // ChatViewport

    #[test]
    fn viewport_new_defaults() {
        let vp = ChatViewport::new();
        assert_eq!(vp.scroll_offset, 0);
        assert_eq!(vp.scroll_target, 0);
        assert!(vp.auto_scroll);
        assert_eq!(vp.width, 0);
        assert!(vp.message_heights.is_empty());
        assert!(vp.dirty_from.is_none());
        assert!(vp.height_prefix_sums.is_empty());
    }

    #[test]
    fn viewport_on_frame_sets_width() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        assert_eq!(vp.width, 80);
    }

    #[test]
    fn viewport_on_frame_resize_invalidates() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(0, 10);
        vp.set_message_height(1, 20);
        vp.rebuild_prefix_sums();

        // Resize: old heights are kept as approximations,
        // but width markers are invalidated so re-measurement happens.
        vp.on_frame(120);
        assert_eq!(vp.message_height(0), 10); // kept, not zeroed
        assert_eq!(vp.message_height(1), 20); // kept, not zeroed
        assert_eq!(vp.message_heights_width, 0); // forces re-measure
        assert_eq!(vp.prefix_sums_width, 0); // forces rebuild
    }

    #[test]
    fn viewport_on_frame_same_width_no_invalidation() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(0, 10);
        vp.on_frame(80); // same width
        assert_eq!(vp.message_height(0), 10); // not zeroed
    }

    #[test]
    fn viewport_message_height_set_and_get() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(0, 5);
        vp.set_message_height(1, 10);
        assert_eq!(vp.message_height(0), 5);
        assert_eq!(vp.message_height(1), 10);
        assert_eq!(vp.message_height(2), 0); // out of bounds
    }

    #[test]
    fn viewport_message_height_grows_vec() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(5, 42);
        assert_eq!(vp.message_heights.len(), 6);
        assert_eq!(vp.message_height(5), 42);
        assert_eq!(vp.message_height(3), 0); // gap filled with 0
    }

    #[test]
    fn viewport_mark_message_dirty_tracks_oldest_index() {
        let mut vp = ChatViewport::new();
        vp.mark_message_dirty(5);
        vp.mark_message_dirty(2);
        vp.mark_message_dirty(7);
        assert_eq!(vp.dirty_from, Some(2));
    }

    #[test]
    fn viewport_mark_heights_valid_clears_dirty_index() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.mark_message_dirty(1);
        assert_eq!(vp.dirty_from, Some(1));
        vp.mark_heights_valid();
        assert!(vp.dirty_from.is_none());
    }

    #[test]
    fn viewport_prefix_sums_basic() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(0, 5);
        vp.set_message_height(1, 10);
        vp.set_message_height(2, 3);
        vp.rebuild_prefix_sums();
        assert_eq!(vp.total_message_height(), 18);
        assert_eq!(vp.cumulative_height_before(0), 0);
        assert_eq!(vp.cumulative_height_before(1), 5);
        assert_eq!(vp.cumulative_height_before(2), 15);
    }

    #[test]
    fn viewport_prefix_sums_streaming_fast_path() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(0, 5);
        vp.set_message_height(1, 10);
        vp.rebuild_prefix_sums();
        assert_eq!(vp.total_message_height(), 15);

        // Simulate streaming: last message grows
        vp.set_message_height(1, 20);
        vp.rebuild_prefix_sums(); // should hit fast path
        assert_eq!(vp.total_message_height(), 25);
        assert_eq!(vp.cumulative_height_before(1), 5);
    }

    #[test]
    fn viewport_find_first_visible() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(0, 10);
        vp.set_message_height(1, 10);
        vp.set_message_height(2, 10);
        vp.rebuild_prefix_sums();

        assert_eq!(vp.find_first_visible(0), 0);
        assert_eq!(vp.find_first_visible(10), 1);
        assert_eq!(vp.find_first_visible(15), 1);
        assert_eq!(vp.find_first_visible(20), 2);
    }

    #[test]
    fn viewport_find_first_visible_handles_offsets_before_first_boundary() {
        let mut vp = ChatViewport::new();
        vp.on_frame(80);
        vp.set_message_height(0, 10);
        vp.set_message_height(1, 10);
        vp.rebuild_prefix_sums();

        assert_eq!(vp.find_first_visible(0), 0);
        assert_eq!(vp.find_first_visible(5), 0);
        assert_eq!(vp.find_first_visible(15), 1);
    }

    #[test]
    fn viewport_scroll_up_down() {
        let mut vp = ChatViewport::new();
        vp.scroll_target = 20;
        vp.auto_scroll = true;

        vp.scroll_up(5);
        assert_eq!(vp.scroll_target, 15);
        assert!(!vp.auto_scroll); // disabled on manual scroll

        vp.scroll_down(3);
        assert_eq!(vp.scroll_target, 18);
        assert!(!vp.auto_scroll); // not re-engaged by scroll_down
    }

    #[test]
    fn viewport_scroll_up_saturates() {
        let mut vp = ChatViewport::new();
        vp.scroll_target = 2;
        vp.scroll_up(10);
        assert_eq!(vp.scroll_target, 0);
    }

    #[test]
    fn viewport_engage_auto_scroll() {
        let mut vp = ChatViewport::new();
        vp.auto_scroll = false;
        vp.engage_auto_scroll();
        assert!(vp.auto_scroll);
    }

    #[test]
    fn viewport_default_eq_new() {
        let a = ChatViewport::new();
        let b = ChatViewport::default();
        assert_eq!(a.width, b.width);
        assert_eq!(a.auto_scroll, b.auto_scroll);
        assert_eq!(a.message_heights.len(), b.message_heights.len());
    }

    #[test]
    fn focus_owner_defaults_to_input() {
        let app = make_test_app();
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }

    #[test]
    fn focus_owner_todo_when_panel_open_and_focused() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        assert_eq!(app.focus_owner(), FocusOwner::TodoList);
    }

    #[test]
    fn focus_owner_permission_overrides_todo() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        app.pending_permission_ids.push("perm-1".into());
        app.claim_focus_target(FocusTarget::Permission);
        assert_eq!(app.focus_owner(), FocusOwner::Permission);
    }

    #[test]
    fn focus_owner_mention_overrides_permission_and_todo() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        app.pending_permission_ids.push("perm-1".into());
        app.claim_focus_target(FocusTarget::Permission);
        app.mention = Some(mention::MentionState {
            trigger_row: 0,
            trigger_col: 0,
            query: String::new(),
            candidates: Vec::new(),
            dialog: super::dialog::DialogState::default(),
        });
        app.claim_focus_target(FocusTarget::Mention);
        assert_eq!(app.focus_owner(), FocusOwner::Mention);
    }

    #[test]
    fn focus_owner_falls_back_to_input_when_claim_is_not_available() {
        let mut app = make_test_app();
        app.claim_focus_target(FocusTarget::TodoList);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }

    #[test]
    fn claim_and_release_focus_target() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.claim_focus_target(FocusTarget::TodoList);
        assert_eq!(app.focus_owner(), FocusOwner::TodoList);
        app.release_focus_target(FocusTarget::TodoList);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }

    #[test]
    fn latest_claim_wins_across_equal_targets() {
        let mut app = make_test_app();
        app.todos.push(TodoItem {
            content: "Task".into(),
            status: TodoStatus::Pending,
            active_form: String::new(),
        });
        app.show_todo_panel = true;
        app.mention = Some(mention::MentionState {
            trigger_row: 0,
            trigger_col: 0,
            query: String::new(),
            candidates: Vec::new(),
            dialog: super::dialog::DialogState::default(),
        });
        app.pending_permission_ids.push("perm-1".into());

        app.claim_focus_target(FocusTarget::TodoList);
        assert_eq!(app.focus_owner(), FocusOwner::TodoList);

        app.claim_focus_target(FocusTarget::Permission);
        assert_eq!(app.focus_owner(), FocusOwner::Permission);

        app.claim_focus_target(FocusTarget::Mention);
        assert_eq!(app.focus_owner(), FocusOwner::Mention);

        app.release_focus_target(FocusTarget::Mention);
        assert_eq!(app.focus_owner(), FocusOwner::Permission);
    }

    // --- InvalidationLevel tests ---

    #[test]
    fn invalidate_single_tail_preserves_prefix_sums() {
        let mut app = make_test_app();
        app.messages.push(user_text_message("a"));
        app.messages.push(user_text_message("b"));
        app.messages.push(user_text_message("c"));
        app.viewport.on_frame(80);
        app.viewport.set_message_height(0, 5);
        app.viewport.set_message_height(1, 10);
        app.viewport.set_message_height(2, 3);
        app.viewport.rebuild_prefix_sums();
        let prefix_width_before = app.viewport.prefix_sums_width;

        app.invalidate_layout(InvalidationLevel::Single(2)); // tail

        assert_eq!(app.viewport.dirty_from, Some(2));
        assert_eq!(app.viewport.prefix_sums_width, prefix_width_before);
    }

    #[test]
    fn invalidate_single_nontail_invalidates_prefix_sums() {
        let mut app = make_test_app();
        app.messages.push(user_text_message("a"));
        app.messages.push(user_text_message("b"));
        app.messages.push(user_text_message("c"));
        app.viewport.on_frame(80);
        app.viewport.set_message_height(0, 5);
        app.viewport.set_message_height(1, 10);
        app.viewport.set_message_height(2, 3);
        app.viewport.rebuild_prefix_sums();

        app.invalidate_layout(InvalidationLevel::Single(1)); // non-tail

        assert_eq!(app.viewport.dirty_from, Some(1));
        assert_eq!(app.viewport.prefix_sums_width, 0);
    }

    #[test]
    fn invalidate_from_always_invalidates_prefix_sums() {
        let mut app = make_test_app();
        app.messages.push(user_text_message("a"));
        app.messages.push(user_text_message("b"));
        app.messages.push(user_text_message("c"));
        app.viewport.on_frame(80);
        app.viewport.set_message_height(0, 5);
        app.viewport.set_message_height(1, 10);
        app.viewport.set_message_height(2, 3);
        app.viewport.rebuild_prefix_sums();
        assert_ne!(app.viewport.prefix_sums_width, 0);

        // From at tail index still invalidates prefix sums (unlike Single).
        app.invalidate_layout(InvalidationLevel::From(2));

        assert_eq!(app.viewport.dirty_from, Some(2));
        assert_eq!(app.viewport.prefix_sums_width, 0);
    }

    #[test]
    fn invalidate_from_zero_matches_old_mark_all() {
        let mut app = make_test_app();
        app.messages.push(user_text_message("a"));
        app.messages.push(user_text_message("b"));
        app.messages.push(user_text_message("c"));
        app.viewport.on_frame(80);
        app.viewport.set_message_height(0, 5);
        app.viewport.set_message_height(1, 10);
        app.viewport.set_message_height(2, 3);
        app.viewport.rebuild_prefix_sums();

        app.invalidate_layout(InvalidationLevel::From(0));

        assert_eq!(app.viewport.dirty_from, Some(0));
        assert_eq!(app.viewport.prefix_sums_width, 0);
    }

    #[test]
    fn invalidate_global_bumps_generation() {
        let mut app = make_test_app();
        app.messages.push(user_text_message("a"));
        app.messages.push(user_text_message("b"));
        app.messages.push(user_text_message("c"));
        app.viewport.on_frame(80);
        app.viewport.rebuild_prefix_sums();
        let gen_before = app.viewport.layout_generation;

        app.invalidate_layout(InvalidationLevel::Global);

        assert_eq!(app.viewport.dirty_from, Some(0));
        assert_eq!(app.viewport.prefix_sums_width, 0);
        assert_eq!(app.viewport.layout_generation, gen_before + 1);
    }

    #[test]
    fn invalidate_global_noop_on_empty() {
        let mut app = make_test_app();
        assert!(app.messages.is_empty());
        let gen_before = app.viewport.layout_generation;

        app.invalidate_layout(InvalidationLevel::Global);

        assert!(app.viewport.dirty_from.is_none());
        assert_eq!(app.viewport.layout_generation, gen_before);
    }

    #[test]
    fn invalidate_single_watermark_tracks_oldest() {
        let mut app = make_test_app();
        // Need enough messages so all indices are non-tail for consistent behavior.
        for _ in 0..10 {
            app.messages.push(user_text_message("x"));
        }

        app.invalidate_layout(InvalidationLevel::Single(5));
        app.invalidate_layout(InvalidationLevel::Single(2));
        app.invalidate_layout(InvalidationLevel::Single(7));

        assert_eq!(app.viewport.dirty_from, Some(2));
    }

    #[test]
    fn invalidation_level_eq_and_debug() {
        assert_eq!(InvalidationLevel::Single(5), InvalidationLevel::Single(5));
        assert_ne!(InvalidationLevel::Single(5), InvalidationLevel::From(5));
        assert_eq!(InvalidationLevel::Global, InvalidationLevel::Global);
        assert_eq!(InvalidationLevel::Resize, InvalidationLevel::Resize);
        // Debug derive works
        let dbg = format!("{:?}", InvalidationLevel::From(3));
        assert!(dbg.contains("From"));
    }
}
