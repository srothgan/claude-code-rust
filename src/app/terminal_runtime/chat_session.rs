// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::chat_terminal::{
    ChatDrawOutcome, ChatDrawRequest, ChatTerminal, HistoryBatchKind, PendingHistoryBatch,
    TerminalResizeDuringDraw, TerminalSnapshot,
};
use super::history_insert::RenderedHistoryRows;
use crate::app::{App, ChatPurgeReplayOptions, HistoryOutputId};
use crate::ui::footer_rows::serialize_footer_rows;
use crate::ui::inline_chat_rows::LiveRowBoundaryKind;
use crate::ui::inline_chat_rows::{
    SerializedLiveRows, serialize_live_rows_with_boundaries_excluding,
};
use crate::ui::input;
use crate::ui::input_rows::{blocked_input_lines, build_btw_status_rows, build_composer_hint_rows};
use crate::ui::theme;
use anyhow::Context;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::BTreeSet;

pub(super) struct ChatTerminalSession {
    terminal: ChatTerminal,
    history: HistoryCommitState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ChatTerminalSeed {
    terminal_width: u16,
    terminal_height: u16,
    cursor_x: u16,
    cursor_y: u16,
    provenance: ChatTerminalSeedProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChatTerminalSeedProvenance {
    BeforeInputReaderMeasured,
    CachedBeforeFullscreen,
    ConservativeAfterResume,
    ConservativeAfterFullscreen,
}

impl ChatTerminalSeed {
    pub(super) fn read_before_input_reader() -> anyhow::Result<Self> {
        let (terminal_width, terminal_height) =
            crossterm::terminal::size().context("failed to read chat terminal size")?;
        let (cursor_x, cursor_y) = read_cursor_position_before_input_reader()
            .context("failed to read chat terminal cursor")?;
        Ok(Self {
            terminal_width,
            terminal_height,
            cursor_x,
            cursor_y,
            provenance: ChatTerminalSeedProvenance::BeforeInputReaderMeasured,
        })
    }

    pub(super) fn conservative_current(
        provenance: ChatTerminalSeedProvenance,
    ) -> anyhow::Result<Self> {
        let (terminal_width, terminal_height) =
            crossterm::terminal::size().context("failed to read chat terminal size")?;
        Ok(Self {
            terminal_width,
            terminal_height,
            cursor_x: 0,
            cursor_y: terminal_height.saturating_sub(1),
            provenance,
        })
    }

    pub(super) fn with_current_size(
        self,
        provenance: ChatTerminalSeedProvenance,
    ) -> anyhow::Result<Self> {
        let (terminal_width, terminal_height) =
            crossterm::terminal::size().context("failed to read chat terminal size")?;
        Ok(Self {
            terminal_width,
            terminal_height,
            cursor_x: self.cursor_x.min(terminal_width.saturating_sub(1)),
            cursor_y: self.cursor_y.min(terminal_height.saturating_sub(1)),
            provenance,
        })
    }
}

impl ChatTerminalSession {
    pub(super) fn new_before_input_reader() -> anyhow::Result<Self> {
        Ok(Self::new_with_seed(ChatTerminalSeed::read_before_input_reader()?))
    }

    pub(super) fn new_with_seed(seed: ChatTerminalSeed) -> Self {
        let width = seed.terminal_width.max(1);
        let height = seed.terminal_height.max(1);
        let cursor_x = seed.cursor_x.min(width.saturating_sub(1));
        let cursor_y = seed.cursor_y.min(height.saturating_sub(1));
        let owned_top = cursor_y;

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_backend_mode",
            message = "chat runtime configured for ratatui inline viewport",
            outcome = "success",
            backend = "ratatui_inline_viewport",
        );
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_terminal_initialized",
            message = "ratatui inline chat terminal session initialized",
            outcome = "success",
            terminal_width = width,
            terminal_height = height,
            cursor_x,
            cursor_y,
            owned_top,
            seed_provenance = ?seed.provenance,
        );

        Self { terminal: ChatTerminal::new(owned_top), history: HistoryCommitState::default() }
    }

    pub(super) fn clear(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
        self.history.reset();
    }

    pub(super) fn clear_for_purge_replay(
        &mut self,
        app: &mut App,
        options: ChatPurgeReplayOptions,
    ) {
        if let Err(err) = self.terminal.reset_purge_replay(options.reason.label()) {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_purge_replay_failed",
                message = "failed to purge terminal before transcript replay",
                outcome = "failure",
                reason = %options.reason.label(),
                error_message = %err,
            );
        }
        app.chat_render.invalidate_live_anchor();
        self.history.reset_for_purge_replay();
    }

    pub(super) fn clear_mutable_viewport(&mut self, app: &mut App) {
        if let Err(err) = self.terminal.reset_mutable_viewport() {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_mutable_viewport_clear_failed",
                message = "failed to clear inline terminal mutable viewport",
                outcome = "failure",
                error_message = %err,
            );
        }
        app.chat_render.invalidate_live_anchor();
    }

    pub(super) fn suspend_for_fullscreen(&mut self, app: &mut App) {
        app.chat_render.invalidate_live_anchor();
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_fullscreen_suspended",
            message = "inline chat session suspended before fullscreen surface",
            outcome = "success",
            confirmed_ids = self.history.confirmed_len(),
        );
    }

    pub(super) fn reattach_after_fullscreen(&mut self, app: &mut App) {
        app.chat_render.invalidate_live_anchor();
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_fullscreen_reattached",
            message = "inline chat session reattached after fullscreen surface",
            outcome = "success",
            confirmed_ids = self.history.confirmed_len(),
        );
    }

    pub(super) fn draw(&mut self, app: &mut App) -> anyhow::Result<()> {
        ChatTerminal::ensure_line_wrap_disabled(&mut app.chat_render.line_wrap_disabled)?;

        // The recorded size is owned by the resize path. A live size that differs
        // from it fails the draw-start snapshot check and is replayed after reconcile.
        let recorded_size = app.chat_render.terminal_size();
        let screen_size = (recorded_size.width, recorded_size.height);
        let width = screen_size.0.max(1);
        let terminal_height = screen_size.1.max(1);

        if app.shutdown_requested() {
            // Reading displays a copy of committed history in the live viewport.
            // Remove that copy before leaving the transcript to the terminal.
            app.chat_render.viewport.resume();
        } else if !app.config.auto_scroll_effective() {
            app.chat_render.viewport.pause();
        }
        let base_excluded_ids = self.base_history_excluded_ids();
        let serialization_excluded_ids = if app.chat_render.viewport.is_reading() {
            BTreeSet::new()
        } else {
            base_excluded_ids.clone()
        };
        let serialized_rows =
            serialize_live_rows_with_boundaries_excluding(app, width, &serialization_excluded_ids);
        self.draw_incremental(
            app,
            screen_size,
            width,
            terminal_height,
            &serialized_rows,
            base_excluded_ids,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn draw_incremental(
        &mut self,
        app: &mut App,
        screen_size: (u16, u16),
        width: u16,
        terminal_height: u16,
        serialized_rows: &SerializedLiveRows,
        base_excluded_ids: BTreeSet<HistoryOutputId>,
    ) -> anyhow::Result<()> {
        app.surface_dirty.chat.take_repaint();
        let composer = Self::build_composer_surface(app, width);
        let mut history_plan =
            self.prepare_history_flush(serialized_rows, width, base_excluded_ids);
        // Reading chooses the visible source window independently of restoring
        // terminal history. Confirmed and queued IDs still prevent duplicate inserts.
        if app.chat_render.viewport.is_reading() {
            history_plan.live_rows = serialized_rows.rows().to_vec();
            history_plan.excluded_rows = 0;
        }
        let live_rows = history_plan.live_rows.as_slice();
        let (requested_layout_plan, layout_plan) =
            viewport_layout(app, serialized_rows, live_rows, &composer, terminal_height);
        let visible_activity_rows =
            layout_plan.activity_window.slice(&composer.activity_rows).to_vec();
        let visible_hint_rows = layout_plan.hint_visible_rows(&composer.hint_rows).to_vec();
        let visible_btw_rows = layout_plan.btw_visible_rows(&composer.btw_rows).to_vec();
        let visible_editor_rows = composer.editor_visible_rows(layout_plan.editor_height).to_vec();
        let visible_footer_rows = layout_plan.footer_visible_rows(&composer.footer_rows).to_vec();
        let composer_preview_rows = composer.preview_rows(
            &visible_activity_rows,
            &visible_hint_rows,
            &visible_btw_rows,
            &visible_editor_rows,
            &visible_footer_rows,
        );
        let visible_composer_row_count = layout_plan.visible_composer_len();

        log_prepared_draw(&PreparedDrawLog {
            app,
            serialized_rows,
            live_rows,
            layout_plan,
            composer: &composer,
            visible_composer_row_count,
            composer_preview_rows: &composer_preview_rows,
            history_plan: &history_plan,
        });

        let visible_live_rows = layout_plan.live_visible_rows(live_rows).to_vec();
        let live_rows_mutable_count = live_rows.len();
        let visible_live_row_count = visible_live_rows.len();
        let chat_frame = ChatDrawRequest {
            requested_inline_height: requested_layout_plan.viewport_height,
            terminal: TerminalSnapshot::new(width, terminal_height),
        };
        let history_action = history_plan.take_action();
        self.queue_history_plan(history_action);
        let outcome_result = self.terminal.draw_chat_frame(chat_frame, |frame, viewport_area| {
            let (live_area, activity_area, hint_area, btw_area, editor_area, footer_area) =
                layout_plan.areas(viewport_area);
            if !live_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_live_rows.clone()), live_area);
            }
            if !activity_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_activity_rows.clone()), activity_area);
            }
            if !hint_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_hint_rows.clone()), hint_area);
            }
            if !btw_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_btw_rows.clone()), btw_area);
            }
            if !editor_area.is_empty() {
                render_composer_editor(frame, app, &composer, editor_area);
            }
            if !footer_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_footer_rows.clone()), footer_area);
            }
        });
        let Some(outcome) = self.resolve_chat_draw_outcome(app, outcome_result)? else {
            return Ok(());
        };
        record_viewport_anchor(
            app,
            serialized_rows,
            layout_plan.live_window.start,
            visible_live_row_count,
        );
        self.complete_history_flush(app, &outcome);
        let viewport_area = outcome.viewport_area;
        let (live_area, activity_area, hint_area, btw_area, editor_area, footer_area) =
            layout_plan.areas(viewport_area);
        complete_draw(
            app,
            DrawCompletion {
                viewport_area,
                live_area,
                activity_area,
                hint_area,
                btw_area,
                editor_area,
                footer_area,
                requested_inline_height: requested_layout_plan.viewport_height,
                terminal_width: screen_size.0,
                terminal_height: screen_size.1,
                live_rows_total: serialized_rows.rows().len(),
                live_rows_mutable: live_rows_mutable_count,
                live_rows_visible: visible_live_row_count,
                live_rows_hidden_above: history_plan
                    .excluded_rows
                    .saturating_add(layout_plan.live_window.hidden_rows_above()),
                composer_rows_total: composer.total_len(),
                composer_rows_visible: visible_composer_row_count,
                scrollback_inserted_rows: outcome.flushed_history.flushed_rows,
            },
        );
        Ok(())
    }

    fn resolve_chat_draw_outcome(
        &mut self,
        app: &mut App,
        outcome_result: anyhow::Result<ChatDrawOutcome>,
    ) -> anyhow::Result<Option<ChatDrawOutcome>> {
        match outcome_result {
            Ok(outcome) => Ok(Some(outcome)),
            Err(err) => {
                self.history.mark_out_of_sync();
                mark_chat_terminal_history_out_of_sync(app);
                if let Some(interrupt) = err.downcast_ref::<TerminalResizeDuringDraw>() {
                    log_resize_interrupted_draw(*interrupt);
                    return Ok(None);
                }
                Err(err)
            }
        }
    }

    fn prepare_history_flush(
        &mut self,
        serialized_rows: &SerializedLiveRows,
        width: u16,
        base_excluded_ids: BTreeSet<HistoryOutputId>,
    ) -> HistoryFlushPlan {
        let width = width.max(1);
        if !self.history.is_synced() {
            return self.prepare_replay_history_flush(serialized_rows, width, base_excluded_ids);
        }

        Self::prepare_static_history_flush(serialized_rows, width, base_excluded_ids)
    }

    fn base_history_excluded_ids(&self) -> BTreeSet<HistoryOutputId> {
        let mut excluded_ids = self.history.confirmed_ids().clone();
        excluded_ids.extend(self.terminal.pending_history_ids());
        excluded_ids
    }

    fn prepare_replay_history_flush(
        &mut self,
        serialized_rows: &SerializedLiveRows,
        width: u16,
        base_excluded_ids: BTreeSet<HistoryOutputId>,
    ) -> HistoryFlushPlan {
        if self.terminal.is_replay_active() {
            if self.terminal.active_replay_matches_width(width) {
                let mut excluded_ids = base_excluded_ids;
                excluded_ids.extend(self.terminal.replay_pending_ids());
                return history_flush_without_action(serialized_rows, excluded_ids, true);
            }

            self.terminal.cancel_replay();
        }

        let replay = build_replay_history_batches(serialized_rows, width);
        if replay.confirm_ids.is_empty() {
            self.history.confirm(Vec::new());
            return history_flush_without_action(serialized_rows, base_excluded_ids, true);
        }

        let mut excluded_ids = base_excluded_ids;
        excluded_ids.extend(replay.confirm_ids.iter().cloned());
        let live_rows = serialized_rows.rows_excluding_ids(&excluded_ids);
        let replay_rows = replay.rows;
        HistoryFlushPlan::new(
            live_rows,
            excluded_row_count(serialized_rows, &excluded_ids),
            excluded_ids,
            HistoryFlushAction::StartReplay {
                render_width: width,
                batches: replay.batches,
                confirm_ids: replay.confirm_ids,
            },
            replay_rows,
            true,
        )
    }

    fn prepare_static_history_flush(
        serialized_rows: &SerializedLiveRows,
        width: u16,
        base_excluded_ids: BTreeSet<HistoryOutputId>,
    ) -> HistoryFlushPlan {
        let candidate_batches =
            build_static_history_batches(serialized_rows, width, &base_excluded_ids);
        if candidate_batches.is_empty() {
            return history_flush_without_action(serialized_rows, base_excluded_ids, false);
        }

        let candidate_ids = candidate_batches
            .iter()
            .flat_map(|batch| batch.confirm_ids.iter().cloned())
            .collect::<Vec<_>>();
        let mut candidate_excluded_ids = base_excluded_ids.clone();
        candidate_excluded_ids.extend(candidate_ids);
        let candidate_live_rows = serialized_rows.rows_excluding_ids(&candidate_excluded_ids);
        let candidate_rows = candidate_batches.iter().map(|batch| batch.rows.len()).sum();
        HistoryFlushPlan::new(
            candidate_live_rows,
            excluded_row_count(serialized_rows, &candidate_excluded_ids),
            candidate_excluded_ids,
            HistoryFlushAction::QueueStatic(candidate_batches),
            candidate_rows,
            false,
        )
    }

    fn queue_history_plan(&mut self, action: HistoryFlushAction) {
        match action {
            HistoryFlushAction::None => {}
            HistoryFlushAction::QueueStatic(batches) => {
                for batch in batches {
                    self.terminal.queue_static_history(batch);
                }
            }
            HistoryFlushAction::StartReplay { render_width, batches, confirm_ids } => {
                self.terminal.start_replay(render_width, batches, confirm_ids);
            }
        }
    }

    fn complete_history_flush(
        &mut self,
        app: &mut App,
        outcome: &super::chat_terminal::ChatDrawOutcome,
    ) {
        if outcome.flushed_history.replay_complete
            || !outcome.flushed_history.confirmed_ids.is_empty()
        {
            self.history.confirm(outcome.flushed_history.confirmed_ids.clone());
        }

        if outcome.flushed_history.replay_incomplete
            || (outcome.flushed_history.flushed_rows > 0
                && !self.terminal.pending_history_ids().is_empty())
        {
            app.request_chat_repaint();
        }
    }

    fn reset_inline_terminal(&mut self, app: &mut App) {
        if let Err(err) = self.terminal.reset_visible() {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_mutable_viewport_clear_failed",
                message = "failed to clear inline terminal before reset",
                outcome = "failure",
                error_message = %err,
            );
        }
        app.chat_render.invalidate_live_anchor();
    }

    fn build_composer_surface(app: &mut App, width: u16) -> ComposerSurface {
        let activity_rows =
            crate::ui::activity_rows::build_activity_rows(app, width, std::time::Instant::now());
        let activity_row_count = u16::try_from(activity_rows.len()).unwrap_or(u16::MAX);
        let mut hint_rows = build_composer_hint_rows(app);
        if app.chat_render.viewport.is_reading() {
            hint_rows.extend(crate::ui::input_rows::reading_hint_rows(app, width));
        }
        let hint_row_count = u16::try_from(hint_rows.len()).unwrap_or(u16::MAX);
        let btw_rows = build_btw_status_rows(app, width);
        let btw_row_count = u16::try_from(btw_rows.len()).unwrap_or(u16::MAX);
        let footer = serialize_footer_rows(app, width);
        let footer_rows = Vec::from(footer.rows);
        let footer_row_count = u16::try_from(footer_rows.len()).unwrap_or(u16::MAX);

        let editor = if let Some(reason) = app.composer_access().blocked_reason() {
            ComposerEditor::Rows(blocked_input_lines(app, reason, width))
        } else {
            let desired_height =
                input::visual_line_count(app, width).saturating_sub(hint_row_count).max(1);
            ComposerEditor::TextArea { desired_height }
        };
        let rule = crate::ui::session_rule::session_rule_line(
            app.session_runtime.session_title.as_deref(),
            width,
        );
        let editor_row_count = editor.total_len_u16().saturating_add(u16::from(rule.is_some()));

        app.chat_render.composer.width = width;
        app.chat_render.composer.activity_rows = activity_row_count;
        app.chat_render.composer.hint_rows = hint_row_count;
        app.chat_render.composer.btw_rows = btw_row_count;
        app.chat_render.composer.editor_rows = editor_row_count;
        app.chat_render.composer.footer_rows = footer_row_count;
        app.chat_render.composer.total_rows = activity_row_count
            .saturating_add(hint_row_count)
            .saturating_add(btw_row_count)
            .saturating_add(editor_row_count)
            .saturating_add(footer_row_count);
        app.chat_render.composer.caret_row = 0;
        app.chat_render.composer.caret_col = 0;

        ComposerSurface { activity_rows, hint_rows, btw_rows, rule, editor, footer_rows }
    }
}

fn read_cursor_position_before_input_reader() -> anyhow::Result<(u16, u16)> {
    crossterm::cursor::position()
        .context("cursor position may only be read before the terminal reader owns terminal input")
}

fn viewport_layout(
    app: &mut App,
    rows: &SerializedLiveRows,
    live: &[Line<'static>],
    composer: &ComposerSurface,
    height: u16,
) -> (MutableLayoutPlan, MutableLayoutPlan) {
    let requested = MutableLayoutPlan::new(live, composer, height);
    let mut layout = MutableLayoutPlan::new(live, composer, requested.viewport_height);
    if app.chat_render.viewport.is_reading() {
        layout.live_window.start =
            app.chat_render.viewport.start(rows, layout.live_window.visible_len);
    }
    (requested, layout)
}

fn record_viewport_anchor(
    app: &mut App,
    rows: &SerializedLiveRows,
    reading_start: usize,
    visible: usize,
) {
    let start = if app.chat_render.viewport.is_reading() {
        reading_start
    } else {
        rows.rows().len().saturating_sub(visible)
    };
    app.chat_render.viewport.record(rows, start);
}

fn mark_chat_terminal_history_out_of_sync(app: &mut App) {
    app.request_chat_purge_replay_rebuild(ChatPurgeReplayOptions::terminal_history_out_of_sync());
}

fn log_resize_interrupted_draw(interrupt: TerminalResizeDuringDraw) {
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_draw_interrupted_by_resize",
        message = "inline chat draw skipped because terminal size changed during transaction",
        outcome = "interrupted",
        phase = interrupt.phase(),
        expected_width = interrupt.expected().width,
        expected_height = interrupt.expected().height,
        actual_width = interrupt.actual().width,
        actual_height = interrupt.actual().height,
    );
}

fn log_prepared_draw(prepared: &PreparedDrawLog<'_>) {
    log_inline_chat_draw(&InlineChatDrawSummary {
        app: prepared.app,
        live_rows_total: prepared.serialized_rows.rows(),
        live_rows_visible: prepared.layout_plan.live_visible_rows(prepared.live_rows),
        live_rows_mutable: prepared.live_rows.len(),
        composer_rows_total: prepared.composer.total_len(),
        composer_rows_visible: prepared.visible_composer_row_count,
        composer_preview: preview_rows(prepared.composer_preview_rows, 3),
        live_rows_hidden_above: prepared
            .history_plan
            .excluded_rows
            .saturating_add(prepared.layout_plan.live_window.hidden_rows_above()),
        history_queued_rows: prepared.history_plan.queued_rows,
        history_excluded_rows: prepared.history_plan.excluded_rows,
        history_excluded_ids: prepared.history_plan.excluded_ids.len(),
        history_full_rebuild: prepared.history_plan.full_rebuild,
        stable_rows: prepared.serialized_rows.stable_row_count(),
        first_mutable_boundary_start: prepared.serialized_rows.first_mutable_boundary_start(),
        first_mutable_boundary_kind: prepared.serialized_rows.first_mutable_boundary_kind(),
        first_mutable_boundary_msg_idx: prepared.serialized_rows.first_mutable_boundary_msg_idx(),
        first_mutable_boundary_block_idx: prepared
            .serialized_rows
            .first_mutable_boundary_block_idx(),
    });
}

struct PreparedDrawLog<'a> {
    app: &'a App,
    serialized_rows: &'a SerializedLiveRows,
    live_rows: &'a [Line<'static>],
    layout_plan: MutableLayoutPlan,
    composer: &'a ComposerSurface,
    visible_composer_row_count: usize,
    composer_preview_rows: &'a [Line<'static>],
    history_plan: &'a HistoryFlushPlan,
}

fn complete_draw(app: &mut App, completion: DrawCompletion) {
    app.chat_render.live_region.anchor_valid = true;
    app.chat_render.live_region.total_rows =
        u16::try_from(completion.live_rows_total).unwrap_or(u16::MAX);
    app.chat_render.live_region.hidden_rows_above =
        u16::try_from(completion.live_rows_hidden_above).unwrap_or(u16::MAX);
    app.chat_render.live_region.viewport_height = completion.viewport_area.height;
    app.chat_render.live_region.last_rendered_rows =
        u16::try_from(completion.live_rows_visible).unwrap_or(u16::MAX);
    app.chat_render.composer.last_rendered_rows =
        u16::try_from(completion.composer_rows_visible).unwrap_or(u16::MAX);

    log_inline_viewport_draw(&InlineViewportDrawMetrics {
        viewport_area: completion.viewport_area,
        live_area: completion.live_area,
        activity_area: completion.activity_area,
        hint_area: completion.hint_area,
        btw_area: completion.btw_area,
        editor_area: completion.editor_area,
        footer_area: completion.footer_area,
        requested_inline_height: completion.requested_inline_height,
        terminal_width: completion.terminal_width,
        terminal_height: completion.terminal_height,
        live_rows_total: completion.live_rows_total,
        live_rows_mutable: completion.live_rows_mutable,
        live_rows_visible: completion.live_rows_visible,
        live_rows_hidden_above: completion.live_rows_hidden_above,
        composer_rows_total: completion.composer_rows_total,
        composer_rows_visible: completion.composer_rows_visible,
        scrollback_inserted_rows: completion.scrollback_inserted_rows,
    });
}

#[derive(Debug, Clone, Copy)]
struct DrawCompletion {
    viewport_area: Rect,
    live_area: Rect,
    activity_area: Rect,
    hint_area: Rect,
    btw_area: Rect,
    editor_area: Rect,
    footer_area: Rect,
    requested_inline_height: u16,
    terminal_width: u16,
    terminal_height: u16,
    live_rows_total: usize,
    live_rows_mutable: usize,
    live_rows_visible: usize,
    live_rows_hidden_above: usize,
    composer_rows_total: usize,
    composer_rows_visible: usize,
    scrollback_inserted_rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryCommitState {
    confirmed: BTreeSet<HistoryOutputId>,
    history_in_sync: bool,
}

impl Default for HistoryCommitState {
    fn default() -> Self {
        Self { confirmed: BTreeSet::new(), history_in_sync: true }
    }
}

impl HistoryCommitState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn reset_for_purge_replay(&mut self) {
        *self = Self { history_in_sync: false, ..Self::default() };
    }

    fn confirmed_ids(&self) -> &BTreeSet<HistoryOutputId> {
        &self.confirmed
    }

    fn confirmed_len(&self) -> usize {
        self.confirmed.len()
    }

    fn is_synced(&self) -> bool {
        self.history_in_sync
    }

    fn confirm(&mut self, ids: Vec<HistoryOutputId>) {
        self.confirmed.extend(ids);
        self.history_in_sync = true;
    }

    fn mark_out_of_sync(&mut self) {
        self.confirmed.clear();
        self.history_in_sync = false;
    }
}

struct HistoryFlushPlan {
    live_rows: Vec<Line<'static>>,
    excluded_rows: usize,
    excluded_ids: BTreeSet<HistoryOutputId>,
    action: HistoryFlushAction,
    queued_rows: usize,
    full_rebuild: bool,
}

impl HistoryFlushPlan {
    fn new(
        live_rows: Vec<Line<'static>>,
        excluded_rows: usize,
        excluded_ids: BTreeSet<HistoryOutputId>,
        action: HistoryFlushAction,
        queued_rows: usize,
        full_rebuild: bool,
    ) -> Self {
        Self { live_rows, excluded_rows, excluded_ids, action, queued_rows, full_rebuild }
    }

    fn take_action(&mut self) -> HistoryFlushAction {
        std::mem::replace(&mut self.action, HistoryFlushAction::None)
    }
}

enum HistoryFlushAction {
    None,
    QueueStatic(Vec<PendingHistoryBatch>),
    StartReplay {
        render_width: u16,
        batches: Vec<PendingHistoryBatch>,
        confirm_ids: Vec<HistoryOutputId>,
    },
}

struct ReplayHistoryPlan {
    batches: Vec<PendingHistoryBatch>,
    confirm_ids: Vec<HistoryOutputId>,
    rows: usize,
}

fn history_flush_without_action(
    serialized_rows: &SerializedLiveRows,
    excluded_ids: BTreeSet<HistoryOutputId>,
    full_rebuild: bool,
) -> HistoryFlushPlan {
    let live_rows = serialized_rows.rows_excluding_ids(&excluded_ids);
    HistoryFlushPlan::new(
        live_rows,
        excluded_row_count(serialized_rows, &excluded_ids),
        excluded_ids,
        HistoryFlushAction::None,
        0,
        full_rebuild,
    )
}

fn build_static_history_batches(
    serialized_rows: &SerializedLiveRows,
    width: u16,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> Vec<PendingHistoryBatch> {
    let stable_row_count = serialized_rows.stable_row_count();
    let mut batches = Vec::new();
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    let mut expected_start = None;

    for segment in serialized_rows.segments() {
        if !segment.commit_ready || segment.start_row >= stable_row_count {
            break;
        }
        let segment_ids = unexcluded_ids(&segment.ids, excluded_ids);
        if segment_ids.is_empty() {
            continue;
        }
        if expected_start.is_some_and(|expected| expected != segment.start_row) {
            push_history_batch(&mut batches, HistoryBatchKind::Normal, width, &mut rows, &mut ids);
        }

        let Some(segment_rows) = serialized_rows.segment_rows(segment) else {
            continue;
        };
        rows.extend(segment_rows.iter().cloned());
        ids.extend(segment_ids);
        expected_start = Some(segment.end_row);
    }

    push_history_batch(&mut batches, HistoryBatchKind::Normal, width, &mut rows, &mut ids);
    batches
}

fn build_replay_history_batches(
    serialized_rows: &SerializedLiveRows,
    width: u16,
) -> ReplayHistoryPlan {
    let stable_row_count = serialized_rows.stable_row_count();
    let stable_segments = serialized_rows
        .segments()
        .iter()
        .filter(|segment| segment.commit_ready && segment.start_row < stable_row_count)
        .filter(|segment| serialized_rows.segment_rows(segment).is_some())
        .collect::<Vec<_>>();
    let confirm_ids = unique_ids(stable_segments.iter().flat_map(|segment| segment.ids.iter()));

    let mut batches = Vec::new();
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    let mut queued_rows = 0usize;
    let mut expected_start = None;
    let empty = BTreeSet::new();

    for segment in stable_segments {
        let segment_ids = unexcluded_ids(&segment.ids, &empty);
        if segment_ids.is_empty() {
            continue;
        }
        if expected_start.is_some_and(|expected| expected != segment.start_row) || rows.len() >= 160
        {
            queued_rows = queued_rows.saturating_add(rows.len());
            push_history_batch(&mut batches, HistoryBatchKind::Replay, width, &mut rows, &mut ids);
        }
        let Some(segment_rows) = serialized_rows.segment_rows(segment) else {
            continue;
        };
        rows.extend(segment_rows.iter().cloned());
        ids.extend(segment_ids);
        expected_start = Some(segment.end_row);
    }

    queued_rows = queued_rows.saturating_add(rows.len());
    push_history_batch(&mut batches, HistoryBatchKind::Replay, width, &mut rows, &mut ids);

    ReplayHistoryPlan { batches, confirm_ids, rows: queued_rows }
}

fn push_history_batch(
    batches: &mut Vec<PendingHistoryBatch>,
    kind: HistoryBatchKind,
    width: u16,
    rows: &mut Vec<Line<'static>>,
    ids: &mut Vec<HistoryOutputId>,
) {
    if rows.is_empty() {
        ids.clear();
        return;
    }
    batches.push(PendingHistoryBatch::new(
        kind,
        RenderedHistoryRows::new(width, std::mem::take(rows)),
        unique_ids(ids.iter()),
    ));
    ids.clear();
}

fn unexcluded_ids(
    ids: &[HistoryOutputId],
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> Vec<HistoryOutputId> {
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|id| !excluded_ids.contains(*id))
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

fn unique_ids<'a>(ids: impl IntoIterator<Item = &'a HistoryOutputId>) -> Vec<HistoryOutputId> {
    let mut seen = BTreeSet::new();
    ids.into_iter().filter(|id| seen.insert((*id).clone())).cloned().collect()
}

fn excluded_row_count(
    serialized_rows: &SerializedLiveRows,
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> usize {
    serialized_rows
        .segments()
        .iter()
        .filter(|segment| segment.ids.iter().all(|id| excluded_ids.contains(id)))
        .map(|segment| match serialized_rows.segment_rows(segment) {
            Some(rows) => rows.len(),
            None => 0,
        })
        .sum()
}

struct ComposerSurface {
    activity_rows: Vec<Line<'static>>,
    hint_rows: Vec<Line<'static>>,
    btw_rows: Vec<Line<'static>>,
    /// The session name rule, drawn as the editor's top row.
    rule: Option<Line<'static>>,
    editor: ComposerEditor,
    footer_rows: Vec<Line<'static>>,
}

impl ComposerSurface {
    fn total_len(&self) -> usize {
        self.activity_rows
            .len()
            .saturating_add(self.hint_rows.len())
            .saturating_add(self.btw_rows.len())
            .saturating_add(self.editor_len())
            .saturating_add(self.footer_rows.len())
    }

    /// Rows the editor slot asks for: the editor plus the rule above it.
    fn editor_len(&self) -> usize {
        self.editor.total_len().saturating_add(usize::from(self.rule.is_some()))
    }

    fn editor_len_u16(&self) -> u16 {
        u16::try_from(self.editor_len()).unwrap_or(u16::MAX)
    }

    fn editor_visible_rows(&self, height: u16) -> &[Line<'static>] {
        self.editor.visible_rows(height)
    }

    fn preview_rows(
        &self,
        activity_rows: &[Line<'static>],
        hint_rows: &[Line<'static>],
        btw_rows: &[Line<'static>],
        editor_rows: &[Line<'static>],
        footer_rows: &[Line<'static>],
    ) -> Vec<Line<'static>> {
        let editor_preview = match &self.editor {
            ComposerEditor::TextArea { desired_height } => vec![Line::from(Span::styled(
                format!("<textarea widget rows={desired_height}>"),
                Style::default().fg(theme::DIM),
            ))],
            ComposerEditor::Rows(_) => editor_rows.to_vec(),
        };

        activity_rows
            .iter()
            .chain(hint_rows.iter())
            .chain(btw_rows.iter())
            .chain(self.rule.iter())
            .chain(editor_preview.iter())
            .chain(footer_rows.iter())
            .cloned()
            .collect::<Vec<_>>()
    }
}

enum ComposerEditor {
    TextArea { desired_height: u16 },
    Rows(Vec<Line<'static>>),
}

impl ComposerEditor {
    fn total_len(&self) -> usize {
        match self {
            Self::TextArea { desired_height } => usize::from(*desired_height),
            Self::Rows(rows) => rows.len(),
        }
    }

    fn total_len_u16(&self) -> u16 {
        u16::try_from(self.total_len()).unwrap_or(u16::MAX)
    }

    fn visible_rows(&self, height: u16) -> &[Line<'static>] {
        match self {
            Self::TextArea { .. } => &[],
            Self::Rows(rows) => RowWindow::tail(rows.len(), height).slice(rows),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowWindow {
    start: usize,
    visible_len: usize,
}

impl RowWindow {
    fn tail(total_rows: usize, budget: u16) -> Self {
        let visible_len = total_rows.min(usize::from(budget));
        let start = total_rows.saturating_sub(visible_len);
        Self { start, visible_len }
    }

    fn hidden_rows_above(self) -> usize {
        self.start
    }

    fn visible_len_u16(self) -> u16 {
        u16::try_from(self.visible_len).unwrap_or(u16::MAX)
    }

    fn end(self) -> usize {
        self.start.saturating_add(self.visible_len)
    }

    fn slice<T>(self, rows: &[T]) -> &[T] {
        let start = self.start.min(rows.len());
        let end = self.end().min(rows.len());
        if start > end { &[] } else { &rows[start..end] }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MutableLayoutPlan {
    live_window: RowWindow,
    activity_window: RowWindow,
    hint_window: RowWindow,
    btw_window: RowWindow,
    editor_height: u16,
    footer_window: RowWindow,
    viewport_height: u16,
}

impl MutableLayoutPlan {
    fn new(live_rows: &[Line<'static>], composer: &ComposerSurface, screen_height: u16) -> Self {
        let screen_height = screen_height.max(1);
        let footer_window = RowWindow::tail(composer.footer_rows.len(), screen_height);
        let editor_budget = screen_height.saturating_sub(footer_window.visible_len_u16());
        let editor_height = composer.editor_len_u16().min(editor_budget);
        let btw_budget = editor_budget.saturating_sub(editor_height);
        // BTW rows are already ordered by status priority; keep the active row on short screens.
        let btw_window = RowWindow {
            start: 0,
            visible_len: composer.btw_rows.len().min(usize::from(btw_budget)),
        };
        let hint_budget = btw_budget.saturating_sub(btw_window.visible_len_u16());
        let hint_window = RowWindow::tail(composer.hint_rows.len(), hint_budget);
        let activity_budget = hint_budget.saturating_sub(hint_window.visible_len_u16());
        let activity_window = RowWindow {
            start: 0,
            visible_len: composer.activity_rows.len().min(usize::from(activity_budget)),
        };
        let live_budget = activity_budget.saturating_sub(activity_window.visible_len_u16());
        let live_window = RowWindow::tail(live_rows.len(), live_budget);
        let viewport_height = live_window
            .visible_len_u16()
            .saturating_add(activity_window.visible_len_u16())
            .saturating_add(hint_window.visible_len_u16())
            .saturating_add(btw_window.visible_len_u16())
            .saturating_add(editor_height)
            .saturating_add(footer_window.visible_len_u16())
            .max(1)
            .min(screen_height);

        Self {
            live_window,
            activity_window,
            hint_window,
            btw_window,
            editor_height,
            footer_window,
            viewport_height,
        }
    }

    fn live_visible_rows<'rows>(self, live_rows: &'rows [Line<'static>]) -> &'rows [Line<'static>] {
        self.live_window.slice(live_rows)
    }

    fn hint_visible_rows<'rows>(self, hint_rows: &'rows [Line<'static>]) -> &'rows [Line<'static>] {
        self.hint_window.slice(hint_rows)
    }

    fn btw_visible_rows<'rows>(self, btw_rows: &'rows [Line<'static>]) -> &'rows [Line<'static>] {
        self.btw_window.slice(btw_rows)
    }

    fn footer_visible_rows<'rows>(
        self,
        footer_rows: &'rows [Line<'static>],
    ) -> &'rows [Line<'static>] {
        self.footer_window.slice(footer_rows)
    }

    fn visible_composer_len(self) -> usize {
        usize::from(
            self.activity_window
                .visible_len_u16()
                .saturating_add(self.hint_window.visible_len_u16())
                .saturating_add(self.btw_window.visible_len_u16())
                .saturating_add(self.editor_height)
                .saturating_add(self.footer_window.visible_len_u16()),
        )
    }

    fn areas(self, viewport_area: Rect) -> (Rect, Rect, Rect, Rect, Rect, Rect) {
        let mut budget = viewport_area.height;
        let footer_height = self.footer_window.visible_len_u16().min(budget);
        budget = budget.saturating_sub(footer_height);
        let editor_height = self.editor_height.min(budget);
        budget = budget.saturating_sub(editor_height);
        let btw_height = self.btw_window.visible_len_u16().min(budget);
        budget = budget.saturating_sub(btw_height);
        let hint_height = self.hint_window.visible_len_u16().min(budget);
        budget = budget.saturating_sub(hint_height);
        let activity_height = self.activity_window.visible_len_u16().min(budget);
        let live_height = budget.saturating_sub(activity_height);
        let mut y = viewport_area.y;
        let mut area = |height| {
            let rect = Rect::new(viewport_area.x, y, viewport_area.width, height);
            y = y.saturating_add(height);
            rect
        };
        (
            area(live_height),
            area(activity_height),
            area(hint_height),
            area(btw_height),
            area(editor_height),
            area(footer_height),
        )
    }
}

fn render_composer_editor(
    frame: &mut ratatui::Frame<'_>,
    app: &mut App,
    composer: &ComposerSurface,
    area: Rect,
) {
    let (rule_area, area) = split_rule_row(composer.rule.is_some(), area);
    if let Some(rule) = composer.rule.as_ref()
        && !rule_area.is_empty()
    {
        frame.render_widget(Paragraph::new(rule.clone()), rule_area);
    }
    match &composer.editor {
        ComposerEditor::TextArea { .. } => render_textarea_editor(frame, app, area),
        ComposerEditor::Rows(rows) => {
            let visible_rows = RowWindow::tail(rows.len(), area.height).slice(rows).to_vec();
            frame.render_widget(Paragraph::new(visible_rows), area);
        }
    }
}

/// The rule takes the editor area's top row, and is the first thing given up
/// when the screen leaves the editor a single row.
fn split_rule_row(has_rule: bool, area: Rect) -> (Rect, Rect) {
    if !has_rule || area.height < 2 {
        return (Rect::new(area.x, area.y, area.width, 0), area);
    }
    let rule = Rect::new(area.x, area.y, area.width, 1);
    let editor = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    (rule, editor)
}

fn render_textarea_editor(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let geometry = input::compute_render_geometry(area, 0);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(theme::USER_MSG_BG)),
        geometry.field,
    );
    if !geometry.prompt.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                input::prompt_prefix_text(),
                Style::default().fg(theme::RUST_ORANGE),
            ))),
            geometry.prompt,
        );
    }

    if geometry.text.is_empty() {
        return;
    }

    input::configure_input_textarea(app);
    frame.render_widget(app.input.editor(), geometry.text);
    if input::should_show_native_textarea_cursor(app)
        && let Some(position) = app.input.editor().rendered_cursor_position()
    {
        frame.set_cursor_position(position);
    }
}

struct InlineViewportDrawMetrics {
    viewport_area: Rect,
    live_area: Rect,
    activity_area: Rect,
    hint_area: Rect,
    btw_area: Rect,
    editor_area: Rect,
    footer_area: Rect,
    requested_inline_height: u16,
    terminal_width: u16,
    terminal_height: u16,
    live_rows_total: usize,
    live_rows_mutable: usize,
    live_rows_visible: usize,
    live_rows_hidden_above: usize,
    composer_rows_total: usize,
    composer_rows_visible: usize,
    scrollback_inserted_rows: usize,
}

fn log_inline_viewport_draw(metrics: &InlineViewportDrawMetrics) {
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_viewport_draw",
        message = "ratatui inline viewport repainted with mutable chat rows",
        outcome = "success",
        viewport_top = metrics.viewport_area.top(),
        viewport_height = metrics.viewport_area.height,
        live_top = metrics.live_area.top(),
        live_height = metrics.live_area.height,
        composer_top = metrics
            .activity_area
            .top()
            .min(metrics.hint_area.top())
            .min(metrics.btw_area.top())
            .min(metrics.editor_area.top())
            .min(metrics.footer_area.top()),
        composer_height = metrics
            .activity_area
            .height
            .saturating_add(metrics.hint_area.height)
            .saturating_add(metrics.btw_area.height)
            .saturating_add(metrics.editor_area.height)
            .saturating_add(metrics.footer_area.height),
        activity_top = metrics.activity_area.top(),
        activity_height = metrics.activity_area.height,
        hint_top = metrics.hint_area.top(),
        hint_height = metrics.hint_area.height,
        btw_top = metrics.btw_area.top(),
        btw_height = metrics.btw_area.height,
        editor_top = metrics.editor_area.top(),
        editor_height = metrics.editor_area.height,
        footer_top = metrics.footer_area.top(),
        footer_height = metrics.footer_area.height,
        requested_inline_height = metrics.requested_inline_height,
        terminal_width = metrics.terminal_width,
        terminal_height = metrics.terminal_height,
        mutable_rows = metrics.live_rows_visible + metrics.composer_rows_visible,
        live_rows_total = metrics.live_rows_total,
        live_rows_mutable = metrics.live_rows_mutable,
        live_rows_visible = metrics.live_rows_visible,
        live_rows_hidden_above = metrics.live_rows_hidden_above,
        scrollback_inserted_rows = metrics.scrollback_inserted_rows,
        composer_rows_total = metrics.composer_rows_total,
        composer_rows_visible = metrics.composer_rows_visible,
    );
}

struct InlineChatDrawSummary<'a> {
    app: &'a App,
    live_rows_total: &'a [Line<'static>],
    live_rows_visible: &'a [Line<'static>],
    live_rows_mutable: usize,
    composer_rows_total: usize,
    composer_rows_visible: usize,
    composer_preview: String,
    live_rows_hidden_above: usize,
    history_queued_rows: usize,
    history_excluded_rows: usize,
    history_excluded_ids: usize,
    history_full_rebuild: bool,
    stable_rows: usize,
    first_mutable_boundary_start: Option<usize>,
    first_mutable_boundary_kind: Option<LiveRowBoundaryKind>,
    first_mutable_boundary_msg_idx: Option<usize>,
    first_mutable_boundary_block_idx: Option<usize>,
}

fn log_inline_chat_draw(summary: &InlineChatDrawSummary<'_>) {
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_draw_summary",
        message = "inline chat draw payload prepared",
        outcome = "prepared",
        status = ?summary.app.status,
        mode = summary.app.session_runtime.mode.as_ref().map_or_else(|| "none".to_owned(), |mode| mode.current_mode_name.clone()),
        terminal_width = summary.app.chat_render.terminal_width,
        terminal_height = summary.app.chat_render.terminal_height,
        anchor_valid = summary.app.chat_render.live_region.anchor_valid,
        live_rows_total = summary.live_rows_total.len(),
        live_rows_mutable = summary.live_rows_mutable,
        live_rows_visible = summary.live_rows_visible.len(),
        live_rows_hidden_above = summary.live_rows_hidden_above,
        stable_rows = summary.stable_rows,
        first_mutable_boundary_start = ?summary.first_mutable_boundary_start,
        first_mutable_boundary_kind = ?summary.first_mutable_boundary_kind,
        first_mutable_boundary_msg_idx = ?summary.first_mutable_boundary_msg_idx,
        first_mutable_boundary_block_idx = ?summary.first_mutable_boundary_block_idx,
        history_queued_rows = summary.history_queued_rows,
        history_excluded_rows = summary.history_excluded_rows,
        history_excluded_ids = summary.history_excluded_ids,
        history_full_rebuild = summary.history_full_rebuild,
        composer_rows_total = summary.composer_rows_total,
        composer_rows_visible = summary.composer_rows_visible,
        live_preview = %preview_rows(summary.live_rows_visible, 3),
        composer_preview = %summary.composer_preview,
    );
}

fn preview_rows(rows: &[Line<'static>], limit: usize) -> String {
    rows.iter()
        .take(limit)
        .enumerate()
        .map(|(idx, row)| {
            let text = row.spans.iter().map(|span| span.content.as_ref()).collect::<String>();
            format!("[{idx}] {text}")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::{
        ChatTerminalSession, ComposerEditor, ComposerSurface, HistoryCommitState,
        MutableLayoutPlan, RowWindow,
    };
    use crate::app::terminal_runtime::chat_terminal::ChatTerminal;
    use crate::app::terminal_runtime::chat_terminal::plan_inline_geometry;
    use crate::app::{
        App, AppStatus, ChatMessage, ChatMessageId, FocusTarget, HistoryOutputId, MessageBlock,
        MessageRole, TextBlock, TextBlockSpacing,
    };
    use crate::ui::inline_chat_rows::{
        LiveRowBoundaryKind, LiveRowSegment, SerializedLiveRows,
        serialize_live_rows_with_boundaries_excluding,
    };
    use crate::ui::theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use std::collections::BTreeSet;

    fn rows(count: usize) -> Vec<Line<'static>> {
        (0..count).map(|idx| Line::from(format!("row {idx}"))).collect()
    }

    fn textarea_composer(
        hint_rows: usize,
        editor_height: u16,
        footer_rows: usize,
    ) -> ComposerSurface {
        ComposerSurface {
            activity_rows: Vec::new(),
            hint_rows: rows(hint_rows),
            btw_rows: Vec::new(),
            rule: None,
            editor: ComposerEditor::TextArea { desired_height: editor_height },
            footer_rows: rows(footer_rows),
        }
    }

    fn output_id() -> HistoryOutputId {
        HistoryOutputId::AssistantLabel(ChatMessageId::new())
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect::<String>().trim_end().into()
    }

    fn live_segment(start_row: usize, end_row: usize, id: HistoryOutputId) -> LiveRowSegment {
        LiveRowSegment {
            ids: vec![id],
            msg_idx: 0,
            block_idx: None,
            kind: LiveRowBoundaryKind::AssistantLabel,
            start_row,
            end_row,
            commit_ready: true,
        }
    }

    fn assistant_blocks_message(blocks: Vec<MessageBlock>) -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, blocks, None)
    }

    fn session_with_history(history: HistoryCommitState) -> ChatTerminalSession {
        ChatTerminalSession { terminal: ChatTerminal::new(0), history }
    }

    fn render_textarea_to_test_backend(app: &mut App, area: Rect) -> TestBackend {
        let backend = TestBackend::new(area.width.max(1), area.height.max(1));
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| {
                super::render_textarea_editor(frame, app, area);
            })
            .expect("test draw should succeed");
        terminal.backend().clone()
    }

    fn render_composer_editor_to_test_backend(app: &mut App, width: u16) -> TestBackend {
        let composer = ChatTerminalSession::build_composer_surface(app, width);
        let height = composer.editor_len_u16();
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| {
                super::render_composer_editor(
                    frame,
                    app,
                    &composer,
                    Rect::new(0, 0, width, height),
                );
            })
            .expect("test draw should succeed");
        terminal.backend().clone()
    }

    fn buffer_row(backend: &TestBackend, y: u16) -> String {
        let buffer = backend.buffer();
        (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect()
    }

    #[test]
    fn named_session_draws_its_rule_directly_above_the_editor() {
        let mut app = App::test_default();
        app.input.set_text("hello");
        let _ = app.input.set_cursor(0, 5);
        app.session_runtime.session_title = Some("probe-e2e".to_owned());

        let backend = render_composer_editor_to_test_backend(&mut app, 30);

        assert_eq!(buffer_row(&backend, 0), format!("{} probe-e2e ─", "─".repeat(18)));
        let buffer = backend.buffer();
        assert_eq!(buffer[(0, 0)].style().fg, Some(theme::DIM));
        // The name is plain text on the terminal's own background.
        for x in 18..29 {
            assert_eq!(buffer[(x, 0)].style().bg, Some(ratatui::style::Color::Reset), "cell {x}");
        }
        // The editor field (padded top and bottom) follows directly below.
        assert_eq!(buffer[(0, 1)].style().bg, Some(theme::USER_MSG_BG));
        assert!(buffer_row(&backend, 2).contains("hello"));
        assert_eq!(backend.cursor_position().y, 2, "the caret moves below the rule");
    }

    #[test]
    fn unnamed_session_keeps_the_editor_on_the_first_row() {
        let mut app = App::test_default();
        app.input.set_text("hello");

        let backend = render_composer_editor_to_test_backend(&mut app, 30);

        assert_eq!(backend.buffer()[(0, 0)].style().bg, Some(theme::USER_MSG_BG));
        assert!(buffer_row(&backend, 1).contains("hello"));
        assert!((0..backend.buffer().area.height).all(|y| !buffer_row(&backend, y).contains('─')));
    }

    #[test]
    fn a_one_row_editor_slot_gives_up_the_rule_before_the_editor() {
        let full = Rect::new(0, 4, 30, 1);
        assert_eq!(super::split_rule_row(true, full), (Rect::new(0, 4, 30, 0), full));
        assert_eq!(
            super::split_rule_row(true, Rect::new(0, 4, 30, 3)),
            (Rect::new(0, 4, 30, 1), Rect::new(0, 5, 30, 2))
        );
    }

    #[test]
    fn render_textarea_editor_sets_native_cursor_when_input_has_focus() {
        let mut app = App::test_default();
        app.input.set_text("hello");
        let _ = app.input.set_cursor(0, 5);

        let backend = render_textarea_to_test_backend(&mut app, Rect::new(0, 0, 20, 3));

        assert!(backend.cursor_visible());
        assert_eq!(backend.cursor_position(), Position { x: 8, y: 1 });
    }

    #[test]
    fn render_textarea_editor_hides_native_cursor_when_permission_has_focus() {
        let mut app = App::test_default();
        app.input.set_text("hello");
        let _ = app.input.set_cursor(0, 5);
        app.pending_interaction_ids.push("perm-1".to_owned());
        app.claim_focus_target(FocusTarget::Permission);

        let backend = render_textarea_to_test_backend(&mut app, Rect::new(0, 0, 20, 3));

        assert!(!backend.cursor_visible());
    }

    #[test]
    fn render_textarea_editor_hides_native_cursor_when_text_area_is_empty() {
        let mut app = App::test_default();
        app.input.set_text("hello");
        let _ = app.input.set_cursor(0, 5);

        let backend = render_textarea_to_test_backend(&mut app, Rect::new(0, 0, 4, 1));

        assert!(!backend.cursor_visible());
    }

    #[test]
    fn render_textarea_editor_uses_user_message_background_for_empty_input() {
        let mut app = App::test_default();

        let backend = render_textarea_to_test_backend(&mut app, Rect::new(0, 0, 20, 3));
        let buffer = backend.buffer();

        for y in 0..3 {
            for x in 0..20 {
                assert_eq!(buffer[(x, y)].style().bg, Some(theme::USER_MSG_BG));
            }
        }
    }

    #[test]
    fn render_textarea_editor_uses_user_message_background_for_populated_input() {
        let mut app = App::test_default();
        app.input.set_text("hello");

        let backend = render_textarea_to_test_backend(&mut app, Rect::new(0, 0, 20, 3));
        let buffer = backend.buffer();

        for y in 0..3 {
            for x in 0..20 {
                assert_eq!(buffer[(x, y)].style().bg, Some(theme::USER_MSG_BG));
            }
        }
    }

    #[test]
    fn composer_places_btw_status_after_the_complete_queued_message_field() {
        let mut app = App::test_default();
        app.pending_user_messages
            .try_push_sending(crate::app::PendingUserMessage::sending(
                "message-1".to_owned(),
                "queued prompt".to_owned(),
                Vec::new(),
            ))
            .expect("queue prompt");
        app.btw
            .try_push("btw-1".to_owned(), "side question".to_owned())
            .expect("queue side question");

        let surface = ChatTerminalSession::build_composer_surface(&mut app, 80);
        let hint_text = surface.hint_rows.iter().map(line_text).collect::<Vec<_>>();
        let btw_text = surface.btw_rows.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(hint_text.len(), 2);
        assert!(hint_text[0].starts_with("Queued Messages (1)"));
        assert!(hint_text[1].contains("1. queued prompt"));
        assert_eq!(btw_text.len(), 1);
        assert!(btw_text[0].contains("BTW  side question"));
        assert_eq!(app.chat_render.composer.hint_rows, 2);
        assert_eq!(app.chat_render.composer.btw_rows, 1);
        assert_eq!(
            app.chat_render.composer.total_rows,
            app.chat_render
                .composer
                .hint_rows
                .saturating_add(app.chat_render.composer.btw_rows)
                .saturating_add(app.chat_render.composer.editor_rows)
                .saturating_add(app.chat_render.composer.footer_rows)
        );
    }

    #[test]
    fn history_commit_state_confirms_output_ids_once() {
        let mut state = HistoryCommitState::default();
        let first = output_id();
        let second = output_id();

        state.confirm(vec![first.clone()]);
        state.confirm(vec![first.clone(), second.clone()]);

        assert_eq!(state.confirmed_len(), 2);
        assert!(state.confirmed_ids().contains(&first));
        assert!(state.confirmed_ids().contains(&second));
        assert!(state.is_synced());
    }

    #[test]
    fn purge_replay_marks_history_unsynced_and_clears_confirmed_ids() {
        let mut state = HistoryCommitState::default();
        state.confirm(vec![output_id()]);

        state.reset_for_purge_replay();

        assert!(!state.is_synced());
        assert_eq!(state.confirmed_len(), 0);
    }

    #[test]
    fn replay_completion_marks_history_synced_and_confirms_output() {
        let mut state = HistoryCommitState::default();
        let id = output_id();

        state.reset_for_purge_replay();
        state.confirm(vec![id.clone()]);

        assert!(state.is_synced());
        assert!(state.confirmed_ids().contains(&id));
    }

    #[test]
    fn incomplete_replay_requests_follow_up_repaint() {
        let mut app = App::test_default();
        let mut session = session_with_history(HistoryCommitState::default());

        app.surface_dirty.chat.take_repaint();
        session.complete_history_flush(
            &mut app,
            &super::super::chat_terminal::ChatDrawOutcome {
                viewport_area: Rect::new(0, 27, 120, 3),
                flushed_history: super::super::chat_terminal::FlushedHistory {
                    flushed_rows: 160,
                    replay_incomplete: true,
                    ..Default::default()
                },
            },
        );

        assert!(app.surface_dirty.chat.repaint);
    }

    #[test]
    fn replay_preserves_all_retained_segments_in_transcripts_over_nine_thousand_rows() {
        let first = output_id();
        let second = output_id();
        let serialized = SerializedLiveRows::from_parts_for_test(
            rows(9_100),
            vec![live_segment(0, 4_550, first.clone()), live_segment(4_550, 9_100, second.clone())],
        );

        let replay = super::build_replay_history_batches(&serialized, 80);
        let text: Vec<_> = replay
            .batches
            .iter()
            .flat_map(|batch| batch.rows.slice(0..batch.rows.len()))
            .map(line_text)
            .collect();

        assert_eq!(text, (0..9_100).map(|index| format!("row {index}")).collect::<Vec<_>>());
        assert_eq!(replay.rows, 9_100);
        assert_eq!(replay.confirm_ids, vec![first, second]);
    }

    #[test]
    fn thinking_and_activity_stay_out_of_history_insertion_and_resize_replay() {
        let mut app = App::test_default();
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![
                MessageBlock::Text(TextBlock::from_complete("completed prefix")),
                MessageBlock::Text(TextBlock::from_complete("live tail")),
            ],
            None,
        ));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Thinking;
        app.begin_turn_activity(std::time::Instant::now());
        for width in [32, 120, 32] {
            let serialized =
                serialize_live_rows_with_boundaries_excluding(&mut app, width, &BTreeSet::new());
            assert!(serialized.rows().iter().any(|row| line_text(row).contains("Thinking…")));
            let inserted =
                super::build_static_history_batches(&serialized, width, &BTreeSet::new());
            let replay = super::build_replay_history_batches(&serialized, width);
            for batches in [&inserted, &replay.batches] {
                let text: Vec<_> = batches
                    .iter()
                    .flat_map(|batch| batch.rows.slice(0..batch.rows.len()))
                    .map(line_text)
                    .collect();
                assert_eq!(text, ["Claude", "completed prefix"]);
                assert!(
                    !batches
                        .iter()
                        .flat_map(|batch| &batch.confirm_ids)
                        .any(|id| { matches!(id, HistoryOutputId::AssistantThinking(_)) })
                );
            }
        }
    }

    #[test]
    fn static_history_batches_do_not_reinsert_confirmed_text_blocks() {
        let first = TextBlock::from_complete("first paragraph\n\n")
            .with_trailing_spacing(TextBlockSpacing::ParagraphBreak);
        let first_id = first.id;
        let second = TextBlock::from_complete("second paragraph");
        let second_id = second.id;
        let message = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(first), MessageBlock::Text(second)],
            None,
        );
        let message_id = message.id;
        let mut app = App::test_default();
        app.transcript.messages.push(message);

        let serialized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 120, &BTreeSet::new());
        let excluded_ids = BTreeSet::from([
            HistoryOutputId::AssistantLabel(message_id),
            HistoryOutputId::Block(first_id),
        ]);
        let batches = super::build_static_history_batches(&serialized, 120, &excluded_ids);
        let inserted_text = batches
            .iter()
            .flat_map(|batch| batch.rows.slice(0..batch.rows.len()))
            .map(line_text)
            .collect::<Vec<_>>();

        assert_eq!(batches.iter().flat_map(|batch| batch.confirm_ids.iter()).count(), 1);
        assert!(
            batches
                .iter()
                .any(|batch| batch.confirm_ids.contains(&HistoryOutputId::Block(second_id)))
        );
        assert_eq!(inserted_text, vec!["second paragraph"]);
    }

    #[test]
    fn btw_card_commits_during_active_turn_and_survives_streaming_and_resize_replay() {
        use crate::agent::{events::ClientEvent, model};
        use crate::app::BtwExchangeBlock;

        let exchange =
            BtwExchangeBlock::new("Side question?".to_owned(), "Side answer.".to_owned());
        let exchange_id = HistoryOutputId::Block(exchange.id);
        let mut app = App::test_default();
        app.session_runtime.session_id = Some(model::SessionId::new("test-session"));
        app.push_message_tracked(assistant_blocks_message(vec![
            MessageBlock::Text(TextBlock::new("Before the card.".to_owned())),
            MessageBlock::BtwExchange(exchange),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
        let batches = super::build_static_history_batches(&serialized, 80, &BTreeSet::new());
        let committed_rows: Vec<_> = batches
            .iter()
            .flat_map(|batch| batch.rows.slice(0..batch.rows.len()).iter().cloned())
            .collect();
        let mut history = HistoryCommitState::default();
        for batch in batches {
            history.confirm(batch.confirm_ids);
        }
        assert!(
            history.confirmed_ids().contains(&exchange_id),
            "completed card must enter actual history batches, even at the active tail"
        );
        assert!(committed_rows.iter().any(|line| line_text(line).contains("Claude · BTW")));

        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: "test-session".to_owned(),
                update: model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                    model::ContentBlock::Text(model::TextContent::new("After the card.")),
                )),
            },
        );
        let serialized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, &BTreeSet::new());
        assert!(
            super::build_static_history_batches(&serialized, 80, history.confirmed_ids())
                .is_empty(),
            "only the new streaming tail remains; confirmed cards must not be queued again"
        );
        let live =
            serialize_live_rows_with_boundaries_excluding(&mut app, 80, history.confirmed_ids());
        assert!(live.rows().iter().any(|line| line_text(line) == "After the card."));
        assert!(!live.rows().iter().any(|line| line_text(line).contains("Claude · BTW")));
        let combined: Vec<_> =
            committed_rows.into_iter().chain(live.rows().iter().cloned()).collect();
        assert_eq!(combined, serialized.rows());

        history.reset_for_purge_replay();
        let resized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 24, history.confirmed_ids());
        let replay = super::build_replay_history_batches(&resized, 24);
        assert!(replay.confirm_ids.contains(&exchange_id));
        let replay_text: Vec<_> = replay
            .batches
            .iter()
            .flat_map(|batch| batch.rows.slice(0..batch.rows.len()))
            .map(line_text)
            .collect();
        assert_eq!(replay_text.iter().filter(|line| line.contains("Claude · BTW")).count(), 1);
        assert!(!replay_text.iter().any(|line| line == "After the card."));
        history.confirm(replay.confirm_ids);

        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: "test-session".to_owned(),
                queued_turn_count: Some(0),
                terminal_reason: None,
            },
        );
        let completed =
            serialize_live_rows_with_boundaries_excluding(&mut app, 24, &BTreeSet::new());
        let final_batches =
            super::build_static_history_batches(&completed, 24, history.confirmed_ids());
        let final_rows: Vec<_> = final_batches
            .iter()
            .flat_map(|batch| batch.rows.slice(0..batch.rows.len()))
            .map(line_text)
            .collect();
        assert!(final_rows.iter().any(|line| line == "After the card."));
        assert!(!final_rows.iter().any(|line| line.contains("Claude · BTW")));
    }

    #[test]
    fn static_history_batches_preserve_rendered_table_prefix_before_mutable_tail() {
        let table = concat!(
            "| Hassle | What users report | Refs |\n",
            "| --- | --- | --- |\n",
            "| Input lag | Keystrokes echo late as context fills | #18943 |\n",
            "| Paste freeze | Pasting many lines froze the terminal |  |\n",
            "| Scrollback destroyed |  | #42002 |\n",
        );
        let mut app = App::test_default();
        app.transcript.messages.push(assistant_blocks_message(vec![
            MessageBlock::Text(TextBlock::from_complete(table)),
            MessageBlock::Text(TextBlock::from_complete("streaming tail remains mutable")),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;

        let serialized =
            serialize_live_rows_with_boundaries_excluding(&mut app, 180, &BTreeSet::new());
        let batches = super::build_static_history_batches(&serialized, 180, &BTreeSet::new());
        let inserted_text = batches
            .iter()
            .flat_map(|batch| batch.rows.slice(0..batch.rows.len()))
            .map(line_text)
            .collect::<Vec<_>>();
        let excluded_ids = batches
            .iter()
            .flat_map(|batch| batch.confirm_ids.iter().cloned())
            .collect::<BTreeSet<_>>();
        let remaining_live_text =
            serialized.rows_excluding_ids(&excluded_ids).iter().map(line_text).collect::<Vec<_>>();

        assert!(inserted_text.iter().any(|line| line == "Claude"));
        assert!(inserted_text.iter().any(|line| line.contains("Paste freeze")));
        assert!(inserted_text.iter().any(|line| line.contains("Scrollback destroyed")));
        assert!(!inserted_text.iter().any(|line| line.trim_start().starts_with('|')));
        assert_eq!(remaining_live_text, vec!["streaming tail remains mutable"]);
    }

    #[test]
    fn history_batches_skip_invalid_live_row_segments() {
        let invalid_id = output_id();
        let serialized = SerializedLiveRows::from_parts_for_test(
            rows(2),
            vec![live_segment(0, 3, invalid_id.clone())],
        );

        let static_batches =
            super::build_static_history_batches(&serialized, 120, &BTreeSet::new());
        let replay = super::build_replay_history_batches(&serialized, 120);
        let excluded_rows = super::excluded_row_count(&serialized, &BTreeSet::from([invalid_id]));

        assert!(static_batches.is_empty());
        assert!(replay.batches.is_empty());
        assert!(replay.confirm_ids.is_empty());
        assert_eq!(replay.rows, 0);
        assert_eq!(excluded_rows, 0);
    }

    #[test]
    fn row_window_slice_returns_empty_for_stale_ranges() {
        let rows = rows(2);
        let stale = RowWindow { start: 4, visible_len: 2 };

        assert!(stale.slice(&rows).is_empty());
    }

    #[test]
    fn fullscreen_reattach_preserves_history_commit_state() {
        let mut state = HistoryCommitState::default();
        state.confirm(vec![output_id()]);
        let mut session = session_with_history(state.clone());
        let mut app = App::test_default();
        app.chat_render.live_region.anchor_valid = true;

        session.reattach_after_fullscreen(&mut app);

        assert_eq!(session.history, state);
        assert!(!app.chat_render.live_region.anchor_valid);
    }

    #[test]
    fn mutable_viewport_clear_preserves_history_commit_state() {
        let mut state = HistoryCommitState::default();
        state.confirm(vec![output_id()]);
        let mut session = session_with_history(state.clone());
        let mut app = App::test_default();
        app.chat_render.live_region.anchor_valid = true;

        session.clear_mutable_viewport(&mut app);

        assert_eq!(session.history, state);
        assert!(!app.chat_render.live_region.anchor_valid);
    }

    #[test]
    fn mutable_layout_prefers_footer_and_editor_when_height_is_tight() {
        let live_rows = rows(4);
        let composer = textarea_composer(2, 3, 2);

        let plan = MutableLayoutPlan::new(&live_rows, &composer, 4);
        let (live_area, _activity_area, hint_area, btw_area, editor_area, footer_area) =
            plan.areas(Rect::new(0, 0, 80, 4));

        assert_eq!(footer_area.height, 2);
        assert_eq!(editor_area.height, 2);
        assert_eq!(hint_area.height, 0);
        assert_eq!(btw_area.height, 0);
        assert_eq!(live_area.height, 0);
        assert_eq!(plan.viewport_height, 4);
    }

    #[test]
    fn activity_and_tip_stay_adjacent_above_queue_and_editor_without_spacer() {
        let mut composer = textarea_composer(2, 2, 1);
        composer.activity_rows = vec![Line::from("│ Working…"), Line::from("│ Tip: example")];
        let live = rows(4);
        let full = MutableLayoutPlan::new(&live, &composer, 20);
        let (live_area, activity_area, hints, btw, editor, footer) =
            full.areas(Rect::new(0, 5, 80, full.viewport_height));
        assert_eq!(activity_area.y, live_area.bottom());
        assert_eq!(hints.y, activity_area.bottom());
        assert_eq!(hints.height, 2);
        assert_eq!(editor.y, btw.bottom());
        assert_eq!(footer.y, editor.bottom());
        assert_eq!(composer.total_len(), full.visible_composer_len());
        for height in 3..=8 {
            let plan = MutableLayoutPlan::new(&live, &composer, height);
            let (_, activity, hints, _, editor, footer) = plan.areas(Rect::new(0, 0, 80, height));
            assert_eq!(footer.height, 1);
            assert_eq!(editor.height, 2);
            assert_eq!(hints.height, (height - 3).min(2));
            assert_eq!(activity.height, height.saturating_sub(5).min(2));
            if activity.height > 0 {
                assert_eq!(
                    plan.activity_window.slice(&composer.activity_rows)[0].to_string(),
                    "│ Working…"
                );
            }
            if activity.height > 1 {
                assert_eq!(
                    plan.activity_window.slice(&composer.activity_rows)[1].to_string(),
                    "│ Tip: example"
                );
            }
        }
    }

    #[test]
    fn mutable_layout_uses_remaining_height_for_hints_then_live_rows() {
        let live_rows = rows(4);
        let composer = textarea_composer(2, 1, 1);

        let plan = MutableLayoutPlan::new(&live_rows, &composer, 5);
        let (live_area, _activity_area, hint_area, btw_area, editor_area, footer_area) =
            plan.areas(Rect::new(0, 0, 80, 5));

        assert_eq!(footer_area.height, 1);
        assert_eq!(editor_area.height, 1);
        assert_eq!(hint_area.height, 2);
        assert_eq!(btw_area.height, 0);
        assert_eq!(live_area.height, 1);
        assert_eq!(plan.viewport_height, 5);
    }

    #[test]
    fn mutable_layout_keeps_btw_surface_between_hints_and_editor() {
        let live_rows = rows(1);
        let composer = ComposerSurface {
            activity_rows: Vec::new(),
            hint_rows: rows(2),
            btw_rows: rows(1),
            rule: None,
            editor: ComposerEditor::TextArea { desired_height: 1 },
            footer_rows: rows(1),
        };

        let plan = MutableLayoutPlan::new(&live_rows, &composer, 6);
        let (live_area, _activity_area, hint_area, btw_area, editor_area, footer_area) =
            plan.areas(Rect::new(0, 0, 80, 6));

        assert_eq!(live_area, Rect::new(0, 0, 80, 1));
        assert_eq!(hint_area, Rect::new(0, 1, 80, 2));
        assert_eq!(btw_area, Rect::new(0, 3, 80, 1));
        assert_eq!(editor_area, Rect::new(0, 4, 80, 1));
        assert_eq!(footer_area, Rect::new(0, 5, 80, 1));
    }

    #[test]
    fn mutable_layout_preserves_btw_priority_when_only_part_of_the_status_field_fits() {
        let composer = ComposerSurface {
            activity_rows: Vec::new(),
            hint_rows: rows(2),
            btw_rows: vec![
                Line::from("active"),
                Line::from("failed"),
                Line::from("waiting"),
                Line::from("overflow"),
            ],
            rule: None,
            editor: ComposerEditor::TextArea { desired_height: 1 },
            footer_rows: rows(1),
        };
        for screen_height in 3..=6 {
            let plan = MutableLayoutPlan::new(&[], &composer, screen_height);
            let visible = plan.btw_visible_rows(&composer.btw_rows);
            assert_eq!(visible, &composer.btw_rows[..usize::from(screen_height - 2)]);
            assert_eq!(line_text(&visible[0]), "active");
        }
    }

    #[test]
    fn resolved_geometry_keeps_required_live_rows_visible_near_terminal_bottom() {
        let live_rows = rows(3);
        let composer = textarea_composer(0, 1, 2);
        let requested_plan = MutableLayoutPlan::new(&live_rows, &composer, 40);
        let geometry_plan = plan_inline_geometry(
            Some(Rect::new(0, 37, 120, requested_plan.viewport_height)),
            requested_plan.viewport_height,
            120,
            40,
        );

        let resolved_plan = MutableLayoutPlan::new(&live_rows, &composer, geometry_plan.height);
        let (live_area, _, _, _, editor_area, footer_area) = resolved_plan
            .areas(geometry_plan.target_area.expect("geometry should resolve a viewport"));

        assert_eq!(requested_plan.viewport_height, 6);
        assert_eq!(geometry_plan.height, 6);
        assert_eq!(resolved_plan.live_visible_rows(&live_rows).len(), 3);
        assert_eq!(live_area.height, 3);
        assert_eq!(editor_area.height.saturating_add(footer_area.height), 3);
    }

    #[test]
    fn plan_for_existing_viewport_preserves_anchor() {
        let area = Rect::new(0, 20, 120, 8);

        let plan = plan_inline_geometry(Some(area), 8, 120, 40);

        assert_eq!(plan.target_area, Some(area));
    }

    #[test]
    fn plan_for_unchanged_geometry_without_insert_does_not_clear() {
        let area = Rect::new(0, 20, 120, 8);

        let plan = plan_inline_geometry(Some(area), 8, 120, 40);

        assert_eq!(plan.target_area, Some(area));
    }

    #[test]
    fn plan_for_composer_expansion_with_room_preserves_viewport_anchor() {
        let old_area = Rect::new(0, 10, 120, 3);

        let plan = plan_inline_geometry(Some(old_area), 4, 120, 40);

        assert_eq!(plan.target_area, Some(Rect::new(0, 10, 120, 4)));
    }

    #[test]
    fn plan_for_composer_expansion_at_bottom_preserves_required_height() {
        let old_area = Rect::new(0, 34, 120, 3);

        let plan = plan_inline_geometry(Some(old_area), 4, 120, 37);

        assert_eq!(plan.target_area, Some(Rect::new(0, 33, 120, 4)));
        assert_eq!(plan.old_area_after_scroll, Some(Rect::new(0, 33, 120, 3)));
        assert_eq!(plan.scroll_rows_before_resize, 1);
        assert_eq!(plan.height, 4);
    }

    #[test]
    fn plan_for_composer_shrink_preserves_viewport_anchor() {
        let old_area = Rect::new(0, 36, 120, 4);

        let plan = plan_inline_geometry(Some(old_area), 3, 120, 40);

        assert_eq!(plan.target_area, Some(Rect::new(0, 36, 120, 3)));
    }
}
