// SPDX-License-Identifier: Apache-2.0

use super::chat_terminal::{
    ChatDrawOutcome, ChatDrawRequest, ChatTerminal, HistoryBatchKind, NativeTerminalIo,
    PendingHistoryBatch, TerminalIo, TerminalResizeDuringDraw, TerminalSnapshot,
};
use super::history_insert::RenderedHistoryRows;
use crate::app::{App, ChatPurgeReplayOptions, HistoryOutputId};
use crate::ui::footer_rows::serialize_footer_rows;
use crate::ui::inline_chat_rows::LiveRowBoundaryKind;
use crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding;
use crate::ui::inline_chat_rows::{LiveRowsCursor, SerializedLiveRows};
use crate::ui::input;
use crate::ui::input_rows::{
    blocked_input_lines, build_btw_status_rows, wrapped_composer_hint_rows,
};
use crate::ui::layout_control::LayoutControl;
use crate::ui::theme;
use anyhow::Context;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

pub(super) struct ChatTerminalSession<I: TerminalIo = NativeTerminalIo> {
    terminal: ChatTerminal<I>,
    history: HistoryCommitState,
    preparation: Option<Box<ReplayPreparation>>,
}

struct ReplayPreparation {
    cursor: LiveRowsCursor,
    reading_rows: SerializedLiveRows,
    finished: bool,
    mutable_seen: bool,
    reading: bool,
    source_revision: u64,
}

impl ReplayPreparation {
    fn new(app: &App) -> Self {
        Self {
            cursor: LiveRowsCursor::new(Instant::now()),
            reading_rows: SerializedLiveRows::new(Vec::new(), Vec::new()),
            finished: false,
            mutable_seen: false,
            reading: app.chat_render.viewport.is_reading(),
            source_revision: app.transcript.topology_revision(),
        }
    }
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
    pub(super) fn read_before_input_reader(io: &impl super::RuntimeIo) -> anyhow::Result<Self> {
        let (terminal_width, terminal_height) =
            io.size().context("failed to read chat terminal size")?;
        let (cursor_x, cursor_y) =
            io.cursor_before_input_reader().context("failed to read chat terminal cursor")?;
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
        io: &impl super::RuntimeIo,
    ) -> anyhow::Result<Self> {
        let (terminal_width, terminal_height) =
            io.size().context("failed to read chat terminal size")?;
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
        io: &impl super::RuntimeIo,
    ) -> anyhow::Result<Self> {
        let (terminal_width, terminal_height) =
            io.size().context("failed to read chat terminal size")?;
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

        Self {
            terminal: ChatTerminal::new(owned_top),
            history: HistoryCommitState::default(),
            preparation: None,
        }
    }
}

impl<I: TerminalIo> ChatTerminalSession<I> {
    pub(super) fn has_pending_work(&self) -> bool {
        self.preparation.is_some() || self.terminal.has_pending_history_rows()
    }

    pub(super) fn layout_ready(&self) -> bool {
        self.preparation.is_some() && !self.terminal.has_pending_history_rows()
    }

    fn needs_draw(&self, app: &App, rows: &SerializedLiveRows) -> bool {
        app.surface_dirty.chat.repaint
            || !rows.rows().is_empty()
            || self.terminal.has_pending_history_rows()
            || self.preparation.as_ref().is_some_and(|preparation| preparation.finished)
    }

    pub(super) fn park_cursor_at_viewport_end(&self) -> anyhow::Result<()> {
        self.terminal.park_cursor_at_viewport_end()
    }

    pub(super) fn clear(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
        self.history.reset();
        self.preparation = None;
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
        self.preparation = Some(Box::new(ReplayPreparation::new(app)));
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

    pub(super) fn apply_rebuild(&mut self, app: &mut App) {
        use crate::app::ChatRebuildKind;
        match app.surface_dirty.chat.take_rebuild() {
            ChatRebuildKind::None => {}
            ChatRebuildKind::MutableViewport => self.clear_mutable_viewport(app),
            ChatRebuildKind::FullscreenReturn => self.reattach_after_fullscreen(app),
            ChatRebuildKind::VisibleScreen => self.clear(app),
            ChatRebuildKind::PurgeReplay(options) => self.clear_for_purge_replay(app, options),
        }
    }

    pub(super) fn update(&mut self, app: &mut App) -> anyhow::Result<()> {
        self.update_with_budget(app, None)
    }

    fn update_with_budget(
        &mut self,
        app: &mut App,
        budget: Option<Duration>,
    ) -> anyhow::Result<()> {
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
        let snapshot = TerminalSnapshot::new(width, terminal_height);
        let observed_resize = Cell::new(None);
        let last_check = Cell::new(None::<Instant>);
        let io = self.terminal.io().clone();
        let check_size = || {
            // Check inside parsing, highlighting and wrapping without a system call per cell.
            if last_check.get().is_some_and(|at| at.elapsed() < Duration::from_millis(1)) {
                return false;
            }
            last_check.set(Some(Instant::now()));
            if let Ok(actual) = io.snapshot()
                && actual != snapshot
            {
                observed_resize.set(Some(actual));
                return true;
            }
            false
        };
        let control = budget.map_or_else(
            || LayoutControl::new(&check_size),
            |budget| LayoutControl::with_budget(&check_size, budget),
        );
        let Some(serialized_rows) =
            self.prepare_rows(app, width, &serialization_excluded_ids, &control)
        else {
            return Ok(());
        };
        if control.cancelled() {
            self.preparation = None;
            self.history.mark_out_of_sync();
            mark_chat_terminal_history_out_of_sync(app);
            if let Some(actual) = observed_resize.get() {
                log_resize_interrupted_draw(TerminalResizeDuringDraw::new(
                    snapshot, actual, "layout",
                ));
            } else {
                tracing::debug!(target: crate::logging::targets::APP_RENDER, event_name = "inline_chat_layout_source_changed");
            }
            return Ok(());
        }
        // Layout can yield without changing anything visible. Keep its continuation
        // independent of repainting so input and animations can still request a draw.
        if !self.needs_draw(app, &serialized_rows) {
            return Ok(());
        }
        self.draw_incremental(
            app,
            screen_size,
            width,
            terminal_height,
            &serialized_rows,
            base_excluded_ids,
        )
    }

    fn prepare_rows(
        &mut self,
        app: &mut App,
        width: u16,
        excluded_ids: &BTreeSet<HistoryOutputId>,
        control: &LayoutControl<'_>,
    ) -> Option<SerializedLiveRows> {
        if self.preparation.as_ref().is_some_and(|preparation| {
            preparation.source_revision != app.transcript.topology_revision()
        }) {
            self.preparation = None;
            self.terminal.cancel_replay();
            self.history.mark_out_of_sync();
            mark_chat_terminal_history_out_of_sync(app);
            return None;
        }
        let mut rows = SerializedLiveRows::new(Vec::new(), Vec::new());
        if self.preparation.is_none() && !self.history.is_synced() {
            self.preparation = Some(Box::new(ReplayPreparation::new(app)));
        }
        if let Some(preparation) = &mut self.preparation {
            if preparation.finished {
                return Some(preparation.reading_rows.clone());
            }
            // Changing the source-window policy restarts the whole replay. A semantic
            // block may already have emitted its title but still be preparing its body.
            if preparation.reading != app.chat_render.viewport.is_reading() {
                self.preparation = None;
                self.terminal.cancel_replay();
                self.history.mark_out_of_sync();
                mark_chat_terminal_history_out_of_sync(app);
                return None;
            }
            // Apply backpressure: finish inserting this chunk before preparing more history.
            if self.terminal.has_pending_history_rows() {
                return Some(rows);
            }
            let started = Instant::now();
            let mut complete = false;
            while let Some(mut next) = preparation.cursor.next(app, width, excluded_ids, control) {
                let has_mutable = next.stable_row_count() < next.rows().len();
                if preparation.mutable_seen {
                    next.defer_commit();
                }
                preparation.mutable_seen |= has_mutable;
                rows.append(next);
                // The mutable suffix is rebuilt from canonical state on the following frame.
                if has_mutable && !app.chat_render.viewport.is_reading() {
                    complete = true;
                    break;
                }
                // Insert the ready chunk before starting another semantic item.
                // Producers also yield while a source block has no rows ready.
                if !rows.rows().is_empty() || control.yield_due() {
                    break;
                }
            }
            complete |= preparation.cursor.is_finished();
            if app.chat_render.viewport.is_reading() {
                preparation.reading_rows.append(rows.clone());
            }
            tracing::debug!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_layout_step",
                rows = rows.rows().len(),
                assistant_text = rows.segments().iter().any(|segment| segment.kind == LiveRowBoundaryKind::AssistantText),
                elapsed_us = started.elapsed().as_micros(),
                complete,
                cancelled = control.cancelled(),
            );
            if complete {
                preparation.finished = true;
            }
        } else {
            rows = serialize_live_rows_with_boundaries_excluding(
                app,
                width,
                excluded_ids,
                Some(control),
            );
        }
        Some(rows)
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
        let layout_pending = self.preparation.as_ref().is_some_and(|preparation| {
            !preparation.finished || preparation.reading != app.chat_render.viewport.is_reading()
        });
        if !layout_pending && app.chat_render.viewport.is_reading() {
            history_plan.live_rows = self.preparation.as_ref().map_or_else(
                || serialized_rows.rows().to_vec(),
                |preparation| preparation.reading_rows.rows().to_vec(),
            );
            history_plan.excluded_rows = 0;
        }
        let live_rows = history_plan.live_rows.as_slice();
        let reading_rows = self
            .preparation
            .as_ref()
            .filter(|preparation| preparation.finished && app.chat_render.viewport.is_reading());
        let viewport_rows =
            reading_rows.map_or(serialized_rows, |preparation| &preparation.reading_rows);
        let (requested_layout_plan, layout_plan) = if layout_pending {
            let requested = MutableLayoutPlan::new(live_rows, &composer, terminal_height);
            (requested, MutableLayoutPlan::new(live_rows, &composer, requested.viewport_height))
        } else {
            viewport_layout(app, viewport_rows, live_rows, &composer, terminal_height)
        };
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
            render_mutable_frame(
                frame,
                app,
                &composer,
                layout_plan,
                &visible_live_rows,
                viewport_area,
            );
        });
        let Some(outcome) = self.resolve_chat_draw_outcome(app, outcome_result)? else {
            return Ok(());
        };
        if !layout_pending {
            let viewport_rows = self
                .preparation
                .as_ref()
                .filter(|preparation| preparation.finished && app.chat_render.viewport.is_reading())
                .map_or(serialized_rows, |preparation| &preparation.reading_rows);
            record_viewport_anchor(
                app,
                viewport_rows,
                layout_plan.live_window.start,
                visible_live_row_count,
            );
        }
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
            if !self.terminal.active_replay_matches_width(width) {
                self.terminal.cancel_replay();
            } else if self.terminal.has_pending_history_rows() {
                let mut excluded_ids = base_excluded_ids;
                excluded_ids.extend(self.terminal.replay_pending_ids());
                return history_flush_without_action(serialized_rows, excluded_ids, true);
            }
        }

        let replay =
            build_replay_history_batches(serialized_rows, width, self.history.confirmed_ids());
        if replay.confirm_ids.is_empty()
            && self.preparation.is_none()
            && !self.terminal.is_replay_active()
        {
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
            HistoryFlushAction::FeedReplay {
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
            HistoryFlushAction::FeedReplay { render_width, batches, confirm_ids } => {
                let input_complete =
                    self.preparation.as_ref().is_none_or(|preparation| preparation.finished);
                self.terminal.queue_replay(render_width, batches, confirm_ids, input_complete);
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
        if outcome.flushed_history.replay_complete
            && let Some(preparation) = self.preparation.take()
            && preparation.mutable_seen
            && !preparation.reading
        {
            // Replay stops at the first mutable item. Now that the stable prefix
            // is committed, render the entire live suffix from canonical state.
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
            crate::ui::activity_rows::build_activity_rows(app, width, crate::app::activity::now());
        let activity_row_count = u16::try_from(activity_rows.len()).unwrap_or(u16::MAX);
        let mut hint_rows = wrapped_composer_hint_rows(app, width);
        if app.chat_render.viewport.is_reading() {
            hint_rows.extend(crate::ui::input_rows::reading_hint_rows(app, width));
        }
        let hint_row_count = u16::try_from(hint_rows.len()).unwrap_or(u16::MAX);
        let btw_rows = build_btw_status_rows(app, width);
        let btw_row_count = u16::try_from(btw_rows.len()).unwrap_or(u16::MAX);
        let footer = serialize_footer_rows(app, width);
        let footer_rows = Vec::from(footer.rows);
        let footer_row_count = u16::try_from(footer_rows.len()).unwrap_or(u16::MAX);

        let status_rows = app
            .composer_access()
            .blocked_reason()
            .and_then(|reason| blocked_input_lines(app, reason, width));
        let editor = if let Some(rows) = status_rows {
            ComposerEditor::Status(rows)
        } else {
            let desired_height = input::editor_height(app, width);
            ComposerEditor::TextArea { desired_height }
        };
        let editor_row_count = editor.total_len_u16();

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

        ComposerSurface { activity_rows, hint_rows, btw_rows, editor, footer_rows }
    }
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
    FeedReplay {
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
    excluded_ids: &BTreeSet<HistoryOutputId>,
) -> ReplayHistoryPlan {
    let stable_row_count = serialized_rows.stable_row_count();
    let stable_segments = serialized_rows
        .segments()
        .iter()
        .filter(|segment| segment.commit_ready && segment.start_row < stable_row_count)
        .filter(|segment| !segment.ids.iter().all(|id| excluded_ids.contains(id)))
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
        if expected_start.is_some_and(|expected| expected != segment.start_row) {
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
    editor: ComposerEditor,
    footer_rows: Vec<Line<'static>>,
}

impl ComposerSurface {
    fn total_len(&self) -> usize {
        self.activity_rows
            .len()
            .saturating_add(self.hint_rows.len())
            .saturating_add(self.btw_rows.len())
            .saturating_add(self.editor.total_len())
            .saturating_add(self.footer_rows.len())
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
            ComposerEditor::Status(_) => editor_rows.to_vec(),
        };

        activity_rows
            .iter()
            .chain(hint_rows.iter())
            .chain(btw_rows.iter())
            .chain(editor_preview.iter())
            .chain(footer_rows.iter())
            .cloned()
            .collect::<Vec<_>>()
    }
}

enum ComposerEditor {
    TextArea {
        desired_height: u16,
    },
    /// Read-only status rows rendered in the composer field in place of the draft.
    Status(Vec<Line<'static>>),
}

impl ComposerEditor {
    fn total_len(&self) -> usize {
        match self {
            Self::TextArea { desired_height } => usize::from(*desired_height),
            Self::Status(rows) => usize::from(input::status_field_height(rows.len())),
        }
    }

    fn total_len_u16(&self) -> u16 {
        u16::try_from(self.total_len()).unwrap_or(u16::MAX)
    }

    fn visible_rows(&self, height: u16) -> &[Line<'static>] {
        match self {
            Self::TextArea { .. } => &[],
            Self::Status(rows) => RowWindow::tail(rows.len(), height).slice(rows),
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
        let editor_height = composer.editor.total_len_u16().min(editor_budget);
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

fn render_mutable_frame(
    frame: &mut ratatui::Frame<'_>,
    app: &mut App,
    composer: &ComposerSurface,
    layout: MutableLayoutPlan,
    visible_live_rows: &[Line<'static>],
    viewport: Rect,
) {
    let (live, activity, hints, btw, editor, footer) = layout.areas(viewport);
    for (area, rows) in [
        (live, visible_live_rows),
        (activity, layout.activity_window.slice(&composer.activity_rows)),
        (hints, layout.hint_visible_rows(&composer.hint_rows)),
        (btw, layout.btw_visible_rows(&composer.btw_rows)),
        (footer, layout.footer_visible_rows(&composer.footer_rows)),
    ] {
        if !area.is_empty() {
            frame.render_widget(Paragraph::new(rows.to_vec()), area);
        }
    }
    if !editor.is_empty() {
        render_composer_editor(frame, app, &composer.editor, editor);
    }
}

fn render_composer_editor(
    frame: &mut ratatui::Frame<'_>,
    app: &mut App,
    editor: &ComposerEditor,
    area: Rect,
) {
    match editor {
        ComposerEditor::TextArea { .. } => render_textarea_editor(frame, app, area),
        ComposerEditor::Status(rows) => render_status_editor(frame, rows, area),
    }
}

/// Paints the composer field background and prompt marker shared by every editor body.
fn render_composer_field(frame: &mut ratatui::Frame<'_>, area: Rect) -> input::InputRenderGeometry {
    let geometry = input::compute_render_geometry(area);
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
    geometry
}

fn render_status_editor(frame: &mut ratatui::Frame<'_>, rows: &[Line<'static>], area: Rect) {
    let geometry = render_composer_field(frame, area);
    let mut visible_rows = RowWindow::tail(rows.len(), geometry.text.height).slice(rows).to_vec();
    // Prewrapped rows carry explicit backgrounds, including padded cells.
    // The composer owns the field background; retain only their foreground styling.
    for span in visible_rows.iter_mut().flat_map(|row| &mut row.spans) {
        span.style = span.style.bg(theme::USER_MSG_BG);
    }
    frame.render_widget(
        Paragraph::new(visible_rows).style(Style::default().bg(theme::USER_MSG_BG)),
        geometry.text,
    );
}

fn render_textarea_editor(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let geometry = render_composer_field(frame, area);

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
    type ChatTerminalSession = super::ChatTerminalSession;
    use super::{HistoryCommitState, MutableLayoutPlan};
    use crate::app::terminal_runtime::chat_terminal::ChatTerminal;
    use crate::app::{
        App, AppStatus, ChatMessage, ChatMessageId, FocusTarget, HistoryOutputId, MessageBlock,
        MessageRole, TextBlock, TextBlockSpacing,
    };
    use crate::ui::inline_chat_rows::{LiveRowBoundaryKind, LiveRowSegment, SerializedLiveRows};
    use crate::ui::input;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use std::collections::BTreeSet;

    fn rows(count: usize) -> Vec<Line<'static>> {
        (0..count).map(|idx| Line::from(format!("row {idx}"))).collect()
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
            kind: LiveRowBoundaryKind::Message,
            start_row,
            end_row,
            commit_ready: true,
        }
    }

    fn assistant_blocks_message(blocks: Vec<MessageBlock>) -> ChatMessage {
        ChatMessage::new(MessageRole::Assistant, blocks, None)
    }

    fn session_with_history(history: HistoryCommitState) -> ChatTerminalSession {
        ChatTerminalSession { terminal: ChatTerminal::new(0), history, preparation: None }
    }

    #[test]
    fn removing_history_during_replay_discards_queued_rows_and_requests_a_fresh_replay() {
        let mut app = App::test_default();
        for text in ["REMOVED", "RETAINED"] {
            app.push_message_tracked(ChatMessage::new(
                MessageRole::User,
                vec![MessageBlock::Text(TextBlock::from_complete(text))],
                None,
            ));
        }
        let _ = app.surface_dirty.chat.take_rebuild();
        let mut history = HistoryCommitState::default();
        history.reset_for_purge_replay();
        let mut session = session_with_history(history);
        let control = crate::ui::layout_control::LayoutControl::uninterrupted();
        let rows = session.prepare_rows(&mut app, 80, &BTreeSet::new(), &control).unwrap();
        let mut plan = session.prepare_history_flush(&rows, 80, BTreeSet::new());
        session.queue_history_plan(plan.take_action());
        assert!(session.terminal.has_pending_history_rows());

        assert!(app.remove_message_tracked(0).is_some());
        assert!(session.prepare_rows(&mut app, 80, &BTreeSet::new(), &control).is_none());
        assert!(!session.terminal.is_replay_active());
        assert!(session.history.confirmed_ids().is_empty());
        assert_eq!(
            app.surface_dirty.chat.take_rebuild(),
            crate::app::ChatRebuildKind::PurgeReplay(
                crate::app::ChatPurgeReplayOptions::terminal_history_out_of_sync()
            )
        );
        let fresh = session.prepare_rows(&mut app, 80, &BTreeSet::new(), &control).unwrap();
        let text = fresh.rows().iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(text.iter().filter(|row| row.contains("RETAINED")).count(), 1);
        assert!(!text.iter().any(|row| row.contains("REMOVED")));
    }

    #[test]
    fn entering_reading_during_preparation_includes_the_already_visited_prefix() {
        let mut app = App::test_default();
        for text in ["FIRST", "LAST"] {
            app.transcript.messages.push(ChatMessage::new(
                MessageRole::User,
                vec![MessageBlock::Text(TextBlock::from_complete(text))],
                None,
            ));
        }
        let mut history = HistoryCommitState::default();
        history.reset_for_purge_replay();
        let mut session = session_with_history(history);
        let control = crate::ui::layout_control::LayoutControl::uninterrupted();
        let _ = session.prepare_rows(&mut app, 80, &BTreeSet::new(), &control);
        app.chat_render.viewport.pause();
        assert!(session.prepare_rows(&mut app, 80, &BTreeSet::new(), &control).is_none());
        assert!(matches!(
            app.surface_dirty.chat.take_rebuild(),
            crate::app::ChatRebuildKind::PurgeReplay(_)
        ));
        let _ = session.prepare_rows(&mut app, 80, &BTreeSet::new(), &control);
        while session.preparation.as_ref().is_some_and(|preparation| !preparation.finished) {
            let _ = session.prepare_rows(&mut app, 80, &BTreeSet::new(), &control);
        }
        let rows = &session.preparation.as_ref().unwrap().reading_rows;
        let text = rows.rows().iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(text.iter().filter(|row| row.contains("FIRST")).count(), 1);
        assert_eq!(text.iter().filter(|row| row.contains("LAST")).count(), 1);
    }

    #[test]
    fn replay_preserves_all_retained_segments_in_transcripts_over_nine_thousand_rows() {
        let first = output_id();
        let second = output_id();
        let serialized = SerializedLiveRows::from_parts_for_test(
            rows(9_100),
            vec![live_segment(0, 4_550, first.clone()), live_segment(4_550, 9_100, second.clone())],
        );

        let replay = super::build_replay_history_batches(&serialized, 80, &BTreeSet::new());
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
    fn composer_field_contains_draft_and_caret_through_constrained_resize() {
        let mut app = App::test_default();
        app.input.set_text("FIRST\nLAST");
        app.input.set_cursor(1, 4);
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        for area in [Rect::new(5, 3, 30, 4), Rect::new(5, 3, 60, 12)] {
            let view = paint_composer_fixture(&mut app, &mut terminal, area);
            assert_eq!(view.draft(), if area.height == 4 { "LAST" } else { "FIRST\nLAST" });
            view.assert_caret_after("LAST");
            assert!(view.editor.bottom() <= view.footer.y);
            for y in 0..30 {
                for x in 0..90 {
                    if !area.contains((x, y).into()) {
                        assert_eq!(view.backend.buffer()[(x, y)].symbol(), " ");
                    }
                }
            }
            let first_y = (view.editor.y..view.editor.bottom())
                .find(|y| {
                    (view.editor.x..view.editor.right())
                        .map(|x| view.backend.buffer()[(x, *y)].symbol())
                        .collect::<String>()
                        .contains(if area.height == 4 { "LAST" } else { "FIRST" })
                })
                .unwrap();
            let prefix = (view.editor.x..view.text.x)
                .map(|x| view.backend.buffer()[(x, first_y)].symbol())
                .collect::<String>();
            assert!(!prefix.trim().is_empty());
        }
    }

    struct ComposerFrame {
        backend: TestBackend,
        hints: Rect,
        editor: Rect,
        text: Rect,
        footer: Rect,
    }

    impl ComposerFrame {
        fn region(&self, area: Rect) -> String {
            (area.y..area.bottom())
                .map(|y| {
                    (area.x..area.right())
                        .map(|x| self.backend.buffer()[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        fn draft(&self) -> String {
            self.region(self.text)
        }
        fn hints(&self) -> String {
            self.region(self.hints)
        }
        fn assert_caret_after(&self, word: &str) {
            assert!(self.backend.cursor_visible());
            let cursor = self.backend.cursor_position();
            assert!(self.text.contains(cursor));
            let row = (self.text.x..cursor.x)
                .map(|x| self.backend.buffer()[(x, cursor.y)].symbol())
                .collect::<String>();
            assert!(row.ends_with(word), "caret after {row:?}, expected {word:?}");
        }
        fn assert_tokens(&self, source: &str, tokens: &[&str]) {
            let chars = (self.text.y..self.text.bottom())
                .flat_map(|y| (self.text.x..self.text.right()).map(move |x| (x, y)))
                .flat_map(|xy| {
                    let cell = &self.backend.buffer()[xy];
                    cell.symbol()
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .map(move |c| (c, cell.style()))
                })
                .collect::<Vec<_>>();
            let expected = source.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            assert_eq!(chars.iter().map(|(c, _)| *c).collect::<String>(), expected);
            let ordinary = chars.last().unwrap().1;
            let mut highlighted = vec![false; chars.len()];
            for token in tokens {
                let token = token.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                let byte = expected.find(&token).unwrap();
                let start = expected[..byte].chars().count();
                let end = start + token.chars().count();
                let style = chars[start].1;
                assert_ne!(style, ordinary, "missing token style: {token}");
                for i in start..end {
                    assert_eq!(chars[i].1, style, "partial token: {token}");
                    highlighted[i] = true;
                }
            }
            for (i, (_, style)) in chars.iter().enumerate() {
                if !highlighted[i] {
                    assert_eq!(*style, ordinary, "styled neighbor at {i}");
                }
            }
        }
    }
    fn paint_composer_fixture(
        app: &mut App,
        terminal: &mut Terminal<TestBackend>,
        area: Rect,
    ) -> ComposerFrame {
        let surface = ChatTerminalSession::build_composer_surface(app, area.width);
        let layout = MutableLayoutPlan::new(&[], &surface, area.height);
        let (_, _, hints, _, editor, footer) = layout.areas(area);
        terminal
            .draw(|frame| super::render_mutable_frame(frame, app, &surface, layout, &[], area))
            .unwrap();
        ComposerFrame {
            backend: terminal.backend().clone(),
            hints,
            editor,
            footer,
            text: input::compute_render_geometry(editor).text,
        }
    }
    fn draw_composer_fixture(app: &mut App, width: u16, height: u16) -> ComposerFrame {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        paint_composer_fixture(app, &mut terminal, Rect::new(0, 0, width, height))
    }
    fn press_composer_key(
        app: &mut App,
        code: crossterm::event::KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) {
        crate::app::events::handle_terminal_event(
            app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(code, modifiers)),
        );
    }
    fn request_composer_permission(
        app: &mut App,
    ) -> tokio::sync::oneshot::Receiver<crate::agent::model::RequestPermissionResponse> {
        use crate::agent::{events::ClientEvent, model};
        app.session_runtime.session_id = Some(model::SessionId::new("test-session"));
        let tc = crate::app::test_support::tool_call_info(
            "audit-permission",
            model::ToolCallStatus::InProgress,
        );
        app.push_message_tracked(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(tc))],
            None,
        ));
        app.index_tool_call("audit-permission".into(), app.transcript.messages.len() - 1, 0);
        let (tx, rx) = tokio::sync::oneshot::channel();
        crate::app::events::handle_client_event(
            app,
            ClientEvent::PermissionRequest {
                session_id: "test-session".into(),
                request: model::RequestPermissionRequest::new(
                    "test-session",
                    model::ToolCallUpdate::new(
                        "audit-permission",
                        model::ToolCallUpdateFields::new(),
                    ),
                    vec![model::PermissionOption::new(
                        "allow",
                        "Allow",
                        model::PermissionOptionKind::AllowOnce,
                    )],
                    None,
                ),
                response_tx: tx,
            },
        );
        app.claim_focus_target(FocusTarget::Permission);
        rx
    }
    fn resolve_composer_permission(app: &mut App) {
        crate::app::events::handle_client_event(
            app,
            crate::agent::events::ClientEvent::InteractionCancelled {
                session_id: "test-session".into(),
                interaction_id: "audit-permission".into(),
            },
        );
    }
    #[test]
    fn composer_slash_highlights_only_the_leading_token() {
        for (source, tokens) in [
            ("/mode plan END", vec!["/mode"]),
            ("\u{3000}/møde plan END", vec!["/møde"]),
            ("hello /mode END", vec![]),
            ("/ END", vec![]),
            ("   END", vec![]),
        ] {
            let mut app = App::test_default();
            app.input.set_text(source);
            draw_composer_fixture(&mut app, 80, 16).assert_tokens(source, &tokens);
            assert_eq!(app.input.text(), source);
        }
    }
    #[test]
    fn composer_subagent_highlights_refresh_after_edits() {
        let mut app = App::test_default();
        app.input.set_text("界 &reviewer and &explore && END");
        draw_composer_fixture(&mut app, 80, 16)
            .assert_tokens("界 &reviewer and &explore && END", &["&reviewer", "&explore"]);
        app.input.set_cursor(0, 2);
        app.input.textarea_delete_char_after();
        draw_composer_fixture(&mut app, 80, 16)
            .assert_tokens("界 reviewer and &explore && END", &["&explore"]);
        app.input.set_text("cargo test && cargo clippy END");
        draw_composer_fixture(&mut app, 80, 16)
            .assert_tokens("cargo test && cargo clippy END", &[]);
    }
    #[test]
    fn composer_literal_mentions_keep_their_unicode_regions_and_clear_stale_style() {
        let mut app = App::test_default();
        app.input.set_text("界 @");
        app.input.set_cursor(0, 3);
        crate::app::mention::activate(&mut app);
        app.input.insert_str("döcs/manual path");
        crate::app::mention::update_query(&mut app);
        assert!(crate::app::mention::commit_literal_if_active(&mut app));
        app.input.insert_str("and @");
        crate::app::mention::activate(&mut app);
        app.input.insert_str("other path");
        crate::app::mention::update_query(&mut app);
        assert!(crate::app::mention::commit_literal_if_active(&mut app));
        app.input.insert_str("END");
        let source = "界 @'döcs/manual path' and @'other path' END";
        assert_eq!(app.input.text(), source);
        let cursor = app.input.cursor();
        draw_composer_fixture(&mut app, 100, 18)
            .assert_tokens(source, &["@'döcs/manual path'", "@'other path'"]);
        assert_eq!(app.input.cursor(), cursor);
        assert_eq!(app.input.text(), source);
        app.input.set_cursor(0, 2);
        app.input.textarea_delete_char_after();
        app.input.textarea_insert_char('x');
        draw_composer_fixture(&mut app, 100, 18)
            .assert_tokens("界 x'döcs/manual path' and @'other path' END", &["@'other path'"]);
    }
    #[test]
    fn composer_atoms_keep_payloads_and_highlights_after_unicode_edits() {
        use crate::app::{clipboard_image::ImageAttachment, input::InputYankPart};
        let mut app = App::test_default();
        let images = [
            ImageAttachment { data: "YQ==".into(), mime_type: "image/png".into() },
            ImageAttachment { data: "Yg==".into(), mime_type: "image/jpeg".into() },
        ];
        app.input.insert_parts([
            InputYankPart::Text("界 prefix ".into()),
            InputYankPart::Paste("stored\nbody".into()),
            InputYankPart::Text(" mid ".into()),
            InputYankPart::Image(images[0].clone()),
            InputYankPart::Text(" ".into()),
            InputYankPart::Image(images[1].clone()),
            InputYankPart::Text(" 終 END".into()),
        ]);
        let original = app.input.lines().join("\n");
        let expanded = app.input.text();
        let cursor = app.input.cursor();
        let paste = app.input.inline_pastes();
        let start = original.find('[').unwrap();
        let end = original[start..].find(']').unwrap() + start + 1;
        let label = original[start..end].to_owned();
        for width in [34, 90] {
            draw_composer_fixture(&mut app, width, 24)
                .assert_tokens(&original, &[&label, "[Image #1]", "[Image #2]"]);
        }
        assert_eq!(app.input.lines().join("\n"), original);
        assert_eq!(app.input.text(), expanded);
        assert_eq!(app.input.cursor(), cursor);
        assert_eq!(app.input.images(), images);
        assert_eq!(app.input.inline_pastes(), paste);
        app.input.set_cursor(0, 0);
        app.input.insert_str("é ");
        let changed = app.input.lines().join("\n");
        draw_composer_fixture(&mut app, 34, 24)
            .assert_tokens(&changed, &[&label, "[Image #1]", "[Image #2]"]);
        assert_eq!(app.input.images(), images);
        assert_eq!(app.input.inline_pastes(), paste);
    }
    #[test]
    fn composer_bounded_viewport_keeps_both_draft_ends_editable() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        let draft = (0..25)
            .map(|n| format!("DRAFT_{n:02} words with a tail"))
            .collect::<Vec<_>>()
            .join("\n");
        app.input.set_text(&draft);
        for width in [36, 75] {
            app.input.set_cursor(24, 27);
            let last = draw_composer_fixture(&mut app, width, 45);
            assert!(last.draft().contains("DRAFT_24"));
            assert!(!last.draft().contains("DRAFT_00"));
            assert!(last.text.height < 25);
            assert!(last.backend.cursor_visible());
            assert!(last.editor.bottom() <= last.footer.y);
            app.input.set_cursor(0, 0);
            let first = draw_composer_fixture(&mut app, width, 45);
            assert!(first.draft().contains("DRAFT_00"));
            assert!(!first.draft().contains("DRAFT_24"));
            assert_eq!(app.input.text(), draft);
        }
        app.input.textarea_insert_char('X');
        assert!(draw_composer_fixture(&mut app, 75, 45).draft().contains("XDRAFT_00"));
        app.input.set_text("SHORT_END");
        app.input.set_cursor(0, 9);
        let short = draw_composer_fixture(&mut app, 75, 45);
        short.assert_caret_after("SHORT_END");
        assert!(short.text.height < 10);
    }
    #[test]
    fn composer_hint_allocation_keeps_the_actual_draft_budget() {
        for cancel in [false, true] {
            let mut app = App::test_default();
            app.status = AppStatus::Running;
            app.turn.cancel_requested = cancel;
            app.input.set_text("FIRST\nSECOND\nTHIRD");
            app.input.set_cursor(2, 5);
            let plain = draw_composer_fixture(&mut app, 80, 30);
            assert_eq!(plain.draft(), "FIRST\nSECOND\nTHIRD");
            plain.assert_caret_after("THIRD");
            app.session_runtime.login_hint = Some(crate::app::LoginHint {
                method_name: "OAuth".into(),
                method_description: "Sign in safely".into(),
            });
            app.pending_user_messages
                .try_push_sending(crate::app::PendingUserMessage::sending(
                    "queued".into(),
                    "QUEUED_BODY".into(),
                    vec![],
                ))
                .unwrap();
            let combined = draw_composer_fixture(&mut app, 80, 30);
            assert_eq!(combined.draft(), "FIRST\nSECOND\nTHIRD");
            combined.assert_caret_after("THIRD");
            assert!(combined.hints().contains("OAuth"));
            assert!(combined.hints().contains("QUEUED_BODY"));
            assert!(combined.hints.bottom() <= combined.editor.y);
        }
    }
    #[test]
    fn composer_login_content_and_action_survive_wrapping() {
        for (width, description) in [
            (40, "Visit the browser and authorize this project before continuing"),
            (90, "Use browser"),
        ] {
            let mut app = App::test_default();
            app.input.set_text("DRAFT_END");
            app.input.set_cursor(0, 9);
            app.session_runtime.login_hint = Some(crate::app::LoginHint {
                method_name: "OAuth".into(),
                method_description: description.into(),
            });
            let view = draw_composer_fixture(&mut app, width, 30);
            let hints = view.hints().split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(hints.contains("OAuth"));
            assert!(hints.contains(description));
            assert!(hints.contains("/login"));
            assert_eq!(view.draft(), "DRAFT_END");
            view.assert_caret_after("DRAFT_END");
        }
    }
    #[test]
    fn composer_suggestions_follow_real_focus_draft_and_autocomplete_transitions() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        app.session_runtime.prompt_suggestion = Some("Inspect retries".into());
        for cancel in [false, true] {
            app.turn.cancel_requested = cancel;
            let v = draw_composer_fixture(&mut app, 80, 22);
            assert!(v.hints().contains("Inspect retries") && v.hints().contains("Tab"));
        }
        app.turn.cancel_requested = false;
        app.input.set_text("DRAFT");
        let v = draw_composer_fixture(&mut app, 80, 22);
        assert!(!v.hints().contains("Inspect retries"));
        assert_eq!(v.draft(), "DRAFT");
        app.input.clear();
        let _rx = request_composer_permission(&mut app);
        let v = draw_composer_fixture(&mut app, 80, 22);
        assert!(!v.hints().contains("Inspect retries"));
        assert!(!v.backend.cursor_visible());
        resolve_composer_permission(&mut app);
        assert!(draw_composer_fixture(&mut app, 80, 22).hints().contains("Inspect retries"));
        app.input.set_text("@");
        app.input.set_cursor(0, 1);
        crate::app::mention::activate(&mut app);
        let v = draw_composer_fixture(&mut app, 80, 22);
        assert!(v.hints().contains("file") && v.hints().contains('@'));
        assert!(!v.hints().contains("Inspect retries"));
        crate::app::mention::deactivate(&mut app);
        app.input.clear();
        assert!(draw_composer_fixture(&mut app, 80, 22).hints().contains("Inspect retries"));
        app.session_runtime.prompt_suggestion = Some("   ".into());
        assert!(draw_composer_fixture(&mut app, 80, 22).hints().trim().is_empty());
    }
    #[test]
    fn composer_queue_previews_keep_ordinals_and_refresh_without_second_lines() {
        let mut app = App::test_default();
        app.input.set_text("DRAFT_END");
        app.input.set_cursor(0, 9);
        let mut terminal = Terminal::new(TestBackend::new(80, 22)).unwrap();
        for (id, body) in
            [("a", "APPLE"), ("b", "BERRY"), ("c", "CHERRY"), ("d", "DATE"), ("e", "ELDER")]
        {
            app.pending_user_messages
                .try_push_sending(crate::app::PendingUserMessage::sending(
                    id.into(),
                    format!("{body}\nPRIVATE_SECOND"),
                    vec![],
                ))
                .unwrap();
        }
        let first = paint_composer_fixture(&mut app, &mut terminal, Rect::new(0, 0, 80, 22));
        let hints = first.hints();
        assert!(hints.contains("(5)") && hints.contains("Esc"));
        assert!(!hints.contains("APPLE") && !hints.contains("PRIVATE_SECOND"));
        let visible = hints.lines().filter_map(|r| r.trim().split_once(". ")).collect::<Vec<_>>();
        assert!(!visible.is_empty() && visible.len() < 5);
        for (ordinal, body) in &visible {
            let n = ordinal.parse::<usize>().unwrap();
            assert_eq!(*body, ["APPLE", "BERRY", "CHERRY", "DATE", "ELDER"][n - 1]);
        }
        assert_eq!(
            visible.iter().map(|(n, _)| n.parse::<usize>().unwrap()).collect::<Vec<_>>(),
            ((6 - visible.len())..=5).collect::<Vec<_>>()
        );
        assert_eq!(visible.last().unwrap().1, "ELDER");
        first.assert_caret_after("DRAFT_END");
        app.pending_user_messages.remove("e").unwrap();
        app.pending_user_messages
            .try_push_sending(crate::app::PendingUserMessage::sending(
                "f".into(),
                "FIG\nPRIVATE_SECOND".into(),
                vec![],
            ))
            .unwrap();
        let next = paint_composer_fixture(&mut app, &mut terminal, Rect::new(0, 0, 80, 22));
        assert!(!next.hints().contains("ELDER"));
        assert!(next.hints().lines().any(|r| r.trim() == "5. FIG"));
        assert_eq!(next.draft(), "DRAFT_END");
        app.pending_user_messages.drain();
        let empty = paint_composer_fixture(&mut app, &mut terminal, Rect::new(0, 0, 80, 22));
        assert!(empty.hints().trim().is_empty());
        empty.assert_caret_after("DRAFT_END");
    }
    #[test]
    fn composer_reading_hints_preserve_multiline_draft_and_caret() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        for n in 0..20 {
            app.push_message_tracked(ChatMessage::new(
                MessageRole::User,
                vec![MessageBlock::Text(TextBlock::from_complete(&format!("HISTORY {n}")))],
                None,
            ));
        }
        app.input.set_text("FIRST source\nSECOND source\nTHIRD_END");
        app.input.set_cursor(2, 9);
        let mut terminal = Terminal::new(TestBackend::new(38, 30)).unwrap();
        press_composer_key(&mut app, KeyCode::PageUp, KeyModifiers::NONE);
        assert!(app.chat_render.viewport.is_reading());
        let reading = paint_composer_fixture(&mut app, &mut terminal, Rect::new(0, 0, 38, 30));
        assert!(reading.hints().contains("Reading output"));
        assert!(reading.hints.height > 1);
        assert_eq!(reading.draft(), "FIRST source\nSECOND source\nTHIRD_END");
        reading.assert_caret_after("THIRD_END");
        press_composer_key(&mut app, KeyCode::End, KeyModifiers::CONTROL);
        let live = paint_composer_fixture(&mut app, &mut terminal, Rect::new(0, 0, 38, 30));
        assert!(!live.region(Rect::new(0, 0, 38, 30)).contains("Reading output"));
        assert_eq!(live.draft(), "FIRST source\nSECOND source\nTHIRD_END");
        live.assert_caret_after("THIRD_END");
        terminal.backend_mut().resize(70, 30);
        let wide = paint_composer_fixture(&mut app, &mut terminal, Rect::new(0, 0, 70, 30));
        assert_eq!(wide.draft(), "FIRST source\nSECOND source\nTHIRD_END");
        wide.assert_caret_after("THIRD_END");
    }
    #[test]
    fn composer_shutdown_selects_guidance_over_competing_status() {
        for status in [AppStatus::Error, AppStatus::CommandPending] {
            let mut app = App::test_default();
            app.status = status;
            app.turn.pending_command_label = Some("COMPETING_LABEL".into());
            app.input.set_text("UNSENT_DRAFT");
            app.request_shutdown();
            let v = draw_composer_fixture(&mut app, 90, 18);
            let body = v.region(v.editor);
            assert!(body.contains("Shutting down") && body.contains("Ctrl+C"));
            assert!(!body.contains("COMPETING_LABEL"));
            assert!(!v.backend.cursor_visible());
            assert_eq!(app.input.text(), "UNSENT_DRAFT");
        }
    }
    include!("chat_session_audit_tests.rs");
}
