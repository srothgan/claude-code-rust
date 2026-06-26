// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

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
    /// Current SDK task state from `TaskCreate`/`TaskUpdate`/`TaskGet`/`TaskList`.
    pub tasks: Vec<model::TaskItem>,
    /// Focus manager for directional/navigation key ownership.
    pub focus: FocusManager,
    /// Resolved keyboard bindings used by chat-surface dispatch.
    pub keymap: ResolvedKeymap,
    /// Commands advertised by the agent via `AvailableCommandsUpdate`.
    pub available_commands: Vec<model::AvailableCommand>,
    /// Rewind candidates loaded from persisted SDK session history.
    pub rewind_targets: Vec<model::RewindTarget>,
    /// Session id that owns `rewind_targets`.
    pub rewind_targets_session_id: Option<model::SessionId>,
    /// True while a rewind target refresh request is in flight.
    pub rewind_targets_in_flight: bool,
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
    pub paste_burst: paste_burst::PasteBurstDetector,
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
    pub account_info: Option<model::AccountInfo>,

    /// Indexed terminal tool calls for per-frame terminal snapshot updates.
    /// Avoids O(n*m) scan of all messages/blocks every frame.
    pub terminal_tool_calls: Vec<TerminalToolCallRef>,
    /// Membership index for `terminal_tool_calls`, used to avoid linear duplicate checks.
    pub terminal_tool_call_membership: HashSet<TerminalToolCallRef>,
    /// Central notification manager (bell + desktop toast when unfocused).
    pub notifications: notify::NotificationManager,
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
    #[must_use]
    pub fn session_thinking_effort_effective(&self) -> model::EffortLevel {
        self.config_options
            .get("effortLevel")
            .and_then(serde_json::Value::as_str)
            .and_then(model::EffortLevel::from_stored)
            .unwrap_or_else(|| self.config.thinking_effort_effective())
    }

    #[must_use]
    pub fn is_project_trusted(&self) -> bool {
        self.trust.is_trusted()
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
            tasks: Vec::new(),
            focus: FocusManager::default(),
            keymap: ResolvedKeymap::defaults(),
            available_commands: Vec::new(),
            rewind_targets: Vec::new(),
            rewind_targets_session_id: None,
            rewind_targets_in_flight: false,
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
            paste_burst: paste_burst::PasteBurstDetector::new(),
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
            notifications: notify::NotificationManager::new(),
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
}
