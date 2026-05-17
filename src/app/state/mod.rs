// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

pub mod block_cache;
pub mod cache_metrics;
pub mod chat_render;
mod history_retention;
pub mod messages;
mod render_budget;
pub mod tool_call_info;
pub mod types;

// Re-export all public types so external `use crate::app::state::X` paths still work.
pub use block_cache::BlockCache;
pub use cache_metrics::CacheMetrics;
pub use chat_render::{
    ChatRenderState, ComposerRenderState, LiveRegionRenderState, TerminalSize, TerminalSizeChange,
};
pub(crate) use messages::MarkdownRenderKey;
pub use messages::{
    ChatMessage, ChatMessageId, HistoryOutputId, ImageAttachmentBlock, IncrementalMarkdown,
    MessageBlock, MessageBlockId, MessageRole, NoticeBlock, NoticeDedupKey, RateLimitIncidentKey,
    SystemSeverity, TextBlock, TextBlockSpacing, WelcomeBlock, hash_text_block_content,
    hash_welcome_block_content,
};
pub use tool_call_info::{
    InlinePermission, InlineQuestion, TerminalSnapshotMode, ToolCallInfo, is_execute_tool_name,
};
pub use types::{
    AppStatus, CancelOrigin, ExtraUsage, HistoryRetentionPolicy, HistoryRetentionStats, LoginHint,
    McpState, MessageUsage, ModeInfo, ModeState, PasteSessionState, PendingCommandAck,
    RecentSessionInfo, RenderCacheBudget, SelectionPoint, SessionPickerState, SessionUsageState,
    TodoItem, TodoStatus, ToolCallScope, UpdateNoticeState, UsageSnapshot, UsageSourceKind,
    UsageSourceMode, UsageState, UsageWindow,
};

use crate::agent::events::ClientEvent;
use crate::agent::model;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;
use std::time::Instant;
use tokio::sync::mpsc;

use super::config::ConfigState;
use super::file_index;
use super::focus::{FocusContext, FocusManager, FocusOwner, FocusTarget};
use super::git_context::GitContextState;
use super::inline_interactions::{clear_inline_interaction_focus, focus_next_inline_interaction};
use super::input::{InputSnapshot, InputState, parse_paste_placeholder_before_cursor};
use super::mention;
use super::plugins::PluginsState;
use super::slash;
use super::subagent;
use super::trust::TrustState;
use super::view::SurfaceMode;
use super::{SurfaceDirtyState, TerminalLifecycleState};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TerminalToolCallRef {
    pub terminal_id: String,
    pub msg_idx: usize,
    pub block_idx: usize,
}

impl TerminalToolCallRef {
    #[must_use]
    pub fn new(terminal_id: String, msg_idx: usize, block_idx: usize) -> Self {
        Self { terminal_id, msg_idx, block_idx }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompleteKind {
    Mention,
    Slash,
    Subagent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeStage {
    Warning,
    Rejected,
    PlanLimitTurnError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnNoticeLocation {
    Inline { msg_idx: usize, block_idx: usize },
    Standalone { msg_idx: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnNoticeRef {
    pub dedup_key: NoticeDedupKey,
    pub stage: NoticeStage,
    pub location: TurnNoticeLocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatRenderTraceState {
    pub width: u16,
    pub content_height: usize,
    pub viewport_height: usize,
    pub auto_scroll: bool,
    pub pinned_to_bottom: bool,
    pub scroll_target: usize,
    pub scroll_offset: usize,
    pub max_scroll: usize,
    pub first_visible: usize,
    pub render_start: usize,
    pub local_scroll: usize,
    pub rendered_msgs: usize,
    pub last_rendered_idx: Option<usize>,
    pub rendered_line_count: usize,
    pub last_message_idx: Option<usize>,
    pub last_message_height: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutInvalidation {
    MessageChanged(usize),
    MessagesFrom(usize),
    Global,
}

pub use LayoutInvalidation as InvalidationLevel;

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub surface_mode: SurfaceMode,
    pub terminal_lifecycle: TerminalLifecycleState,
    pub surface_dirty: SurfaceDirtyState,
    pub config: ConfigState,
    pub trust: TrustState,
    pub settings_home_override: Option<PathBuf>,
    pub messages: Vec<ChatMessage>,
    /// Cached approximate retained bytes for each message, parallel to `messages`.
    pub message_retained_bytes: Vec<usize>,
    /// Rolling total of `message_retained_bytes`.
    pub retained_history_bytes: usize,
    pub input: InputState,
    pub status: AppStatus,
    /// Session id currently being resumed via `/resume`.
    pub resuming_session_id: Option<String>,
    /// Whether the synthetic session overview is eligible for chat transcript output.
    pub show_session_overview: bool,
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
    /// Monotonic session authority epoch used to ignore stale async view data.
    pub session_scope_epoch: u64,
    pub current_model: Option<model::CurrentModel>,
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
    /// Tool call IDs with pending inline interactions, ordered by arrival.
    /// The first entry is the focused interaction that receives keyboard input.
    /// Up / Down arrow keys cycle focus through the list.
    pub pending_interaction_ids: Vec<String>,
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
    pub file_index_event_tx: std_mpsc::Sender<file_index::FileIndexEvent>,
    pub file_index_event_rx: std_mpsc::Receiver<file_index::FileIndexEvent>,
    pub spinner_frame: usize,
    pub spinner_last_advance_at: Option<Instant>,
    /// Message index that owns the current main-assistant turn indicators.
    pub active_turn_assistant_message_idx: Option<usize>,
    /// IDs of root Task/Agent tool calls currently `InProgress`.
    /// Use `insert_active_task()`, `remove_active_task()`.
    pub active_task_ids: HashSet<String>,
    /// Tool scope keyed by tool call ID; used to distinguish main-agent, subagent roots,
    /// and explicitly owned subagent child tools.
    pub tool_call_scopes: HashMap<String, ToolCallScope>,
    /// Shared terminal process map - used to snapshot output on completion.
    pub terminals: crate::agent::events::TerminalMap,
    /// O(1) lookup: `tool_call_id` -> `(message_index, block_index)`.
    /// Use `lookup_tool_call()`, `index_tool_call()`.
    pub tool_call_index: HashMap<String, (usize, usize)>,
    /// Current todo list from Claude's `TodoWrite` tool calls.
    pub todos: Vec<TodoItem>,
    /// Focus manager for directional/navigation key ownership.
    pub focus: FocusManager,
    /// Commands advertised by the agent via `AvailableCommandsUpdate`.
    pub available_commands: Vec<model::AvailableCommand>,
    /// Plugin inventory and UI state for the Config > Plugins view.
    pub plugins: PluginsState,
    /// Subagents advertised by the agent via `AvailableAgentsUpdate`.
    pub available_agents: Vec<model::AvailableAgent>,
    /// Models advertised by the agent SDK for the active session.
    pub available_models: Vec<model::AvailableModel>,
    /// Recently persisted session IDs discovered at startup.
    pub recent_sessions: Vec<RecentSessionInfo>,
    /// Selection state for the startup session picker screen.
    pub session_picker: SessionPickerState,
    /// Deterministic measurement state for the future mutable chat region.
    pub chat_render: ChatRenderState,
    /// Active `@` file mention autocomplete state.
    pub mention: Option<mention::MentionState>,
    /// App-owned file index backing `@` file mention autocomplete.
    pub file_index: file_index::FileIndexState,
    /// Active slash-command autocomplete state.
    pub slash: Option<slash::SlashState>,
    /// Active subagent autocomplete state (`&name`).
    pub subagent: Option<subagent::SubagentState>,
    /// Deferred plain-Enter submit. Stores the exact input state from before the
    /// Enter key so submission can restore and use the original draft text.
    ///
    /// If another editing-like event or a paste payload arrives in the same
    /// drain cycle, this is cleared and no submit occurs.
    pub pending_submit: Option<InputSnapshot>,
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
    /// Pending image attachments accumulated via Ctrl+V clipboard reads and
    /// consumed on submit. No cap on count — this is a developer tool, so
    /// users are trusted to attach as many images as they need.
    pub pending_images: Vec<crate::app::clipboard_image::ImageAttachment>,
    /// Git repo context used by footer/status rendering and live branch tracking.
    pub(crate) git_context: GitContextState,
    /// Update availability state for the current app lifetime.
    pub update_notice: Option<UpdateNoticeState>,
    /// Session-wide usage and cost telemetry from the bridge.
    pub session_usage: SessionUsageState,
    /// Config > Usage snapshot and refresh lifecycle.
    pub usage: UsageState,
    /// Config > MCP live server snapshot and refresh lifecycle.
    pub mcp: McpState,
    /// Fast mode state telemetry from the SDK.
    pub fast_mode_state: model::FastModeState,
    /// Latest SDK runtime liveness state.
    pub runtime_session_state: Option<model::RuntimeSessionState>,
    /// Latest prompt suggestion from the SDK, shown in the input hint band.
    pub prompt_suggestion: Option<String>,
    /// Latest rate-limit telemetry from the SDK.
    pub last_rate_limit_update: Option<model::RateLimitUpdate>,
    /// Turn-local inline/system notices that may upgrade in place during the active turn.
    pub turn_notice_refs: Vec<TurnNoticeRef>,
    /// True while the SDK reports active compaction.
    pub is_compacting: bool,
    /// Account info from the bridge status snapshot (email, org, subscription).
    pub account_info: Option<crate::agent::types::AccountInfo>,

    /// Indexed terminal tool calls for per-frame terminal snapshot updates.
    /// Avoids O(n*m) scan of all messages/blocks every frame.
    pub terminal_tool_calls: Vec<TerminalToolCallRef>,
    /// Membership index for `terminal_tool_calls`, used to avoid linear duplicate checks.
    pub terminal_tool_call_membership: HashSet<TerminalToolCallRef>,
    /// Central notification manager (bell + desktop toast when unfocused).
    pub notifications: super::notify::NotificationManager,
    /// Performance logger. Present only when built with `--features perf`.
    /// Taken out (`Option::take`) during render, used, then put back to avoid
    /// borrow conflicts with `&mut App`.
    pub perf: Option<crate::perf::PerfLogger>,
    /// Global in-memory budget for rendered block and message caches.
    pub render_cache_budget: RenderCacheBudget,
    /// Cached render-cache slot metadata parallel to `messages[*].blocks[*]`
    /// plus one synthetic per-message slot at the tail of each row.
    pub(crate) render_cache_slots: Vec<Vec<render_budget::RenderCacheSlotState>>,
    /// Rolling total of cached render bytes across blocks and message-level caches.
    pub(crate) render_cache_total_bytes: usize,
    /// Rolling total of cached render bytes currently excluded from the budget.
    pub(crate) render_cache_protected_bytes: usize,
    /// Evictable cached blocks ordered by LRU and size tie-breaker.
    pub(crate) render_cache_evictable: BTreeSet<render_budget::RenderCacheEvictionKey>,
    /// Last message index currently protected as the streaming tail, if any.
    pub(crate) render_cache_tail_msg_idx: Option<usize>,
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
    /// Last emitted chat render trace snapshot to suppress identical per-frame summaries.
    pub last_chat_render_trace_state: Option<ChatRenderTraceState>,
    pub startup_connection_requested: bool,
    pub connection_started: bool,
    pub startup_bridge_script: Option<PathBuf>,
    pub startup_resume_id: Option<String>,
    pub startup_resume_requested: bool,
    pub startup_session_picker_requested: bool,
    pub startup_recent_sessions_loaded: bool,
    pub startup_session_picker_resolved: bool,
}

impl App {
    /// Queue a paste payload for drain-cycle finalization.
    ///
    /// This is fed by paste payloads captured from terminal events.
    pub fn queue_paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let chunk_chars = text.chars().count();
        let had_pending_submit = self.pending_submit.is_some();
        self.pending_submit = None;
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
            if let Some(session) = self.pending_paste_session {
                tracing::debug!(
                    target: crate::logging::targets::APP_PASTE,
                    event_name = "paste_queue_opened",
                    message = "paste queue session opened",
                    outcome = "start",
                    session_id = session.id,
                    start_row = session.start.row,
                    start_col = session.start.col,
                    placeholder_index = ?session.placeholder_index,
                    chunk_chars,
                    had_pending_submit,
                );
            }
        }
        self.pending_paste_text.push_str(text);
        tracing::debug!(
            target: crate::logging::targets::APP_PASTE,
            event_name = "paste_queue_updated",
            message = "paste queue updated",
            outcome = "success",
            chunk_chars,
            pending_chars = self.pending_paste_text.chars().count(),
            had_pending_submit,
        );
    }

    pub(crate) fn request_chat_repaint(&mut self) {
        self.surface_dirty.chat.request_repaint();
    }

    pub(crate) fn request_chat_mutable_rebuild(&mut self) {
        self.surface_dirty.chat.request_mutable_rebuild();
    }

    pub(crate) fn request_chat_visible_rebuild(&mut self) {
        self.surface_dirty.chat.request_visible_screen_rebuild();
    }

    pub(crate) fn request_chat_fullscreen_return_rebuild(&mut self) {
        self.surface_dirty.chat.request_fullscreen_return_rebuild();
    }

    pub(crate) fn request_chat_resize_purge_replay_rebuild(&mut self) {
        self.surface_dirty.chat.request_resize_purge_replay_rebuild();
    }

    pub(crate) fn request_chat_session_boundary_rebuild(&mut self) {
        self.surface_dirty.chat.request_session_boundary_rebuild();
    }

    pub(crate) fn request_fullscreen_repaint(&mut self) {
        self.surface_dirty.fullscreen.redraw = true;
    }

    pub(crate) fn request_active_surface_repaint(&mut self) {
        match self.terminal_lifecycle {
            TerminalLifecycleState::Running(SurfaceMode::Fullscreen(_)) => {
                self.request_fullscreen_repaint();
            }
            TerminalLifecycleState::Running(SurfaceMode::Chat)
            | TerminalLifecycleState::Bootstrapping => {
                self.request_chat_repaint();
            }
            TerminalLifecycleState::ReleasedToChild(_)
            | TerminalLifecycleState::Restoring
            | TerminalLifecycleState::Exited => {}
        }
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
    pub fn is_project_trusted(&self) -> bool {
        self.trust.is_trusted()
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
        self.insert_message_tracked(0, self.build_welcome_message());
    }

    #[must_use]
    fn welcome_subscription_display(&self) -> &str {
        self.account_info
            .as_ref()
            .and_then(|account| account.subscription_type.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("-")
    }

    #[must_use]
    fn welcome_cwd_display(&self) -> &str {
        let cwd = self.cwd.trim();
        if cwd.is_empty() { "-" } else { cwd }
    }

    #[must_use]
    fn welcome_session_id_display(&self) -> String {
        self.session_id
            .as_ref()
            .map(std::string::ToString::to_string)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "-".to_owned())
    }

    #[must_use]
    pub(crate) fn build_welcome_message(&self) -> ChatMessage {
        let subscription = self.welcome_subscription_display();
        let session_id = self.welcome_session_id_display();
        ChatMessage::welcome(
            env!("CARGO_PKG_VERSION"),
            subscription,
            self.welcome_cwd_display(),
            &session_id,
        )
    }

    #[must_use]
    pub(crate) fn current_welcome_tip_seed(&self) -> Option<u64> {
        let first = self.messages.first()?;
        let MessageBlock::Welcome(welcome) = first.blocks.first()? else {
            return None;
        };
        Some(welcome.tip_seed)
    }

    pub(crate) fn apply_welcome_tip_seed(message: &mut ChatMessage, tip_seed: u64) {
        let Some(MessageBlock::Welcome(welcome)) = message.blocks.first_mut() else {
            return;
        };
        welcome.tip_seed = tip_seed;
    }

    /// Update the welcome message with the latest session/account snapshot.
    pub fn sync_welcome_snapshot(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        let subscription = self.welcome_subscription_display().to_owned();
        let cwd = self.welcome_cwd_display().to_owned();
        let session_id = self.welcome_session_id_display();
        let Some(first) = self.messages.first() else {
            return;
        };
        if !matches!(first.role, MessageRole::Welcome) {
            return;
        }
        let mut changed = false;
        {
            let Some(first) = self.messages.first_mut() else {
                return;
            };
            let Some(MessageBlock::Welcome(welcome)) = first.blocks.first_mut() else {
                return;
            };
            if welcome.version != version
                || welcome.subscription != subscription
                || welcome.cwd != cwd
                || welcome.session_id != session_id
            {
                version.clone_into(&mut welcome.version);
                welcome.subscription = subscription;
                welcome.cwd = cwd;
                welcome.session_id = session_id;
                welcome.cache.invalidate();
                changed = true;
            }
        }
        if changed {
            self.recompute_message_retained_bytes(0);
            self.invalidate_layout(InvalidationLevel::MessagesFrom(0));
        }
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
        self.tool_call_scopes.get(id).cloned()
    }

    #[must_use]
    pub(crate) fn tracked_terminal_id_for_tool(tc: &ToolCallInfo) -> Option<String> {
        (tc.is_execute_tool()
            && matches!(
                tc.status,
                model::ToolCallStatus::Pending | model::ToolCallStatus::InProgress
            ))
        .then(|| tc.terminal_id.clone())
        .flatten()
    }

    pub fn clear_tool_scope_tracking(&mut self) {
        self.tool_call_scopes.clear();
        self.active_task_ids.clear();
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

    pub(crate) fn sync_terminal_tool_call(
        &mut self,
        terminal_id: String,
        msg_idx: usize,
        block_idx: usize,
    ) {
        let desired = TerminalToolCallRef::new(terminal_id, msg_idx, block_idx);
        if self.terminal_tool_call_membership.contains(&desired) {
            return;
        }
        self.untrack_terminal_tool_call(msg_idx, block_idx);
        self.terminal_tool_call_membership.insert(desired.clone());
        self.terminal_tool_calls.push(desired);
    }

    pub(crate) fn untrack_terminal_tool_call(&mut self, msg_idx: usize, block_idx: usize) {
        let removed: Vec<_> = self
            .terminal_tool_calls
            .iter()
            .filter(|entry| entry.msg_idx == msg_idx && entry.block_idx == block_idx)
            .cloned()
            .collect();
        if removed.is_empty() {
            return;
        }
        self.terminal_tool_calls
            .retain(|entry| entry.msg_idx != msg_idx || entry.block_idx != block_idx);
        for entry in removed {
            self.terminal_tool_call_membership.remove(&entry);
        }
    }

    pub(crate) fn clear_terminal_tool_call_tracking(&mut self) {
        self.terminal_tool_calls.clear();
        self.terminal_tool_call_membership.clear();
    }

    pub(crate) fn sync_after_message_blocks_changed(&mut self, msg_idx: usize) {
        self.note_render_cache_structure_changed();
        self.sync_render_cache_message(msg_idx);
        self.recompute_message_retained_bytes(msg_idx);
        self.invalidate_layout(InvalidationLevel::MessageChanged(msg_idx));
    }

    pub fn invalidate_layout(&mut self, _level: LayoutInvalidation) {
        self.chat_render.clear_measurements();
        self.chat_render.invalidate_live_anchor();
        self.request_chat_repaint();
    }

    pub(crate) fn invalidate_message_set<I>(&mut self, indices: I)
    where
        I: IntoIterator<Item = usize>,
    {
        let unique: BTreeSet<_> =
            indices.into_iter().filter(|&idx| idx < self.messages.len()).collect();
        if !unique.is_empty() {
            self.invalidate_layout(LayoutInvalidation::Global);
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
                &self.history_retention_stats,
                self.history_retention,
                &self.cache_metrics,
                stats.dropped_messages,
            );
            cache_metrics::emit_history_metrics(&snap);
        }
    }

    /// Force-finish any lingering in-progress tool calls.
    /// Returns the number of tool calls that were transitioned.
    pub fn finalize_in_progress_tool_calls(&mut self, new_status: model::ToolCallStatus) -> usize {
        let mut changed = 0usize;
        let mut cleared_interaction = false;
        let mut changed_message_indices = Vec::new();
        let mut changed_slots = Vec::new();
        let mut detached_terminal = false;

        for (msg_idx, msg) in self.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                if let MessageBlock::ToolCall(tc) = block {
                    let tc = tc.as_mut();
                    if matches!(
                        tc.status,
                        model::ToolCallStatus::InProgress | model::ToolCallStatus::Pending
                    ) {
                        tc.status = new_status;
                        tc.invalidate_render_cache();
                        changed_slots.push((msg_idx, block_idx));
                        if tc.pending_permission.take().is_some() {
                            cleared_interaction = true;
                        }
                        if tc.pending_question.take().is_some() {
                            cleared_interaction = true;
                        }
                        if tc.is_execute_tool() && tc.terminal_id.take().is_some() {
                            detached_terminal = true;
                        }
                        if changed_message_indices.last().copied() != Some(msg_idx) {
                            changed_message_indices.push(msg_idx);
                        }
                        changed += 1;
                    }
                }
            }
        }

        if detached_terminal {
            self.rebuild_tool_indices_and_terminal_refs();
        }

        for (msg_idx, block_idx) in changed_slots {
            self.sync_render_cache_slot(msg_idx, block_idx);
        }

        for msg_idx in changed_message_indices.iter().copied() {
            self.recompute_message_retained_bytes(msg_idx);
        }

        if changed > 0 || cleared_interaction {
            self.invalidate_message_set(changed_message_indices.iter().copied());
            self.pending_interaction_ids.clear();
            self.release_focus_target(FocusTarget::Permission);
        }

        changed
    }

    /// Clear any inline permission/question UI still attached to tool calls.
    /// Returns the number of tool call blocks that changed.
    pub fn clear_inline_tool_interactions(&mut self) -> usize {
        let mut changed = 0usize;
        let mut changed_message_indices = Vec::new();
        let mut changed_slots = Vec::new();

        for (msg_idx, msg) in self.messages.iter_mut().enumerate() {
            for (block_idx, block) in msg.blocks.iter_mut().enumerate() {
                let MessageBlock::ToolCall(tc) = block else {
                    continue;
                };
                let tc = tc.as_mut();
                let mut block_changed = false;
                if tc.pending_permission.take().is_some() {
                    block_changed = true;
                }
                if tc.pending_question.take().is_some() {
                    block_changed = true;
                }
                if !block_changed {
                    continue;
                }
                tc.invalidate_render_cache();
                changed_slots.push((msg_idx, block_idx));
                if changed_message_indices.last().copied() != Some(msg_idx) {
                    changed_message_indices.push(msg_idx);
                }
                changed += 1;
            }
        }

        for (msg_idx, block_idx) in changed_slots {
            self.sync_render_cache_slot(msg_idx, block_idx);
        }

        for msg_idx in changed_message_indices.iter().copied() {
            self.recompute_message_retained_bytes(msg_idx);
        }

        if changed > 0 {
            self.invalidate_message_set(changed_message_indices.iter().copied());
        }

        if changed > 0 || !self.pending_interaction_ids.is_empty() {
            self.pending_interaction_ids.clear();
            self.release_focus_target(FocusTarget::Permission);
        }

        changed
    }

    /// Clear runtime-only turn tracking while preserving the message history itself.
    pub fn finalize_turn_runtime_artifacts(&mut self, new_status: model::ToolCallStatus) {
        let _ = self.finalize_in_progress_tool_calls(new_status);
        let _ = self.clear_inline_tool_interactions();
        self.clear_tool_scope_tracking();
    }

    /// Build a minimal `App` for unit/integration tests.
    /// All fields get sensible defaults; the `mpsc` channel is wired up internally.
    #[doc(hidden)]
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn test_default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (file_index_tx, file_index_rx) = std_mpsc::channel();
        Self {
            surface_mode: SurfaceMode::Chat,
            terminal_lifecycle: TerminalLifecycleState::Running(SurfaceMode::Chat),
            surface_dirty: SurfaceDirtyState::initial_chat(),
            config: ConfigState::default(),
            trust: TrustState::default(),
            settings_home_override: None,
            messages: Vec::new(),
            message_retained_bytes: Vec::new(),
            retained_history_bytes: 0,
            input: InputState::new(),
            status: AppStatus::Ready,
            resuming_session_id: None,
            show_session_overview: true,
            pending_command_label: None,
            pending_command_ack: None,
            should_quit: false,
            exit_error: None,
            session_id: None,
            conn: None,
            session_scope_epoch: 0,
            current_model: Some(
                model::CurrentModel::new("test-model", "test-model", "test-model")
                    .authoritative(true),
            ),
            cwd: "/test".into(),
            cwd_raw: "/test".into(),
            files_accessed: 0,
            mode: None,
            config_options: BTreeMap::new(),
            login_hint: None,
            pending_compact_clear: false,
            pending_interaction_ids: Vec::new(),
            cancelled_turn_pending_hint: false,
            pending_cancel_origin: None,
            pending_auto_submit_after_cancel: false,
            event_tx: tx,
            event_rx: rx,
            file_index_event_tx: file_index_tx,
            file_index_event_rx: file_index_rx,
            spinner_frame: 0,
            spinner_last_advance_at: None,
            active_turn_assistant_message_idx: None,
            active_task_ids: HashSet::default(),
            tool_call_scopes: HashMap::default(),
            terminals: std::rc::Rc::default(),
            tool_call_index: HashMap::default(),
            todos: Vec::new(),
            focus: FocusManager::default(),
            available_commands: Vec::new(),
            plugins: PluginsState::default(),
            available_agents: Vec::new(),
            available_models: Vec::new(),
            recent_sessions: Vec::new(),
            session_picker: SessionPickerState::default(),
            chat_render: ChatRenderState::default(),
            mention: None,
            file_index: file_index::FileIndexState::default(),
            slash: None,
            subagent: None,
            pending_submit: None,
            paste_burst: super::paste_burst::PasteBurstDetector::new(),
            pending_paste_text: String::new(),
            pending_paste_session: None,
            active_paste_session: None,
            next_paste_session_id: 1,
            pending_images: Vec::new(),
            git_context: GitContextState::default(),
            update_notice: None,
            session_usage: SessionUsageState::default(),
            usage: UsageState::default(),
            mcp: McpState::default(),
            fast_mode_state: model::FastModeState::Off,
            runtime_session_state: None,
            prompt_suggestion: None,
            last_rate_limit_update: None,
            turn_notice_refs: Vec::new(),
            is_compacting: false,
            account_info: None,
            terminal_tool_calls: Vec::new(),
            terminal_tool_call_membership: HashSet::new(),
            notifications: super::notify::NotificationManager::new(),
            perf: None,
            render_cache_budget: RenderCacheBudget::default(),
            render_cache_slots: Vec::new(),
            render_cache_total_bytes: 0,
            render_cache_protected_bytes: 0,
            render_cache_evictable: BTreeSet::new(),
            render_cache_tail_msg_idx: None,
            history_retention: HistoryRetentionPolicy::default(),
            history_retention_stats: HistoryRetentionStats::default(),
            cache_metrics: CacheMetrics::default(),
            fps_ema: None,
            last_frame_at: None,
            last_chat_render_trace_state: None,
            startup_connection_requested: false,
            connection_started: false,
            startup_bridge_script: None,
            startup_resume_id: None,
            startup_resume_requested: false,
            startup_session_picker_requested: false,
            startup_recent_sessions_loaded: false,
            startup_session_picker_resolved: false,
        }
    }

    #[must_use]
    pub fn git_branch(&self) -> Option<&str> {
        self.git_context.branch_name()
    }

    pub fn sync_git_context(&mut self) {
        if self.git_context.sync_to_cwd(Path::new(&self.cwd_raw)) {
            self.request_chat_repaint();
        }
    }

    pub fn tick_git_context(&mut self, now: Instant) {
        if self.git_context.tick(Path::new(&self.cwd_raw), now) {
            self.request_chat_repaint();
        }
    }

    #[cfg(test)]
    pub fn set_git_branch_for_test(&mut self, branch: Option<&str>) {
        self.git_context.set_branch_for_test(branch);
    }

    /// Resolve the effective focus owner for Up/Down and other directional keys.
    #[must_use]
    pub fn focus_owner(&self) -> FocusOwner {
        self.focus.owner(self.focus_context())
    }

    #[must_use]
    pub fn active_turn_assistant_idx(&self) -> Option<usize> {
        self.active_turn_assistant_message_idx.filter(|&idx| {
            self.messages.get(idx).is_some_and(|msg| matches!(msg.role, MessageRole::Assistant))
        })
    }

    pub fn bind_active_turn_assistant(&mut self, idx: usize) {
        self.active_turn_assistant_message_idx = self
            .messages
            .get(idx)
            .is_some_and(|msg| matches!(msg.role, MessageRole::Assistant))
            .then_some(idx);
    }

    pub fn bind_active_turn_assistant_to_tail(&mut self) {
        if let Some(idx) = self.messages.len().checked_sub(1) {
            self.bind_active_turn_assistant(idx);
        } else {
            self.clear_active_turn_assistant();
        }
    }

    pub fn clear_active_turn_assistant(&mut self) {
        self.active_turn_assistant_message_idx = None;
    }

    pub(crate) fn clear_turn_notice_refs(&mut self) {
        self.turn_notice_refs.clear();
    }

    pub(crate) fn shift_turn_notice_refs_for_insert(&mut self, idx: usize) {
        for notice_ref in &mut self.turn_notice_refs {
            match &mut notice_ref.location {
                TurnNoticeLocation::Inline { msg_idx, .. }
                | TurnNoticeLocation::Standalone { msg_idx }
                    if idx <= *msg_idx =>
                {
                    *msg_idx = msg_idx.saturating_add(1);
                }
                TurnNoticeLocation::Inline { .. } | TurnNoticeLocation::Standalone { .. } => {}
            }
        }
    }

    pub(crate) fn shift_turn_notice_refs_for_remove(&mut self, idx: usize) {
        self.turn_notice_refs.retain_mut(|notice_ref| match &mut notice_ref.location {
            TurnNoticeLocation::Inline { msg_idx, .. }
            | TurnNoticeLocation::Standalone { msg_idx } => match idx.cmp(msg_idx) {
                std::cmp::Ordering::Less => {
                    *msg_idx = msg_idx.saturating_sub(1);
                    true
                }
                std::cmp::Ordering::Equal => false,
                std::cmp::Ordering::Greater => true,
            },
        });
    }

    pub(crate) fn remap_turn_notice_refs_after_message_drop(
        &mut self,
        old_to_new: &[Option<usize>],
    ) {
        self.turn_notice_refs.retain_mut(|notice_ref| match &mut notice_ref.location {
            TurnNoticeLocation::Inline { msg_idx, .. }
            | TurnNoticeLocation::Standalone { msg_idx } => {
                let Some(new_idx) = old_to_new.get(*msg_idx).copied().flatten() else {
                    return false;
                };
                *msg_idx = new_idx;
                true
            }
        });
    }

    pub fn bump_session_scope_epoch(&mut self) {
        self.session_scope_epoch = self.session_scope_epoch.saturating_add(1);
    }

    pub fn clear_session_runtime_identity(&mut self) {
        self.session_id = None;
        self.current_model = None;
        self.mode = None;
        self.fast_mode_state = model::FastModeState::Off;
        self.session_usage = SessionUsageState::default();
    }

    pub fn reconcile_trust_state_from_preferences_and_cwd(&mut self) {
        let lookup = crate::app::trust::store::read_status(
            &self.config.committed_preferences_document,
            Path::new(&self.cwd_raw),
        );
        self.trust.project_key = lookup.project_key;
        self.trust.status = if lookup.trusted {
            crate::app::trust::TrustStatus::Trusted
        } else {
            crate::app::trust::TrustStatus::Untrusted
        };
        self.trust.selection = crate::app::trust::TrustSelection::Yes;
        self.trust.last_error = self
            .config
            .preferences_path
            .is_none()
            .then(|| "Trust preferences path is not available".to_owned());
    }

    pub fn reconcile_runtime_from_persisted_settings_change(&mut self) {
        self.reconcile_trust_state_from_preferences_and_cwd();
    }

    pub(crate) fn shift_active_turn_assistant_for_insert(&mut self, idx: usize) {
        if let Some(owner_idx) = self.active_turn_assistant_message_idx
            && idx <= owner_idx
        {
            self.active_turn_assistant_message_idx = Some(owner_idx.saturating_add(1));
        }
    }

    pub(crate) fn shift_active_turn_assistant_for_remove(&mut self, idx: usize) {
        let Some(owner_idx) = self.active_turn_assistant_message_idx else {
            return;
        };
        self.active_turn_assistant_message_idx = match idx.cmp(&owner_idx) {
            std::cmp::Ordering::Less => Some(owner_idx.saturating_sub(1)),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(owner_idx),
        };
    }

    #[must_use]
    pub fn active_autocomplete_kind(&self) -> Option<AutocompleteKind> {
        if self.mention.is_some() {
            Some(AutocompleteKind::Mention)
        } else if self.slash.is_some() {
            Some(AutocompleteKind::Slash)
        } else if self.subagent.is_some() {
            Some(AutocompleteKind::Subagent)
        } else {
            None
        }
    }

    #[must_use]
    pub fn autocomplete_focus_available(&self) -> bool {
        self.mention.as_ref().is_some_and(mention::MentionState::has_selectable_candidates)
            || self.slash.is_some()
            || self.subagent.is_some()
    }

    #[must_use]
    pub fn has_draft_input_for_focus(&self) -> bool {
        !self.input.is_empty()
    }

    pub fn rebuild_chat_focus_from_state(&mut self) {
        if self.surface_mode != SurfaceMode::Chat {
            return;
        }

        self.normalize_focus_stack();

        if self.pending_interaction_ids.is_empty() {
            clear_inline_interaction_focus(self);
        } else if self.focus_owner() == FocusOwner::Permission || !self.has_draft_input_for_focus()
        {
            focus_next_inline_interaction(self);
        } else {
            clear_inline_interaction_focus(self);
        }

        if self.autocomplete_focus_available() {
            self.claim_focus_target(FocusTarget::Mention);
        } else {
            self.release_focus_target(FocusTarget::Mention);
        }

        self.normalize_focus_stack();
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
            self.autocomplete_focus_available(),
            !self.pending_interaction_ids.is_empty(),
        )
    }
}

#[cfg(test)]
mod tests {
    // =====
    // TESTS: 26
    // =====

    use super::*;
    use crate::app::slash::{SlashCandidate, SlashContext, SlashState};
    use pretty_assertions::assert_eq;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    // BlockCache

    #[test]
    fn cache_lifecycle_covers_default_store_invalidate_and_restore() {
        let mut cache = BlockCache::default();
        assert!(cache.get().is_none());

        cache.store(vec![Line::from("old")]);
        assert_eq!(cache.get().unwrap().len(), 1);

        cache.invalidate();
        cache.invalidate();
        cache.invalidate();
        assert!(cache.get().is_none());

        cache.store(vec![Line::from("new")]);
        let lines = cache.get().unwrap();
        assert_eq!(lines.len(), 1);
        let span_content: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(span_content, "new");
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
    fn cache_set_height_then_height_at() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello")]);
        cache.set_height(1, 80);
        assert_eq!(cache.height_at(80), Some(1));
        assert!(cache.get().is_some());
    }

    #[test]
    fn cache_height_at_wrong_width_returns_none() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello")]);
        cache.set_height(1, 80);
        assert!(cache.height_at(120).is_none());
    }

    #[test]
    fn cache_height_invalidated_returns_none() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello")]);
        cache.set_height(1, 80);
        cache.invalidate();
        assert!(cache.height_at(80).is_none());
    }

    #[test]
    fn clear_session_runtime_identity_resets_session_usage() {
        let mut app = App::test_default();
        app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
        app.current_model = Some(
            crate::agent::model::CurrentModel::new("sonnet", "Claude Sonnet", "Claude Sonnet")
                .authoritative(true),
        );
        app.mode = Some(crate::app::ModeState {
            current_mode_id: "plan".to_owned(),
            current_mode_name: "Plan".to_owned(),
            available_modes: Vec::new(),
        });
        app.session_usage.context_usage_percent = Some(62);
        app.session_usage.context_usage_in_flight = true;
        app.session_usage.context_usage_refresh_pending = true;
        app.session_usage.context_usage_last_requested_at = Some(Instant::now());
        app.session_usage.last_compaction_pre_tokens = Some(123_456);

        app.clear_session_runtime_identity();

        assert!(app.session_id.is_none());
        assert!(app.current_model.is_none());
        assert!(app.mode.is_none());
        assert_eq!(app.session_usage, SessionUsageState::default());
    }

    #[test]
    fn test_default_initializes_chat_render_state() {
        let app = App::test_default();

        assert_eq!(app.chat_render, ChatRenderState::default());
    }

    #[test]
    fn cache_store_without_height_has_no_height() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("hello")]);
        // store() without height leaves wrapped_width at 0
        assert!(cache.height_at(80).is_none());
    }

    #[test]
    fn cache_store_and_set_height_overwrite() {
        let mut cache = BlockCache::default();
        cache.store(vec![Line::from("old")]);
        cache.set_height(1, 80);
        cache.invalidate();
        cache.store(vec![Line::from("new long line")]);
        cache.set_height(3, 120);
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
        MessageBlock::Text(TextBlock::from_complete(text))
    }

    fn user_text_message(text: &str) -> ChatMessage {
        ChatMessage::new(MessageRole::User, vec![assistant_text_block(text)], None)
    }

    fn system_text_message(text: &str) -> ChatMessage {
        ChatMessage::new(
            MessageRole::System(Some(SystemSeverity::Info)),
            vec![assistant_text_block(text)],
            None,
        )
    }

    fn user_text_image_message(text: &str, image_count: usize) -> ChatMessage {
        ChatMessage::new(
            MessageRole::User,
            vec![
                assistant_text_block(text),
                MessageBlock::ImageAttachment(ImageAttachmentBlock::new(image_count)),
            ],
            None,
        )
    }

    fn set_account_subscription(app: &mut App, subscription: &str) {
        app.account_info = Some(crate::agent::types::AccountInfo {
            subscription_type: Some(subscription.to_owned()),
            ..Default::default()
        });
    }

    #[test]
    fn push_message_tracked_appends_user_message_and_requests_repaint() {
        let mut app = make_test_app();
        let _ = app.surface_dirty.chat.take_repaint();

        app.push_message_tracked(user_text_message("hello"));

        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, MessageRole::User));
        let MessageBlock::Text(text) = &app.messages[0].blocks[0] else {
            panic!("expected text block");
        };
        assert_eq!(text.text, "hello");
        assert!(app.surface_dirty.chat.repaint);
    }

    #[test]
    fn push_message_tracked_preserves_message_order() {
        let mut app = make_test_app();

        app.push_message_tracked(user_text_message("first"));
        app.push_message_tracked(system_text_message("second"));

        assert_eq!(app.messages.len(), 2);
        assert!(matches!(app.messages[0].role, MessageRole::User));
        assert!(matches!(app.messages[1].role, MessageRole::System(Some(SystemSeverity::Info))));
    }

    #[test]
    fn sync_welcome_snapshot_updates_canonical_welcome_message() {
        let mut app = make_test_app();
        app.ensure_welcome_message();

        app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
        set_account_subscription(&mut app, "Pro");

        app.sync_welcome_snapshot();
        app.sync_welcome_snapshot();

        let MessageBlock::Welcome(welcome) = &app.messages[0].blocks[0] else {
            panic!("expected welcome block");
        };
        assert_eq!(welcome.subscription, "Pro");
        assert_eq!(welcome.session_id, "session-1");
    }

    #[test]
    fn sync_welcome_snapshot_updates_existing_canonical_welcome_in_place() {
        let mut app = make_test_app();
        app.ensure_welcome_message();
        app.session_id = Some(crate::agent::model::SessionId::new("session-1"));
        set_account_subscription(&mut app, "Pro");
        app.sync_welcome_snapshot();

        set_account_subscription(&mut app, "Claude Max");
        app.sync_welcome_snapshot();

        let MessageBlock::Welcome(welcome) = &app.messages[0].blocks[0] else {
            panic!("expected welcome block");
        };
        assert_eq!(app.messages.len(), 1);
        assert_eq!(welcome.subscription, "Claude Max");
    }

    #[test]
    fn push_message_tracked_preserves_user_image_attachment_block() {
        let mut app = make_test_app();

        app.push_message_tracked(user_text_image_message("see attached", 2));

        assert_eq!(app.messages.len(), 1);
        assert!(matches!(app.messages[0].role, MessageRole::User));
        let MessageBlock::Text(text) = &app.messages[0].blocks[0] else {
            panic!("expected text block");
        };
        assert_eq!(text.text, "see attached");
        let MessageBlock::ImageAttachment(image) = &app.messages[0].blocks[1] else {
            panic!("expected image attachment block");
        };
        assert_eq!(image.count, 2);
    }

    fn assistant_tool_message(id: &str, status: model::ToolCallStatus) -> ChatMessage {
        ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
                id: id.to_owned(),
                title: format!("tool {id}"),
                sdk_tool_name: "Read".to_owned(),
                raw_input: None,
                raw_input_bytes: 0,
                output_metadata: None,
                task_metadata: None,
                status,
                content: Vec::new(),
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: Some("x".repeat(1024)),
                terminal_output_len: 1024,
                terminal_bytes_seen: 1024,
                terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
                cache: BlockCache::default(),
                pending_permission: None,
                pending_question: None,
            }))],
            None,
        )
    }

    fn assistant_bash_tool_message(
        id: &str,
        status: model::ToolCallStatus,
        terminal_id: &str,
    ) -> ChatMessage {
        ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
                id: id.to_owned(),
                title: format!("tool {id}"),
                sdk_tool_name: "Bash".to_owned(),
                raw_input: None,
                raw_input_bytes: 0,
                output_metadata: None,
                task_metadata: None,
                status,
                content: Vec::new(),
                hidden: false,
                terminal_id: Some(terminal_id.to_owned()),
                terminal_command: Some("echo hi".to_owned()),
                terminal_output: Some("x".repeat(1024)),
                terminal_output_len: 1024,
                terminal_bytes_seen: 1024,
                terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
                cache: BlockCache::default(),
                pending_permission: None,
                pending_question: None,
            }))],
            None,
        )
    }

    fn assistant_tool_message_with_pending_permission(id: &str) -> ChatMessage {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
                id: id.to_owned(),
                title: format!("tool {id}"),
                sdk_tool_name: "Read".to_owned(),
                raw_input: None,
                raw_input_bytes: 0,
                output_metadata: None,
                task_metadata: None,
                status: model::ToolCallStatus::Completed,
                content: Vec::new(),
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: Some("x".repeat(1024)),
                terminal_output_len: 1024,
                terminal_bytes_seen: 1024,
                terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
                cache: BlockCache::default(),
                pending_permission: Some(InlinePermission {
                    options: vec![model::PermissionOption::new(
                        "allow-once",
                        "Allow once",
                        model::PermissionOptionKind::AllowOnce,
                    )],
                    display: None,
                    response_tx: tx,
                    selected_index: 0,
                    focused: false,
                }),
                pending_question: None,
            }))],
            None,
        )
    }

    #[test]
    fn enforce_render_cache_budget_evicts_lru_block() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("a")], None),
            ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("b")], None),
        ];

        let bytes_a = if let MessageBlock::Text(block) = &mut app.messages[0].blocks[0] {
            block.cache.store(vec![Line::from("x".repeat(2200))]);
            block.cache.cached_bytes()
        } else {
            0
        };
        let bytes_b = if let MessageBlock::Text(block) = &mut app.messages[1].blocks[0] {
            block.cache.store(vec![Line::from("y".repeat(2200))]);
            let _ = block.cache.get();
            block.cache.cached_bytes()
        } else {
            0
        };

        app.render_cache_budget.max_bytes = bytes_b;
        let stats = app.enforce_render_cache_budget();
        assert!(stats.evicted_blocks >= 1);
        assert!(stats.evicted_bytes >= bytes_a);
        assert!(stats.total_after_bytes <= app.render_cache_budget.max_bytes);
        assert_eq!(stats.protected_bytes, 0);

        if let MessageBlock::Text(block) = &app.messages[0].blocks[0] {
            assert_eq!(block.cache.cached_bytes(), 0);
        } else {
            panic!("expected text block");
        }
        if let MessageBlock::Text(block) = &app.messages[1].blocks[0] {
            assert_eq!(block.cache.cached_bytes(), bytes_b);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_protects_streaming_tail_message() {
        let mut app = make_test_app();
        app.status = AppStatus::Thinking;
        app.messages = vec![ChatMessage::new(
            MessageRole::Assistant,
            vec![assistant_text_block("streaming tail")],
            None,
        )];

        let before = if let MessageBlock::Text(block) = &mut app.messages[0].blocks[0] {
            block.cache.store(vec![Line::from("z".repeat(4096))]);
            block.cache.cached_bytes()
        } else {
            0
        };
        app.render_cache_budget.max_bytes = 64;
        let stats = app.enforce_render_cache_budget();
        assert_eq!(stats.evicted_blocks, 0);
        assert_eq!(stats.evicted_bytes, 0);
        assert_eq!(stats.protected_bytes, before);

        if let MessageBlock::Text(block) = &app.messages[0].blocks[0] {
            assert_eq!(block.cache.cached_bytes(), before);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_excludes_protected_from_budget() {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.messages = vec![
            ChatMessage::new(
                MessageRole::Assistant,
                vec![assistant_text_block("old message")],
                None,
            ),
            ChatMessage::new(
                MessageRole::Assistant,
                vec![assistant_text_block("streaming tail")],
                None,
            ),
        ];

        let bytes_a = if let MessageBlock::Text(block) = &mut app.messages[0].blocks[0] {
            block.cache.store(vec![Line::from("x".repeat(2200))]);
            block.cache.cached_bytes()
        } else {
            0
        };
        let bytes_b = if let MessageBlock::Text(block) = &mut app.messages[1].blocks[0] {
            block.cache.store(vec![Line::from("y".repeat(5000))]);
            block.cache.cached_bytes()
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
        if let MessageBlock::Text(block) = &app.messages[0].blocks[0] {
            assert_eq!(block.cache.cached_bytes(), bytes_a);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_protects_active_streaming_owner_not_physical_tail() {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.messages = vec![
            ChatMessage::new(
                MessageRole::Assistant,
                vec![assistant_text_block("old message")],
                None,
            ),
            ChatMessage::new(
                MessageRole::Assistant,
                vec![assistant_text_block("active streaming owner")],
                None,
            ),
            ChatMessage::new(
                MessageRole::System(Some(SystemSeverity::Info)),
                vec![assistant_text_block("late trailing system row")],
                None,
            ),
        ];
        app.bind_active_turn_assistant(1);

        if let MessageBlock::Text(block) = &mut app.messages[0].blocks[0] {
            block.cache.store(vec![Line::from("x".repeat(2000))]);
        }
        let protected_bytes = if let MessageBlock::Text(block) = &mut app.messages[1].blocks[0] {
            block.cache.store(vec![Line::from("y".repeat(4000))]);
            block.cache.cached_bytes()
        } else {
            0
        };
        if let MessageBlock::Text(block) = &mut app.messages[2].blocks[0] {
            block.cache.store(vec![Line::from("z".repeat(5000))]);
        }

        app.render_cache_budget.max_bytes = 64;
        let stats = app.enforce_render_cache_budget();

        assert_eq!(stats.protected_bytes, protected_bytes);
    }

    #[test]
    fn enforce_render_cache_budget_evicts_when_budgeted_over_limit() {
        let mut app = make_test_app();
        app.status = AppStatus::Running;
        app.messages = vec![
            ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("old-a")], None),
            ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("old-b")], None),
            ChatMessage::new(MessageRole::Assistant, vec![assistant_text_block("streaming")], None),
        ];

        // Populate caches: messages 0 and 1 evictable, message 2 protected.
        if let MessageBlock::Text(block) = &mut app.messages[0].blocks[0] {
            block.cache.store(vec![Line::from("x".repeat(3000))]);
        }
        let bytes_b = if let MessageBlock::Text(block) = &mut app.messages[1].blocks[0] {
            block.cache.store(vec![Line::from("y".repeat(3000))]);
            let _ = block.cache.get(); // touch to make more recently accessed
            block.cache.cached_bytes()
        } else {
            0
        };
        let bytes_c = if let MessageBlock::Text(block) = &mut app.messages[2].blocks[0] {
            block.cache.store(vec![Line::from("z".repeat(5000))]);
            block.cache.cached_bytes()
        } else {
            0
        };

        // Budget fits message B but not A+B (excludes C as protected).
        app.render_cache_budget.max_bytes = bytes_b + 100;

        let stats = app.enforce_render_cache_budget();

        assert_eq!(stats.protected_bytes, bytes_c);
        assert!(stats.evicted_blocks >= 1); // message A evicted (older access)
        // Message B should survive (more recent access).
        if let MessageBlock::Text(block) = &app.messages[1].blocks[0] {
            assert_eq!(block.cache.cached_bytes(), bytes_b);
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn enforce_render_cache_budget_protected_bytes_zero_when_not_streaming() {
        let mut app = make_test_app();
        app.status = AppStatus::Ready;
        app.messages = vec![ChatMessage::new(
            MessageRole::Assistant,
            vec![assistant_text_block("done")],
            None,
        )];

        if let MessageBlock::Text(block) = &mut app.messages[0].blocks[0] {
            block.cache.store(vec![Line::from("x".repeat(2000))]);
        }
        app.render_cache_budget.max_bytes = usize::MAX;

        let stats = app.enforce_render_cache_budget();
        assert_eq!(stats.protected_bytes, 0);
    }

    #[test]
    fn enforce_history_retention_noop_under_budget() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
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
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
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
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
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
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
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
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
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
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
            user_text_message("drop this"),
            assistant_bash_tool_message("tool-idx", model::ToolCallStatus::InProgress, "term-1"),
        ];
        app.index_tool_call("tool-idx".to_owned(), 99, 99);
        app.sync_terminal_tool_call("stale-term".to_owned(), 99, 99);
        app.history_retention.max_bytes = 1;

        let _ = app.enforce_history_retention();
        assert_eq!(app.lookup_tool_call("tool-idx"), Some((2, 0)));
        assert_eq!(app.terminal_tool_calls.len(), 1);
        assert_eq!(app.terminal_tool_call_membership.len(), 1);
        assert_eq!(app.terminal_tool_calls[0].terminal_id, "term-1");
        assert_eq!(app.terminal_tool_calls[0].msg_idx, 2);
        assert_eq!(app.terminal_tool_calls[0].block_idx, 0);
    }

    #[test]
    fn enforce_history_retention_preserves_active_turn_assistant_message() {
        let mut app = make_test_app();
        app.status = AppStatus::Thinking;
        app.messages = vec![
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
            user_text_message("drop this"),
            ChatMessage::new(MessageRole::Assistant, Vec::new(), None),
        ];
        app.bind_active_turn_assistant(2);
        app.history_retention.max_bytes = 1;

        let stats = app.enforce_history_retention();

        assert_eq!(stats.dropped_messages, 1);
        assert_eq!(app.active_turn_assistant_idx(), Some(2));
        assert!(matches!(app.messages[2].role, MessageRole::Assistant));
    }

    #[test]
    fn enforce_history_retention_remaps_active_turn_assistant_after_prune() {
        let mut app = make_test_app();
        app.status = AppStatus::Thinking;
        app.messages = vec![
            user_text_message("drop this"),
            ChatMessage::new(
                MessageRole::Assistant,
                vec![assistant_text_block("streaming reply")],
                None,
            ),
        ];
        app.bind_active_turn_assistant(1);
        app.history_retention.max_bytes = App::measure_message_bytes(&app.messages[1]);

        let stats = app.enforce_history_retention();

        assert_eq!(stats.dropped_messages, 1);
        assert_eq!(app.active_turn_assistant_idx(), Some(1));
        assert!(App::is_history_hidden_marker_message(&app.messages[0]));
        assert!(matches!(app.messages[1].role, MessageRole::Assistant));
    }

    #[test]
    fn enforce_history_retention_keeps_single_marker_on_repeat() {
        let mut app = make_test_app();
        app.messages = vec![
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-"),
            user_text_message("drop me"),
        ];
        app.history_retention.max_bytes = 1;

        let first = app.enforce_history_retention();
        let second = app.enforce_history_retention();
        let marker_count =
            app.messages.iter().filter(|msg| App::is_history_hidden_marker_message(msg)).count();

        assert_eq!(first.dropped_messages, 1);
        assert_eq!(second.dropped_messages, 0);
        assert_eq!(marker_count, 1);
    }

    #[allow(clippy::cast_precision_loss)]
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
    }

    #[test]
    fn finalize_in_progress_tool_calls_detaches_execute_terminal_refs() {
        let mut app = make_test_app();
        app.messages.push(assistant_bash_tool_message(
            "bash-1",
            model::ToolCallStatus::InProgress,
            "term-1",
        ));
        app.index_tool_call("bash-1".to_owned(), 0, 0);
        app.sync_terminal_tool_call("term-1".to_owned(), 0, 0);

        let changed = app.finalize_in_progress_tool_calls(model::ToolCallStatus::Completed);

        assert_eq!(changed, 1);
        assert!(app.terminal_tool_calls.is_empty());
        assert!(app.terminal_tool_call_membership.is_empty());
        let MessageBlock::ToolCall(tc) = &app.messages[0].blocks[0] else {
            panic!("expected tool call");
        };
        assert_eq!(tc.status, model::ToolCallStatus::Completed);
        assert_eq!(tc.terminal_id, None);
    }

    #[test]
    fn remove_message_tracked_tail_removes_orphaned_tool_indices() {
        let mut app = make_test_app();
        app.messages.push(user_text_message("before"));
        app.messages.push(assistant_tool_message("tool-1", model::ToolCallStatus::Completed));
        app.index_tool_call("tool-1".to_owned(), 1, 0);

        let removed = app.remove_message_tracked(1);

        assert!(removed.is_some());
        assert!(app.lookup_tool_call("tool-1").is_none());
    }

    #[test]
    fn remove_message_tracked_prunes_tool_scope_entries() {
        let mut app = make_test_app();
        app.messages.push(assistant_tool_message("tool-1", model::ToolCallStatus::Completed));
        app.index_tool_call("tool-1".to_owned(), 0, 0);
        app.register_tool_call_scope(
            "tool-1".to_owned(),
            ToolCallScope::SubagentChild { parent_tool_use_id: "task-1".to_owned() },
        );

        let removed = app.remove_message_tracked(0);

        assert!(removed.is_some());
        assert_eq!(app.tool_call_scope("tool-1"), None);
    }

    #[test]
    fn clear_messages_tracked_clears_tool_and_terminal_tracking() {
        let mut app = make_test_app();
        app.messages.push(assistant_bash_tool_message(
            "bash-1",
            model::ToolCallStatus::InProgress,
            "term-1",
        ));
        app.index_tool_call("bash-1".to_owned(), 0, 0);
        app.sync_terminal_tool_call("term-1".to_owned(), 0, 0);
        app.pending_interaction_ids.push("bash-1".into());

        app.clear_messages_tracked();

        assert!(app.messages.is_empty());
        assert!(app.tool_call_index.is_empty());
        assert!(app.terminal_tool_calls.is_empty());
        assert!(app.terminal_tool_call_membership.is_empty());
        assert!(app.pending_interaction_ids.is_empty());
    }

    #[test]
    fn rebuild_tool_indices_skips_completed_terminal_refs() {
        let mut app = make_test_app();
        app.messages.push(assistant_bash_tool_message(
            "bash-1",
            model::ToolCallStatus::Completed,
            "term-1",
        ));
        app.index_tool_call("bash-1".to_owned(), 0, 0);
        app.sync_terminal_tool_call("term-1".to_owned(), 0, 0);

        app.rebuild_tool_indices_and_terminal_refs();

        assert!(app.terminal_tool_calls.is_empty());
        assert!(app.terminal_tool_call_membership.is_empty());
    }

    // IncrementalMarkdown

    /// Simple render function for tests: wraps each line in a `Line`.
    fn test_render(src: &str) -> Vec<Line<'static>> {
        src.lines().map(|l| Line::from(l.to_owned())).collect()
    }

    fn test_render_key() -> super::messages::MarkdownRenderKey {
        super::messages::MarkdownRenderKey { width: 80, bg: None, preserve_newlines: false }
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
        let lines = incr.lines(test_render_key(), &test_render);
        // test_render maps each source line to one output line
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn incr_ensure_rendered_preserves_text() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("p1\n\np2\n\ntail");
        incr.ensure_rendered(test_render_key(), &test_render);
        assert_eq!(incr.full_text(), "p1\n\np2\n\ntail");
    }

    #[test]
    fn incr_invalidate_renders_preserves_text() {
        let mut incr = IncrementalMarkdown::default();
        incr.append("p1\n\np2\n\ntail");
        incr.invalidate_renders();
        assert_eq!(incr.full_text(), "p1\n\np2\n\ntail");
    }

    #[test]
    fn incr_reuses_rendered_prefix_chunks() {
        use std::cell::Cell;

        let calls = Cell::new(0usize);
        let render = |src: &str| -> Vec<Line<'static>> {
            calls.set(calls.get() + 1);
            test_render(src)
        };

        let mut incr = IncrementalMarkdown::default();
        incr.append("p1\n\np2");
        let _ = incr.lines(test_render_key(), &render);
        assert_eq!(calls.get(), 2);

        incr.append(" tail");
        let _ = incr.lines(test_render_key(), &render);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn incr_does_not_split_inside_fenced_code_blocks() {
        let calls = std::cell::Cell::new(0usize);
        let render = |src: &str| -> Vec<Line<'static>> {
            calls.set(calls.get() + 1);
            test_render(src)
        };

        let mut incr = IncrementalMarkdown::default();
        incr.append("```rust\nfn main() {\n\nprintln!(\"hi\");\n}\n```\n\nafter");
        let _ = incr.lines(test_render_key(), &render);

        assert_eq!(calls.get(), 2);
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

    fn focus_test_app_with_available_targets() -> App {
        let mut app = make_test_app();
        app.pending_interaction_ids.push("perm-1".into());
        app.slash = Some(SlashState {
            trigger_row: 0,
            trigger_col: 0,
            query: String::new(),
            context: SlashContext::CommandName,
            candidates: vec![SlashCandidate {
                insert_value: "/config".into(),
                primary: "/config".into(),
                secondary: Some("Open settings".into()),
            }],
            dialog: crate::app::dialog::DialogState::default(),
        });
        app
    }

    #[test]
    fn focus_owner_respects_target_priority_and_release_order() {
        let mut app = focus_test_app_with_available_targets();

        assert_eq!(app.focus_owner(), FocusOwner::Input);

        app.claim_focus_target(FocusTarget::Permission);
        assert_eq!(app.focus_owner(), FocusOwner::Permission);

        app.claim_focus_target(FocusTarget::Mention);
        assert_eq!(app.focus_owner(), FocusOwner::Mention);

        app.release_focus_target(FocusTarget::Mention);
        assert_eq!(app.focus_owner(), FocusOwner::Permission);

        app.release_focus_target(FocusTarget::Permission);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }

    #[test]
    fn focus_owner_falls_back_to_input_when_claimed_target_is_unavailable() {
        let mut app = make_test_app();
        app.claim_focus_target(FocusTarget::Permission);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }

    // --- InvalidationLevel tests ---
}
