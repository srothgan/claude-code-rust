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
    pub transcript: Transcript,
    pub session_runtime: SessionRuntimeState,
    pub input: InputState,
    pub status: AppStatus,
    /// Session id currently being resumed via `/resume`.
    pub resuming_session_id: Option<String>,
    /// Whether the synthetic session overview is eligible for chat transcript output.
    pub show_session_overview: bool,
    pub should_quit: bool,
    /// Optional fatal app error that should be surfaced at CLI boundary.
    pub exit_error: Option<crate::error::AppError>,
    pub cwd: String,
    pub cwd_raw: String,
    pub files_accessed: usize,
    /// State scoped to the currently active turn (command spinner, cancel
    /// bookkeeping, inline interactions, turn-local notices).
    pub turn: TurnState,
    pub event_tx: mpsc::UnboundedSender<ClientEvent>,
    pub event_rx: mpsc::UnboundedReceiver<ClientEvent>,
    pub file_index_event_tx: std_mpsc::Sender<file_index::FileIndexEvent>,
    pub file_index_event_rx: std_mpsc::Receiver<file_index::FileIndexEvent>,
    pub spinner_frame: usize,
    pub spinner_last_advance_at: Option<Instant>,
    /// Tool scope keyed by tool call ID; used to distinguish main-agent, subagent roots,
    /// and explicitly owned subagent child tools.
    pub tool_call_scopes: HashMap<String, ToolCallScope>,
    /// Shared terminal process map - used to snapshot output on completion.
    pub terminals: crate::agent::events::TerminalMap,
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
    /// Paste ingestion state: burst detection, queued chunks, session tracking.
    pub paste: PasteState,
    /// Pending image attachments accumulated via Ctrl+V clipboard reads and
    /// consumed on submit. No cap on count — this is a developer tool, so
    /// users are trusted to attach as many images as they need.
    pub pending_images: Vec<crate::app::clipboard_image::ImageAttachment>,
    /// Git repo context used by footer/status rendering and live branch tracking.
    pub(crate) git_context: GitContextState,
    /// Update availability state for the current app lifetime.
    pub update_notice: Option<UpdateNoticeState>,
    /// Config > Usage snapshot and refresh lifecycle.
    pub usage: UsageState,
    /// Config > MCP live server snapshot and refresh lifecycle.
    pub mcp: McpState,

    /// Central notification manager (bell + desktop toast when unfocused).
    pub notifications: notify::NotificationManager,
    /// Performance logger. Present only when built with `--features perf`.
    /// Taken out (`Option::take`) during render, used, then put back to avoid
    /// borrow conflicts with `&mut App`.
    pub perf: Option<crate::perf::PerfLogger>,
    /// Global in-memory budget for rendered block and message caches.
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
    /// Last emitted chat render trace snapshot to suppress identical per-frame summaries.
    pub last_chat_render_trace_state: Option<ChatRenderTraceState>,
    /// Bootstrap sequencing state resolved from CLI flags at launch.
    pub startup: StartupState,
}

impl App {
    #[must_use]
    pub fn session_thinking_effort_effective(&self) -> model::EffortLevel {
        self.session_runtime
            .config_options
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
            transcript: Transcript::default(),
            session_runtime: SessionRuntimeState::test_default(),
            input: InputState::new(),
            status: AppStatus::Ready,
            resuming_session_id: None,
            show_session_overview: true,
            turn: TurnState::default(),
            should_quit: false,
            exit_error: None,
            cwd: "/test".into(),
            cwd_raw: "/test".into(),
            files_accessed: 0,
            event_tx: tx,
            event_rx: rx,
            file_index_event_tx: file_index_tx,
            file_index_event_rx: file_index_rx,
            spinner_frame: 0,
            spinner_last_advance_at: None,
            tool_call_scopes: HashMap::default(),
            terminals: std::rc::Rc::default(),
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
            paste: PasteState::default(),
            pending_images: Vec::new(),
            git_context: GitContextState::default(),
            update_notice: None,
            usage: UsageState::default(),
            mcp: McpState::default(),
            notifications: notify::NotificationManager::new(),
            perf: None,
            render_cache_budget: RenderCacheBudget::default(),
            history_retention: HistoryRetentionPolicy::default(),
            history_retention_stats: HistoryRetentionStats::default(),
            cache_metrics: CacheMetrics::default(),
            fps_ema: None,
            last_frame_at: None,
            last_chat_render_trace_state: None,
            startup: StartupState::default(),
        }
    }

    #[cfg(test)]
    pub fn test_request_startup_session_picker(&mut self) {
        self.startup = StartupState::new(None, None, true);
    }
}
