// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::chat_terminal::{ChatDrawRequest, ChatTerminal, HistoryBatchKind, PendingHistoryBatch};
use super::history_insert::RenderedHistoryRows;
use crate::app::{App, HistoryOutputId};
use crate::ui::footer_rows::serialize_footer_rows;
use crate::ui::inline_chat_rows::LiveRowBoundaryKind;
use crate::ui::inline_chat_rows::{
    SerializedLiveRows, serialize_live_rows_with_boundaries_excluding,
};
use crate::ui::input;
use crate::ui::input_rows::{blocked_input_lines, build_composer_hint_rows};
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

impl ChatTerminalSession {
    pub(super) fn new() -> anyhow::Result<Self> {
        let (width, height) =
            crossterm::terminal::size().context("failed to read chat terminal size")?;
        let (cursor_x, cursor_y) =
            crossterm::cursor::position().context("failed to read chat terminal cursor")?;
        let owned_top = cursor_y.min(height.saturating_sub(1));

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
        );

        Ok(Self { terminal: ChatTerminal::new(owned_top), history: HistoryCommitState::default() })
    }

    pub(super) fn clear(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
        self.history.reset();
    }

    pub(super) fn clear_session_boundary(&mut self, app: &mut App) {
        if let Err(err) = self.terminal.reset_session_boundary() {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_session_boundary_clear_failed",
                message = "failed to clear inline terminal for session boundary",
                outcome = "failure",
                error_message = %err,
            );
        }
        app.chat_render.invalidate_live_anchor();
        self.history.reset();
    }

    pub(super) fn clear_for_resize_purge_replay(&mut self, app: &mut App) {
        if let Err(err) = self.terminal.reset_resize_purge_replay() {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_resize_purge_replay_failed",
                message = "failed to purge terminal before resize replay",
                outcome = "failure",
                error_message = %err,
            );
        }
        app.chat_render.invalidate_live_anchor();
        self.history.reset_for_resize_purge_replay();
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

        let screen_size =
            crossterm::terminal::size().context("failed to read chat terminal size")?;
        let width = screen_size.0.max(1);
        let terminal_height = screen_size.1.max(1);
        app.chat_render.set_terminal_size(screen_size.0, screen_size.1);

        let base_excluded_ids = self.base_history_excluded_ids();
        let serialized_rows =
            serialize_live_rows_with_boundaries_excluding(app, width, &base_excluded_ids);
        self.draw_incremental(
            app,
            screen_size,
            width,
            terminal_height,
            &serialized_rows,
            base_excluded_ids,
        )
    }

    fn draw_incremental(
        &mut self,
        app: &mut App,
        screen_size: (u16, u16),
        width: u16,
        terminal_height: u16,
        serialized_rows: &SerializedLiveRows,
        base_excluded_ids: BTreeSet<HistoryOutputId>,
    ) -> anyhow::Result<()> {
        let composer = Self::build_composer_surface(app, width);
        let mut history_plan = self.prepare_history_flush(
            serialized_rows,
            &composer,
            width,
            terminal_height,
            base_excluded_ids,
        );
        let live_rows = history_plan.live_rows.as_slice();
        let requested_layout_plan = MutableLayoutPlan::new(live_rows, &composer, terminal_height);
        let layout_plan =
            MutableLayoutPlan::new(live_rows, &composer, requested_layout_plan.viewport_height);
        let visible_hint_rows = layout_plan.hint_visible_rows(&composer.hint_rows).to_vec();
        let visible_editor_rows = composer.editor_visible_rows(layout_plan.editor_height).to_vec();
        let visible_footer_rows = layout_plan.footer_visible_rows(&composer.footer_rows).to_vec();
        let composer_preview_rows =
            composer.preview_rows(&visible_hint_rows, &visible_editor_rows, &visible_footer_rows);
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
            terminal_width: width,
            terminal_height,
        };
        let history_action = history_plan.take_action();
        self.queue_history_plan(history_action);
        let outcome_result = self.terminal.draw_chat_frame(chat_frame, |frame, viewport_area| {
            let (live_area, hint_area, editor_area, footer_area) = layout_plan.areas(viewport_area);
            if !live_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_live_rows.clone()), live_area);
            }
            if !hint_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_hint_rows.clone()), hint_area);
            }
            if !editor_area.is_empty() {
                render_composer_editor(frame, app, &composer.editor, editor_area);
            }
            if !footer_area.is_empty() {
                frame.render_widget(Paragraph::new(visible_footer_rows.clone()), footer_area);
            }
        });
        let outcome = match outcome_result {
            Ok(outcome) => outcome,
            Err(err) => {
                self.history.mark_out_of_sync();
                mark_chat_terminal_history_out_of_sync(app);
                return Err(err);
            }
        };
        self.complete_history_flush(app, width, &outcome);
        let viewport_area = outcome.viewport_area;
        let (live_area, hint_area, editor_area, footer_area) = layout_plan.areas(viewport_area);
        complete_draw(
            app,
            DrawCompletion {
                viewport_area,
                live_area,
                hint_area,
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

        app.surface_dirty.chat.take_repaint();
        Ok(())
    }

    fn prepare_history_flush(
        &mut self,
        serialized_rows: &SerializedLiveRows,
        composer: &ComposerSurface,
        width: u16,
        terminal_height: u16,
        base_excluded_ids: BTreeSet<HistoryOutputId>,
    ) -> HistoryFlushPlan {
        let width = width.max(1);
        if !self.history.is_synced() {
            return self.prepare_replay_history_flush(serialized_rows, width, base_excluded_ids);
        }

        self.prepare_static_history_flush(
            serialized_rows,
            composer,
            width,
            terminal_height,
            base_excluded_ids,
        )
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

        let replay =
            build_replay_history_batches(serialized_rows, width, self.history.cap_replay_rows());
        if replay.confirm_ids.is_empty() {
            self.history.mark_terminal_history_synced(width, Vec::new());
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
        &self,
        serialized_rows: &SerializedLiveRows,
        composer: &ComposerSurface,
        width: u16,
        terminal_height: u16,
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
        let candidate_layout =
            MutableLayoutPlan::new(&candidate_live_rows, composer, terminal_height);
        let candidate_frame = ChatDrawRequest {
            requested_inline_height: candidate_layout.viewport_height,
            terminal_width: width,
            terminal_height,
        };
        let candidate_rows = candidate_batches.iter().map(|batch| batch.rows.len()).sum();
        if self.terminal.can_insert_scrollback_rows(candidate_frame, candidate_rows) {
            return HistoryFlushPlan::new(
                candidate_live_rows,
                excluded_row_count(serialized_rows, &candidate_excluded_ids),
                candidate_excluded_ids,
                HistoryFlushAction::QueueStatic(candidate_batches),
                candidate_rows,
                false,
            );
        }

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_scrollback_insert_deferred",
            message = "stable chat rows kept in source-backed live window because inline insertion is unsafe for current viewport",
            outcome = "deferred",
            rows = candidate_rows,
            confirmed_ids = self.history.confirmed_len(),
            history_width = self.history.width,
            requested_inline_height = candidate_frame.requested_inline_height,
            terminal_width = width,
            terminal_height,
        );

        history_flush_without_action(serialized_rows, base_excluded_ids, false)
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
        width: u16,
        outcome: &super::chat_terminal::ChatDrawOutcome,
    ) {
        if outcome.flushed_history.replay_complete {
            self.history
                .mark_terminal_history_synced(width, outcome.flushed_history.confirmed_ids.clone());
        } else if !outcome.flushed_history.confirmed_ids.is_empty() {
            self.history.confirm(width, outcome.flushed_history.confirmed_ids.clone());
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
        let hint_rows = build_composer_hint_rows(app);
        let hint_row_count = u16::try_from(hint_rows.len()).unwrap_or(u16::MAX);
        let footer = serialize_footer_rows(app, width);
        let footer_rows = Vec::from(footer.rows);
        let footer_row_count = u16::try_from(footer_rows.len()).unwrap_or(u16::MAX);

        let editor = if matches!(
            app.status,
            crate::app::AppStatus::Connecting
                | crate::app::AppStatus::CommandPending
                | crate::app::AppStatus::Error
        ) {
            ComposerEditor::Rows(blocked_input_lines(app))
        } else {
            let desired_height =
                input::visual_line_count(app, width).saturating_sub(hint_row_count).max(1);
            ComposerEditor::TextArea { desired_height }
        };
        let editor_row_count = editor.total_len_u16();

        app.chat_render.composer.width = width;
        app.chat_render.composer.hint_rows = hint_row_count;
        app.chat_render.composer.editor_rows = editor_row_count;
        app.chat_render.composer.footer_rows = footer_row_count;
        app.chat_render.composer.total_rows =
            hint_row_count.saturating_add(editor_row_count).saturating_add(footer_row_count);
        app.chat_render.composer.caret_row = 0;
        app.chat_render.composer.caret_col = 0;

        ComposerSurface { hint_rows, editor, footer_rows }
    }
}

fn mark_chat_terminal_history_out_of_sync(app: &mut App) {
    app.request_chat_resize_purge_replay_rebuild();
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
        hint_area: completion.hint_area,
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
    hint_area: Rect,
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
    width: u16,
    confirmed: BTreeSet<HistoryOutputId>,
    history_in_sync: bool,
    cap_next_purge_replay: bool,
}

const RESIZE_PURGE_REPLAY_MAX_ROWS: usize = 9_000;

impl Default for HistoryCommitState {
    fn default() -> Self {
        Self {
            width: 0,
            confirmed: BTreeSet::new(),
            history_in_sync: true,
            cap_next_purge_replay: false,
        }
    }
}

impl HistoryCommitState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn reset_for_resize_purge_replay(&mut self) {
        *self = Self { history_in_sync: false, cap_next_purge_replay: true, ..Self::default() };
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

    fn cap_replay_rows(&self) -> Option<usize> {
        self.cap_next_purge_replay.then_some(RESIZE_PURGE_REPLAY_MAX_ROWS)
    }

    fn confirm(&mut self, width: u16, ids: Vec<HistoryOutputId>) {
        self.width = width.max(1);
        self.confirmed.extend(ids);
        self.history_in_sync = true;
        self.cap_next_purge_replay = false;
    }

    fn mark_terminal_history_synced(&mut self, width: u16, ids: Vec<HistoryOutputId>) {
        self.width = width.max(1);
        self.confirmed.extend(ids);
        self.history_in_sync = true;
        self.cap_next_purge_replay = false;
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

        rows.extend(serialized_rows.rows()[segment.start_row..segment.end_row].iter().cloned());
        ids.extend(segment_ids);
        expected_start = Some(segment.end_row);
    }

    push_history_batch(&mut batches, HistoryBatchKind::Normal, width, &mut rows, &mut ids);
    batches
}

fn build_replay_history_batches(
    serialized_rows: &SerializedLiveRows,
    width: u16,
    row_cap: Option<usize>,
) -> ReplayHistoryPlan {
    let stable_row_count = serialized_rows.stable_row_count();
    let stable_segments = serialized_rows
        .segments()
        .iter()
        .filter(|segment| segment.commit_ready && segment.start_row < stable_row_count)
        .collect::<Vec<_>>();
    let confirm_ids = unique_ids(stable_segments.iter().flat_map(|segment| segment.ids.iter()));

    let start_idx = row_cap.map_or(0, |cap| {
        let mut rows = 0usize;
        let mut start = stable_segments.len();
        for (idx, segment) in stable_segments.iter().enumerate().rev() {
            let segment_rows = segment.end_row.saturating_sub(segment.start_row);
            if rows > 0 && rows.saturating_add(segment_rows) > cap {
                break;
            }
            rows = rows.saturating_add(segment_rows);
            start = idx;
        }
        start
    });

    let mut batches = Vec::new();
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    let mut queued_rows = 0usize;
    let mut expected_start = None;
    let empty = BTreeSet::new();

    for segment in stable_segments.into_iter().skip(start_idx) {
        let segment_ids = unexcluded_ids(&segment.ids, &empty);
        if segment_ids.is_empty() {
            continue;
        }
        if expected_start.is_some_and(|expected| expected != segment.start_row) || rows.len() >= 160
        {
            queued_rows = queued_rows.saturating_add(rows.len());
            push_history_batch(&mut batches, HistoryBatchKind::Replay, width, &mut rows, &mut ids);
        }
        rows.extend(serialized_rows.rows()[segment.start_row..segment.end_row].iter().cloned());
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
        .map(|segment| segment.end_row.saturating_sub(segment.start_row))
        .sum()
}

struct ComposerSurface {
    hint_rows: Vec<Line<'static>>,
    editor: ComposerEditor,
    footer_rows: Vec<Line<'static>>,
}

impl ComposerSurface {
    fn total_len(&self) -> usize {
        self.hint_rows
            .len()
            .saturating_add(self.editor.total_len())
            .saturating_add(self.footer_rows.len())
    }

    fn editor_visible_rows(&self, height: u16) -> &[Line<'static>] {
        self.editor.visible_rows(height)
    }

    fn preview_rows(
        &self,
        hint_rows: &[Line<'static>],
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

        hint_rows
            .iter()
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
        &rows[self.start..self.end()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MutableLayoutPlan {
    live_window: RowWindow,
    hint_window: RowWindow,
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
        let hint_budget = editor_budget.saturating_sub(editor_height);
        let hint_window = RowWindow::tail(composer.hint_rows.len(), hint_budget);
        let live_budget = hint_budget.saturating_sub(hint_window.visible_len_u16());
        let live_window = RowWindow::tail(live_rows.len(), live_budget);
        let viewport_height = live_window
            .visible_len_u16()
            .saturating_add(hint_window.visible_len_u16())
            .saturating_add(editor_height)
            .saturating_add(footer_window.visible_len_u16())
            .max(1)
            .min(screen_height);

        Self { live_window, hint_window, editor_height, footer_window, viewport_height }
    }

    fn live_visible_rows<'rows>(self, live_rows: &'rows [Line<'static>]) -> &'rows [Line<'static>] {
        self.live_window.slice(live_rows)
    }

    fn hint_visible_rows<'rows>(self, hint_rows: &'rows [Line<'static>]) -> &'rows [Line<'static>] {
        self.hint_window.slice(hint_rows)
    }

    fn footer_visible_rows<'rows>(
        self,
        footer_rows: &'rows [Line<'static>],
    ) -> &'rows [Line<'static>] {
        self.footer_window.slice(footer_rows)
    }

    fn visible_composer_len(self) -> usize {
        usize::from(
            self.hint_window
                .visible_len_u16()
                .saturating_add(self.editor_height)
                .saturating_add(self.footer_window.visible_len_u16()),
        )
    }

    fn areas(self, viewport_area: Rect) -> (Rect, Rect, Rect, Rect) {
        let footer_height = self.footer_window.visible_len_u16().min(viewport_area.height);
        let editor_height =
            self.editor_height.min(viewport_area.height.saturating_sub(footer_height));
        let hint_height = self
            .hint_window
            .visible_len_u16()
            .min(viewport_area.height.saturating_sub(footer_height).saturating_sub(editor_height));
        let live_height = viewport_area
            .height
            .saturating_sub(hint_height)
            .saturating_sub(editor_height)
            .saturating_sub(footer_height);
        let live_area =
            Rect::new(viewport_area.x, viewport_area.y, viewport_area.width, live_height);
        let hint_area = Rect::new(
            viewport_area.x,
            viewport_area.y.saturating_add(live_height),
            viewport_area.width,
            hint_height,
        );
        let editor_area = Rect::new(
            viewport_area.x,
            viewport_area.y.saturating_add(live_height).saturating_add(hint_height),
            viewport_area.width,
            editor_height,
        );
        let footer_area = Rect::new(
            viewport_area.x,
            viewport_area
                .y
                .saturating_add(live_height)
                .saturating_add(hint_height)
                .saturating_add(editor_height),
            viewport_area.width,
            footer_height,
        );
        (live_area, hint_area, editor_area, footer_area)
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
        ComposerEditor::Rows(rows) => {
            let visible_rows = RowWindow::tail(rows.len(), area.height).slice(rows).to_vec();
            frame.render_widget(Paragraph::new(visible_rows), area);
        }
    }
}

fn render_textarea_editor(frame: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let geometry = input::compute_render_geometry(area, 0);
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
}

struct InlineViewportDrawMetrics {
    viewport_area: Rect,
    live_area: Rect,
    hint_area: Rect,
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
            .hint_area
            .top()
            .min(metrics.editor_area.top())
            .min(metrics.footer_area.top()),
        composer_height = metrics
            .hint_area
            .height
            .saturating_add(metrics.editor_area.height)
            .saturating_add(metrics.footer_area.height),
        hint_top = metrics.hint_area.top(),
        hint_height = metrics.hint_area.height,
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
        mode = summary.app.mode.as_ref().map_or_else(|| "none".to_owned(), |mode| mode.current_mode_name.clone()),
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
        ChatTerminalSession, ComposerEditor, ComposerSurface, HistoryCommitState, MutableLayoutPlan,
    };
    use crate::app::terminal_runtime::chat_terminal::ChatTerminal;
    use crate::app::terminal_runtime::chat_terminal::plan_inline_geometry;
    use crate::app::{
        App, ChatMessage, ChatMessageId, HistoryOutputId, MessageBlock, MessageRole, TextBlock,
        TextBlockSpacing,
    };
    use crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding;
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
            hint_rows: rows(hint_rows),
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

    fn session_with_history(history: HistoryCommitState) -> ChatTerminalSession {
        ChatTerminalSession { terminal: ChatTerminal::new(0), history }
    }

    #[test]
    fn history_commit_state_confirms_output_ids_once() {
        let mut state = HistoryCommitState::default();
        let first = output_id();
        let second = output_id();

        state.confirm(80, vec![first.clone()]);
        state.confirm(80, vec![first.clone(), second.clone()]);

        assert_eq!(state.confirmed_len(), 2);
        assert!(state.confirmed_ids().contains(&first));
        assert!(state.confirmed_ids().contains(&second));
        assert!(state.is_synced());
    }

    #[test]
    fn resize_purge_replay_marks_history_unsynced_and_caps_replay_rows() {
        let mut state = HistoryCommitState::default();
        state.confirm(80, vec![output_id()]);

        state.reset_for_resize_purge_replay();

        assert!(!state.is_synced());
        assert_eq!(state.confirmed_len(), 0);
        assert_eq!(state.cap_replay_rows(), Some(super::RESIZE_PURGE_REPLAY_MAX_ROWS));
    }

    #[test]
    fn replay_completion_marks_history_synced_and_clears_cap() {
        let mut state = HistoryCommitState::default();
        let id = output_id();

        state.reset_for_resize_purge_replay();
        state.mark_terminal_history_synced(80, vec![id.clone()]);

        assert!(state.is_synced());
        assert_eq!(state.cap_replay_rows(), None);
        assert!(state.confirmed_ids().contains(&id));
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
        app.messages.push(message);

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
    fn fullscreen_reattach_preserves_history_commit_state() {
        let mut state = HistoryCommitState::default();
        state.confirm(80, vec![output_id()]);
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
        state.confirm(80, vec![output_id()]);
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
        let (live_area, hint_area, editor_area, footer_area) = plan.areas(Rect::new(0, 0, 80, 4));

        assert_eq!(footer_area.height, 2);
        assert_eq!(editor_area.height, 2);
        assert_eq!(hint_area.height, 0);
        assert_eq!(live_area.height, 0);
        assert_eq!(plan.viewport_height, 4);
    }

    #[test]
    fn mutable_layout_uses_remaining_height_for_hints_then_live_rows() {
        let live_rows = rows(4);
        let composer = textarea_composer(2, 1, 1);

        let plan = MutableLayoutPlan::new(&live_rows, &composer, 5);
        let (live_area, hint_area, editor_area, footer_area) = plan.areas(Rect::new(0, 0, 80, 5));

        assert_eq!(footer_area.height, 1);
        assert_eq!(editor_area.height, 1);
        assert_eq!(hint_area.height, 2);
        assert_eq!(live_area.height, 1);
        assert_eq!(plan.viewport_height, 5);
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
        let (live_area, _, editor_area, footer_area) = resolved_plan
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

        assert_eq!(plan.target_area, Some(Rect::new(0, 34, 120, 4)));
        assert_eq!(plan.height, 4);
    }

    #[test]
    fn plan_for_composer_shrink_preserves_viewport_anchor() {
        let old_area = Rect::new(0, 36, 120, 4);

        let plan = plan_inline_geometry(Some(old_area), 3, 120, 40);

        assert_eq!(plan.target_area, Some(Rect::new(0, 36, 120, 3)));
    }
}
