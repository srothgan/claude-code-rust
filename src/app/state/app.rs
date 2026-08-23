// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

pub(crate) struct PendingSessionResume {
    session_id: String,
    operation_id: Option<String>,
}

#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub surface_mode: SurfaceMode,
    pub(crate) terminal_lifecycle: TerminalLifecycleState,
    pub(crate) surface_dirty: SurfaceDirtyState,
    pub(crate) config: ConfigState,
    pub(crate) global_settings: crate::app::AppSettings,
    pub(crate) global_settings_path: Option<PathBuf>,
    pub(crate) trust: TrustState,
    pub(crate) settings_home_override: Option<PathBuf>,
    pub transcript: Transcript,
    pub session_runtime: SessionRuntimeState,
    pub sdk_inventory: SdkInventoryState,
    pub input: InputState,
    pub status: AppStatus,
    /// Authoritative source and request identity for an in-flight session resume.
    pub(crate) pending_session_resume: Option<PendingSessionResume>,
    /// Whether the synthetic session overview is eligible for chat transcript output.
    pub(crate) show_session_overview: bool,
    pub(crate) shutdown: ShutdownState,
    /// Optional fatal app error that should be surfaced at CLI boundary.
    pub exit_error: Option<crate::error::AppError>,
    pub(crate) cwd: String,
    pub cwd_raw: String,
    pub files_accessed: usize,
    /// State scoped to the currently active turn (command spinner, cancel
    /// bookkeeping, inline interactions, turn-local notices).
    pub turn: TurnState,
    pub(crate) event_tx: mpsc::Sender<ClientEvent>,
    pub(crate) event_rx: mpsc::Receiver<ClientEvent>,
    pub(crate) file_index_event_tx: std_mpsc::SyncSender<file_index::FileIndexEvent>,
    pub(crate) file_index_event_rx: std_mpsc::Receiver<file_index::FileIndexEvent>,
    pub spinner_frame: usize,
    pub(crate) spinner_last_advance_at: Option<Instant>,
    /// Tool scope keyed by tool call ID; used to distinguish main-agent, subagent roots,
    /// and explicitly owned subagent child tools.
    pub(crate) tool_call_scopes: HashMap<String, ToolCallScope>,
    /// Focus manager for directional/navigation key ownership.
    pub(crate) focus: FocusManager,
    /// Resolved keyboard bindings used by chat-surface dispatch.
    pub(crate) keymap: ResolvedKeymap,
    /// Plugin inventory and UI state for the Config > Plugins view.
    pub(crate) plugins: PluginsState,
    /// Recently persisted session IDs discovered at startup.
    pub(crate) recent_sessions: Vec<RecentSessionInfo>,
    /// Selection state for the startup session picker screen.
    pub(crate) session_picker: SessionPickerState,
    /// Deterministic measurement state for the future mutable chat region.
    pub(crate) chat_render: ChatRenderState,
    /// Active `@` file mention autocomplete state.
    pub(crate) mention: Option<mention::MentionState>,
    /// Visual-only literal `@` mentions committed by the user.
    pub(crate) committed_mentions: Vec<mention::CommittedMentionSpan>,
    /// App-owned file index backing `@` file mention autocomplete.
    pub(crate) file_index: file_index::FileIndexState,
    /// Active slash-command autocomplete state.
    pub slash: Option<slash::SlashState>,
    /// Active subagent autocomplete state (`&name`).
    pub(crate) subagent: Option<subagent::SubagentState>,
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
    pub(crate) pending_images: Vec<crate::app::clipboard_image::ImageAttachment>,
    /// Git repo context used by footer/status rendering and live branch tracking.
    pub(crate) git_context: GitContextState,
    /// Update prompt state for the startup fullscreen surface.
    pub(crate) update_prompt: Option<UpdatePromptState>,
    /// Work to run after the TUI has restored the user's terminal.
    pub post_exit_action: Option<super::PostExitAction>,
    /// Config > Usage snapshot and refresh lifecycle.
    pub(crate) usage: UsageState,
    /// Config > MCP live server snapshot and refresh lifecycle.
    pub(crate) mcp: McpState,

    /// Central notification manager (bell + desktop toast when unfocused).
    pub(crate) notifications: notify::NotificationManager,
    /// Performance logger. Present only when built with `--features perf`.
    /// Taken out (`Option::take`) during render, used, then put back to avoid
    /// borrow conflicts with `&mut App`.
    pub(crate) perf: Option<crate::perf::PerfLogger>,
    /// Global in-memory budget for rendered block and message caches.
    pub(crate) render_cache_budget: RenderCacheBudget,
    /// Byte budget for source conversation history retained in memory.
    pub(crate) history_retention: HistoryRetentionPolicy,
    /// Last history-retention enforcement statistics.
    pub(crate) history_retention_stats: HistoryRetentionStats,
    /// Cross-cutting cache metrics accumulator (enforcement counts, watermarks, rate limits).
    pub(crate) cache_metrics: CacheMetrics,
    /// Smoothed frames-per-second (EMA of presented frame cadence).
    pub(crate) fps_ema: Option<f32>,
    /// Timestamp of the previous presented frame.
    pub(crate) last_frame_at: Option<Instant>,
    /// Bootstrap sequencing state resolved from CLI flags at launch.
    pub(crate) startup: StartupState,
    /// Owned bridge-process task and its explicit shutdown signal.
    pub(crate) bridge_task: Option<crate::app::connect::BridgeTask>,
}

impl App {
    #[must_use]
    pub(crate) fn composer_access(&self) -> ComposerAccess {
        if self.shutdown_requested() {
            return ComposerAccess::Blocked(ComposerBlockReason::Shutdown);
        }
        match self.status {
            AppStatus::Connecting => ComposerAccess::DraftOnly,
            AppStatus::CommandPending => {
                ComposerAccess::Blocked(ComposerBlockReason::CommandPending)
            }
            AppStatus::Error => ComposerAccess::Blocked(ComposerBlockReason::Error),
            AppStatus::Ready | AppStatus::Thinking | AppStatus::Running => ComposerAccess::Active,
        }
    }

    pub(crate) fn request_shutdown(&mut self) {
        if matches!(self.shutdown, ShutdownState::Running) {
            self.shutdown = ShutdownState::Requested;
        }
    }

    pub(crate) fn force_shutdown(&mut self) {
        self.shutdown = ShutdownState::Forced;
    }

    #[must_use]
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown.is_requested()
    }

    #[must_use]
    pub(crate) fn shutdown_forced(&self) -> bool {
        self.shutdown.is_forced()
    }

    pub(crate) fn set_pending_session_resume(
        &mut self,
        session_id: String,
        operation_id: Option<String>,
    ) {
        self.pending_session_resume = Some(PendingSessionResume { session_id, operation_id });
    }

    pub(crate) fn clear_pending_session_resume(&mut self) {
        self.pending_session_resume = None;
    }

    pub(crate) fn pending_session_resume_id(&self) -> Option<&str> {
        self.pending_session_resume.as_ref().map(|pending| pending.session_id.as_str())
    }

    pub(crate) fn pending_resume_at_operation_id(&self) -> Option<&str> {
        self.pending_session_resume.as_ref().and_then(|pending| pending.operation_id.as_deref())
    }

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
        let (tx, rx) = mpsc::channel(crate::app::connect::CLIENT_EVENT_QUEUE_CAPACITY);
        let (file_index_tx, file_index_rx) = file_index::event_channel();
        Self {
            surface_mode: SurfaceMode::Chat,
            terminal_lifecycle: TerminalLifecycleState::Running(SurfaceMode::Chat),
            surface_dirty: SurfaceDirtyState::initial_chat(),
            config: ConfigState::default(),
            global_settings: crate::app::AppSettings::default(),
            global_settings_path: None,
            trust: TrustState::default(),
            settings_home_override: None,
            transcript: Transcript::default(),
            session_runtime: SessionRuntimeState::test_default(),
            sdk_inventory: SdkInventoryState::default(),
            input: InputState::new(),
            status: AppStatus::Ready,
            pending_session_resume: None,
            show_session_overview: true,
            turn: TurnState::default(),
            shutdown: ShutdownState::Running,
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
            focus: FocusManager::default(),
            keymap: ResolvedKeymap::defaults(),
            plugins: PluginsState::default(),
            recent_sessions: Vec::new(),
            session_picker: SessionPickerState::default(),
            chat_render: ChatRenderState::default(),
            mention: None,
            committed_mentions: Vec::new(),
            file_index: file_index::FileIndexState::default(),
            slash: None,
            subagent: None,
            pending_submit: None,
            paste: PasteState::default(),
            pending_images: Vec::new(),
            git_context: GitContextState::default(),
            update_prompt: None,
            post_exit_action: None,
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
            startup: StartupState::default(),
            bridge_task: None,
        }
    }

    #[cfg(test)]
    pub fn test_request_startup_session_picker(&mut self) {
        self.startup = StartupState::new(None, None, true);
    }
}
