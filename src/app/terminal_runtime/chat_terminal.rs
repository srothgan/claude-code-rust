// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::history_insert::RenderedHistoryRows;
use crate::app::HistoryOutputId;
use anyhow::{Context, anyhow, bail};
use crossterm::SynchronizedUpdate;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal::{Clear, ClearType, DisableLineWrap};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear as RatatuiClear, Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::io::{Stdout, Write};

type StdoutBackend = CrosstermBackend<Stdout>;
type StdoutTerminal = Terminal<StdoutBackend>;
pub(super) const PURGE_REPLAY_CLEAR_ANSI: &str = "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H";
const MIN_REPLAY_BATCH_ROWS: usize = 32;
const MAX_REPLAY_BATCH_ROWS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalSnapshot {
    pub width: u16,
    pub height: u16,
}

impl TerminalSnapshot {
    pub(super) fn new(width: u16, height: u16) -> Self {
        Self { width: width.max(1), height: height.max(1) }
    }

    fn current(context: &'static str) -> anyhow::Result<Self> {
        let (width, height) = crossterm::terminal::size().with_context(|| context)?;
        Ok(Self::new(width, height))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalResizeDuringDraw {
    expected: TerminalSnapshot,
    actual: TerminalSnapshot,
    phase: &'static str,
}

impl TerminalResizeDuringDraw {
    fn new(expected: TerminalSnapshot, actual: TerminalSnapshot, phase: &'static str) -> Self {
        Self { expected, actual, phase }
    }

    pub(super) fn expected(self) -> TerminalSnapshot {
        self.expected
    }

    pub(super) fn actual(self) -> TerminalSnapshot {
        self.actual
    }

    pub(super) fn phase(self) -> &'static str {
        self.phase
    }
}

impl fmt::Display for TerminalResizeDuringDraw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "terminal resized during inline chat draw at {}: expected {:?}, actual {:?}",
            self.phase, self.expected, self.actual
        )
    }
}

impl Error for TerminalResizeDuringDraw {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryBatchKind {
    Normal,
    Replay,
}

#[derive(Debug, Clone)]
pub(super) struct PendingHistoryBatch {
    pub(super) kind: HistoryBatchKind,
    pub(super) rows: RenderedHistoryRows,
    pub(super) confirm_ids: Vec<HistoryOutputId>,
    next_row: usize,
}

impl PendingHistoryBatch {
    pub(super) fn new(
        kind: HistoryBatchKind,
        rows: RenderedHistoryRows,
        confirm_ids: Vec<HistoryOutputId>,
    ) -> Self {
        Self { kind, rows, confirm_ids, next_row: 0 }
    }

    fn is_complete(&self) -> bool {
        self.next_row >= self.rows.len()
    }

    fn remaining_len(&self) -> usize {
        self.rows.remaining_len(self.next_row)
    }

    fn pending_ids(&self) -> impl Iterator<Item = &HistoryOutputId> {
        self.confirm_ids.iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ChatDrawRequest {
    pub requested_inline_height: u16,
    pub terminal: TerminalSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChatDrawOutcome {
    pub viewport_area: Rect,
    pub flushed_history: FlushedHistory,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct FlushedHistory {
    pub confirmed_ids: Vec<HistoryOutputId>,
    pub flushed_rows: usize,
    pub replay_complete: bool,
    pub replay_incomplete: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct InlineViewportState {
    pub(super) area: Option<Rect>,
    pub(super) owned_top: u16,
    pub(super) owned_bottom: u16,
}

impl InlineViewportState {
    fn new(owned_top: u16) -> Self {
        Self { owned_top, owned_bottom: owned_top, ..Self::default() }
    }

    fn height(self) -> u16 {
        self.area.map_or(0, |area| area.height)
    }
}

pub(super) struct ChatTerminal {
    terminal: Option<StdoutTerminal>,
    state: InlineViewportState,
    pending_history: VecDeque<PendingHistoryBatch>,
    replay: Option<ReplayState>,
    pending_native_clear_reason: Option<&'static str>,
}

#[derive(Debug)]
struct ReplayState {
    render_width: u16,
    pending_batches: VecDeque<PendingHistoryBatch>,
    confirm_ids: Vec<HistoryOutputId>,
}

impl ReplayState {
    fn new(
        render_width: u16,
        batches: Vec<PendingHistoryBatch>,
        confirm_ids: Vec<HistoryOutputId>,
    ) -> Self {
        Self {
            render_width: render_width.max(1),
            pending_batches: VecDeque::from(batches),
            confirm_ids,
        }
    }

    fn matches_width(&self, width: u16) -> bool {
        self.render_width == width.max(1)
    }

    fn pending_ids(&self) -> impl Iterator<Item = &HistoryOutputId> {
        self.confirm_ids.iter()
    }

    fn is_complete(&self) -> bool {
        self.pending_batches.is_empty()
    }
}

impl ChatTerminal {
    pub(super) fn new(owned_top: u16) -> Self {
        Self {
            terminal: None,
            state: InlineViewportState::new(owned_top),
            pending_history: VecDeque::new(),
            replay: None,
            pending_native_clear_reason: None,
        }
    }

    pub(super) fn reset_visible(&mut self) -> anyhow::Result<()> {
        self.reset_visible_with_reason("visible_reset")
    }

    pub(super) fn reset_purge_replay(&mut self, reason: &'static str) -> anyhow::Result<()> {
        let (terminal_width, terminal_height) = crossterm::terminal::size()
            .context("failed to read terminal size before purge replay")?;
        let mut stdout = std::io::stdout();
        stdout
            .write_all(PURGE_REPLAY_CLEAR_ANSI.as_bytes())
            .context("failed to queue purge replay clear")?;
        stdout.flush().context("failed to flush purge replay clear")?;

        self.reset_after_purge_replay_clear(terminal_width, terminal_height);
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_purge_replay_cleared",
            message = "terminal scrollback and visible screen cleared before transcript replay",
            outcome = "success",
            reason,
            terminal_width = terminal_width.max(1),
            terminal_height = terminal_height.max(1),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
        );
        Ok(())
    }

    fn reset_visible_with_reason(&mut self, reason: &'static str) -> anyhow::Result<()> {
        let cleared_area = self.clear_owned_region(reason)?;
        self.reset_after_owned_region_clear(cleared_area);
        Ok(())
    }

    fn reset_visible_state(&mut self) {
        self.terminal = None;
        self.pending_history.clear();
        self.replay = None;
        self.pending_native_clear_reason = None;
    }

    fn reset_after_owned_region_clear(&mut self, cleared_area: Option<Rect>) {
        self.reset_visible_state();
        if let Some(area) = cleared_area {
            self.anchor_next_viewport_to_cleared_region(area);
        } else {
            self.state.area = None;
        }
    }

    fn reset_after_purge_replay_clear(&mut self, terminal_width: u16, terminal_height: u16) {
        let screen_height = terminal_height.max(1);
        self.terminal = None;
        self.pending_history.clear();
        self.replay = None;
        self.pending_native_clear_reason = None;
        self.state.owned_top = 0;
        self.state.owned_bottom = screen_height;
        self.state.area = Some(Rect::new(0, 0, terminal_width.max(1), 1));
    }

    pub(super) fn reset_mutable_viewport(&mut self) -> anyhow::Result<()> {
        self.clear_inline_terminal_viewport("mutable_reset")?;
        self.terminal = None;
        self.pending_native_clear_reason = None;
        Ok(())
    }

    pub(super) fn ensure_line_wrap_disabled(disabled: &mut bool) -> anyhow::Result<()> {
        if *disabled {
            return Ok(());
        }

        let mut stdout = std::io::stdout();
        queue!(stdout, DisableLineWrap).context("failed to disable inline viewport line wrap")?;
        stdout.flush().context("failed to flush line-wrap disable")?;
        *disabled = true;
        Ok(())
    }

    pub(super) fn queue_static_history(&mut self, batch: PendingHistoryBatch) {
        if self.replay.is_some() {
            tracing::debug!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_static_insert_deferred",
                message = "static transcript insert left unqueued while replay is active",
                outcome = "deferred",
                queued_rows = batch.rows.len(),
                queued_ids = batch.confirm_ids.len(),
            );
            return;
        }

        self.pending_history.push_back(batch);
    }

    pub(super) fn start_replay(
        &mut self,
        render_width: u16,
        batches: Vec<PendingHistoryBatch>,
        confirm_ids: Vec<HistoryOutputId>,
    ) {
        self.pending_history.clear();
        self.replay = Some(ReplayState::new(render_width, batches, confirm_ids));
    }

    pub(super) fn replay_pending_ids(&self) -> Vec<HistoryOutputId> {
        self.replay
            .as_ref()
            .map(|replay| replay.pending_ids().cloned().collect())
            .unwrap_or_default()
    }

    pub(super) fn pending_history_ids(&self) -> Vec<HistoryOutputId> {
        let mut ids = self
            .pending_history
            .iter()
            .flat_map(PendingHistoryBatch::pending_ids)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(replay) = &self.replay {
            ids.extend(replay.pending_ids().cloned());
        }
        ids
    }

    pub(super) fn is_replay_active(&self) -> bool {
        self.replay.is_some()
    }

    pub(super) fn active_replay_matches_width(&self, width: u16) -> bool {
        self.replay.as_ref().is_some_and(|replay| replay.matches_width(width))
    }

    pub(super) fn cancel_replay(&mut self) {
        if self.replay.take().is_some() {
            tracing::debug!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_replay_cancelled",
                message = "active transcript replay cancelled before confirmation",
                outcome = "cancelled",
            );
        }
    }

    pub(super) fn draw_chat_frame<F>(
        &mut self,
        chat_frame: ChatDrawRequest,
        render_mutable: F,
    ) -> anyhow::Result<ChatDrawOutcome>
    where
        F: FnOnce(&mut ratatui::Frame<'_>, Rect),
    {
        let mut stdout = std::io::stdout();
        stdout
            .sync_update(|_| -> anyhow::Result<ChatDrawOutcome> {
                let snapshot = chat_frame.terminal;
                ensure_terminal_snapshot(snapshot, "draw_start")?;
                let geometry_plan = plan_inline_geometry(
                    self.state.area,
                    chat_frame.requested_inline_height,
                    snapshot.width,
                    snapshot.height,
                );
                log_inline_geometry_plan(&geometry_plan);

                self.state.area = geometry_plan.old_area;
                self.apply_inline_geometry_scroll(&geometry_plan, snapshot)?;
                self.ensure_inline_terminal_height(
                    geometry_plan.height,
                    geometry_plan.target_area,
                    snapshot,
                )?;
                let flushed_history = self.flush_queued_history(snapshot)?;
                let viewport_area = self.draw_mutable_viewport(snapshot, render_mutable)?;

                self.state.area = Some(viewport_area);
                self.mark_area_owned(viewport_area, snapshot.height, "viewport_draw");
                tracing::debug!(
                    target: crate::logging::targets::APP_RENDER,
                    event_name = "inline_chat_terminal_draw_transaction",
                    message = "chat terminal owner completed draw transaction",
                    outcome = "success",
                    requested_height = chat_frame.requested_inline_height,
                    final_height = viewport_area.height,
                    viewport_top = viewport_area.top(),
                    owned_top = self.state.owned_top,
                    owned_bottom = self.state.owned_bottom,
                    flushed_rows = flushed_history.flushed_rows,
                    confirmed_ids = flushed_history.confirmed_ids.len(),
                    replay_complete = flushed_history.replay_complete,
                    replay_incomplete = flushed_history.replay_incomplete,
                );

                Ok(ChatDrawOutcome { viewport_area, flushed_history })
            })
            .context("failed synchronized inline chat terminal update")?
    }

    pub(super) fn can_insert_scrollback_rows(
        &self,
        chat_frame: ChatDrawRequest,
        row_count: usize,
    ) -> bool {
        if row_count == 0 {
            return true;
        }

        let Some(viewport_area) = self.predicted_viewport_after_ensure(chat_frame) else {
            return false;
        };
        let inserted_rows = u16::try_from(row_count).unwrap_or(u16::MAX);
        let plan = plan_owned_insert(
            viewport_area,
            self.state.owned_top,
            inserted_rows,
            chat_frame.terminal.height,
        );

        !matches!(plan.action, ScrollbackInsertAction::RebuildVisibleRows)
    }

    fn predicted_viewport_after_ensure(&self, chat_frame: ChatDrawRequest) -> Option<Rect> {
        let geometry_plan = plan_inline_geometry(
            self.state.area,
            chat_frame.requested_inline_height,
            chat_frame.terminal.width,
            chat_frame.terminal.height,
        );
        let anchor = geometry_plan.target_area.or(self.state.area)?;
        let screen_height = chat_frame.terminal.height;

        Some(Rect::new(
            0,
            inline_viewport_top_after_create(anchor.y, geometry_plan.height, screen_height),
            chat_frame.terminal.width,
            geometry_plan.height,
        ))
    }

    fn ensure_inline_terminal_height(
        &mut self,
        desired_height: u16,
        anchor_area: Option<Rect>,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<()> {
        ensure_terminal_snapshot(snapshot, "ensure_inline_terminal_height")?;
        let screen_height = snapshot.height;
        let next_height = desired_height.max(1).min(screen_height);
        let current_anchor = self.state.area;
        let anchor_changed = anchor_area.zip(current_anchor).is_some_and(|(next, current)| {
            next.x != current.x || next.y != current.y || next.width != current.width
        });
        if self.terminal.is_some() && self.state.height() == next_height && !anchor_changed {
            return Ok(());
        }

        let cursor_before = crossterm::cursor::position()
            .context("failed to read cursor before inline terminal ensure")?;
        let anchor = anchor_area.or(self.state.area);
        let cursor_y = anchor.map_or(cursor_before.1, |area| area.y);
        let predicted_area = Rect::new(
            0,
            inline_viewport_top_after_create(cursor_y, next_height, screen_height),
            snapshot.width,
            next_height,
        );

        if self.terminal.is_some()
            && let Some(area) = current_anchor
        {
            self.clear_owned_area(
                inline_viewport_reconfigure_clear_area(area, predicted_area),
                "viewport_reconfigure",
                snapshot,
            )?;
        }

        if let Some(area) = anchor {
            move_cursor_to(area).context("failed to restore inline viewport anchor")?;
        }

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_terminal_ensure",
            message = "inline terminal viewport ensured inside owned session region",
            outcome = "prepared",
            cursor_before_x = cursor_before.0,
            cursor_before_y = cursor_before.1,
            anchor_top = anchor.map(Rect::top),
            requested_height = desired_height,
            final_height = next_height,
            predicted_top = predicted_area.top(),
            predicted_bottom = predicted_area.bottom(),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
        );

        let terminal = create_inline_terminal(next_height)?;
        self.track_owned_region_scroll(
            inline_viewport_scroll_rows_after_create(cursor_y, next_height, screen_height),
            screen_height,
            "inline_terminal_ensure",
        );
        self.terminal = Some(terminal);
        self.state.area = Some(predicted_area);
        self.mark_area_owned(predicted_area, screen_height, "inline_terminal_ensure");
        self.clear_ratatui_inline_viewport("inline_terminal_ensure", Some(snapshot))?;
        Ok(())
    }

    fn apply_inline_geometry_scroll(
        &mut self,
        geometry_plan: &InlineGeometryPlan,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<()> {
        if geometry_plan.scroll_rows_before_resize == 0 {
            return Ok(());
        }

        let area_after_scroll = geometry_plan.old_area_after_scroll.ok_or_else(|| {
            anyhow!("inline viewport resize scroll missing shifted area: plan={geometry_plan:?}")
        })?;
        self.append_blank_lines_for_owned_region(
            geometry_plan.scroll_rows_before_resize,
            snapshot,
        )?;
        self.state.area = Some(area_after_scroll);
        self.terminal = None;

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_resize_scroll_applied",
            message = "terminal scrolled before expanding inline viewport",
            outcome = "success",
            rows = geometry_plan.scroll_rows_before_resize,
            old_top = geometry_plan.old_area.map(Rect::top),
            shifted_top = area_after_scroll.top(),
            target_top = geometry_plan.target_area.map(Rect::top),
            target_height = geometry_plan.height,
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
        );

        Ok(())
    }

    fn clear_inline_terminal_viewport(&mut self, reason: &'static str) -> anyhow::Result<()> {
        if self.clear_ratatui_inline_viewport(reason, None)? {
            return Ok(());
        }

        let Some(area) = self.state.area else {
            return Ok(());
        };
        let snapshot = TerminalSnapshot::current("failed to read terminal size before clear")?;
        self.clear_owned_area(area, reason, snapshot)
    }

    fn clear_owned_region(&mut self, reason: &'static str) -> anyhow::Result<Option<Rect>> {
        let (terminal_width, terminal_height) =
            crossterm::terminal::size().context("failed to read terminal size before clear")?;
        let top = self.state.owned_top.min(terminal_height);
        let bottom = self.state.owned_bottom.min(terminal_height);
        if top >= bottom {
            return Ok(None);
        }

        let area = Rect::new(0, top, terminal_width.max(1), bottom - top);
        self.clear_owned_area(
            area,
            reason,
            TerminalSnapshot::new(terminal_width, terminal_height),
        )?;
        Ok(Some(area))
    }

    fn draw_mutable_viewport<F>(
        &mut self,
        snapshot: TerminalSnapshot,
        render_mutable: F,
    ) -> anyhow::Result<Rect>
    where
        F: FnOnce(&mut ratatui::Frame<'_>, Rect),
    {
        if let Some(reason) = self.pending_native_clear_reason {
            self.clear_ratatui_inline_viewport(reason, Some(snapshot))?;
        }

        ensure_terminal_snapshot(snapshot, "draw_mutable_viewport")?;
        let terminal = self
            .terminal
            .as_mut()
            .ok_or_else(|| anyhow!("inline chat terminal missing before draw"))?;
        let mut viewport_area = Rect::new(0, 0, 0, 0);
        terminal
            .draw(|frame| {
                viewport_area = frame.area();
                frame.render_widget(RatatuiClear, viewport_area);
                render_mutable(frame, viewport_area);
            })
            .context("failed to draw ratatui inline chat viewport")?;
        ensure_terminal_snapshot(snapshot, "draw_mutable_viewport_complete")?;
        ensure_area_inside_snapshot(viewport_area, snapshot, "draw_mutable_viewport_area")?;

        Ok(viewport_area)
    }

    fn request_native_inline_clear(&mut self, reason: &'static str) {
        self.pending_native_clear_reason = Some(reason);
    }

    fn clear_ratatui_inline_viewport(
        &mut self,
        reason: &'static str,
        expected_snapshot: Option<TerminalSnapshot>,
    ) -> anyhow::Result<bool> {
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(false);
        };
        if let Some(snapshot) = expected_snapshot {
            ensure_terminal_snapshot(snapshot, "ratatui_inline_clear_start")?;
        }
        let terminal_size =
            terminal.size().context("failed to read terminal size before ratatui inline clear")?;
        let terminal_snapshot = TerminalSnapshot::new(terminal_size.width, terminal_size.height);
        if let Some(snapshot) = expected_snapshot
            && terminal_snapshot != snapshot
        {
            return Err(TerminalResizeDuringDraw::new(
                snapshot,
                terminal_snapshot,
                "ratatui_inline_clear_size",
            )
            .into());
        }
        terminal.clear().context("failed to clear ratatui inline viewport")?;
        if let Some(snapshot) = expected_snapshot {
            ensure_terminal_snapshot(snapshot, "ratatui_inline_clear_complete")?;
        }
        self.pending_native_clear_reason = None;
        if let Some(area) = self.state.area {
            self.state.owned_top =
                self.state.owned_top.min(area.top().min(terminal_snapshot.height));
            self.state.owned_bottom = terminal_snapshot.height;
        }

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_native_viewport_cleared",
            message = "ratatui inline viewport cleared before mutable redraw",
            outcome = "success",
            reason,
            viewport_top = self.state.area.map(Rect::top),
            viewport_bottom = self.state.area.map(Rect::bottom),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
        );

        Ok(true)
    }

    fn flush_queued_history(
        &mut self,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<FlushedHistory> {
        if self.replay.is_some() {
            return self.flush_replay_history(snapshot);
        }

        let mut flushed = FlushedHistory::default();
        while let Some(mut batch) = self.pending_history.pop_front() {
            flushed.flushed_rows = flushed
                .flushed_rows
                .saturating_add(self.insert_history_batch(&mut batch, snapshot)?);

            if batch.is_complete() {
                flushed.confirmed_ids.extend(batch.confirm_ids);
            } else {
                self.pending_history.push_front(batch);
                break;
            }
        }
        Ok(flushed)
    }

    fn flush_replay_history(
        &mut self,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<FlushedHistory> {
        if self.replay.is_none() {
            return Ok(FlushedHistory::default());
        }

        let budget = replay_batch_row_budget(snapshot.height);
        let mut flushed_rows = 0usize;
        let mut drained_rows = 0usize;
        while let Some(mut batch) =
            self.replay.as_mut().and_then(|replay| replay.pending_batches.pop_front())
        {
            let target_rows = batch.remaining_len();
            flushed_rows =
                flushed_rows.saturating_add(self.insert_history_batch(&mut batch, snapshot)?);
            drained_rows =
                drained_rows.saturating_add(target_rows.saturating_sub(batch.remaining_len()));

            if !batch.is_complete() {
                if let Some(replay) = self.replay.as_mut() {
                    replay.pending_batches.push_front(batch);
                }
                break;
            }

            if drained_rows >= budget {
                break;
            }
        }

        let replay_complete = self.replay.as_ref().is_some_and(ReplayState::is_complete);
        let confirmed_ids = if replay_complete {
            self.replay.as_ref().map_or_else(Vec::new, |replay| replay.confirm_ids.clone())
        } else {
            Vec::new()
        };
        if replay_complete {
            self.replay = None;
        }

        Ok(FlushedHistory {
            confirmed_ids,
            flushed_rows,
            replay_complete,
            replay_incomplete: !replay_complete,
        })
    }

    fn insert_history_batch(
        &mut self,
        batch: &mut PendingHistoryBatch,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<usize> {
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_history_insert_request",
            message = "history rows scheduled for ratatui inline insertion",
            outcome = "prepared",
            rows = batch.rows.len(),
            remaining_rows = batch.remaining_len(),
            confirm_ids = batch.confirm_ids.len(),
            batch_kind = ?batch.kind,
        );

        if batch.rows.is_empty() || batch.is_complete() {
            batch.next_row = batch.rows.len();
            return Ok(0);
        }

        if batch.rows.width != snapshot.width {
            bail!(
                "refusing inline history insert rendered at stale width: rendered_width={}, terminal_width={}, rows={}",
                batch.rows.width,
                snapshot.width,
                batch.rows.len()
            );
        }

        let mut inserted_rows = 0usize;
        while !batch.is_complete() {
            let remaining_rows = u16::try_from(batch.remaining_len()).unwrap_or(u16::MAX);
            let plan = self.prepare_scrollback_insert_plan(remaining_rows, snapshot)?;
            if matches!(plan.action, ScrollbackInsertAction::RebuildVisibleRows) {
                tracing::debug!(
                    target: crate::logging::targets::APP_RENDER,
                    event_name = "inline_chat_history_insert_deferred",
                    message = "history batch remains pending because the inline viewport cannot be moved safely",
                    outcome = "deferred",
                    rows_remaining = batch.remaining_len(),
                    inserted_rows,
                    viewport = ?self.state.area,
                    terminal_height = snapshot.height,
                    max_insert_rows = plan.max_insert_rows_for_viewport,
                    batch_kind = ?batch.kind,
                );
                return Ok(inserted_rows);
            }

            let chunk_len = usize::from(plan.inserted_rows);
            if chunk_len == 0 {
                bail!(
                    "refusing inline history insert without a safe chunk: rows_remaining={}, viewport={:?}, terminal_height={}",
                    batch.remaining_len(),
                    self.state.area,
                    snapshot.height
                );
            }

            let start = batch.next_row;
            let end = start.saturating_add(chunk_len).min(batch.rows.len());
            self.insert_scrollback_chunk(batch.rows.slice(start..end), plan, snapshot)?;
            batch.next_row = end;
            inserted_rows = inserted_rows.saturating_add(end.saturating_sub(start));
        }

        Ok(inserted_rows)
    }

    fn prepare_scrollback_insert_plan(
        &self,
        row_count: u16,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<ScrollbackInsertPlan> {
        let area = self.state.area.ok_or_else(|| {
            anyhow!("inline chat viewport missing before scrollback insert guard")
        })?;
        let plan = plan_owned_insert(area, self.state.owned_top, row_count, snapshot.height);

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_scrollback_insert_ownership_plan",
            message = "scrollback insert checked against owned terminal region",
            outcome = ?plan.action,
            rows = row_count,
            planned_rows = plan.inserted_rows,
            viewport_top = area.top(),
            viewport_bottom = area.bottom(),
            viewport_after_scroll_top = plan.viewport_after_scroll.top(),
            viewport_after_insert_top = plan.viewport_after_insert.top(),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
            terminal_height = snapshot.height,
            available_below = plan.available_below,
            max_insert_rows_for_viewport = plan.max_insert_rows_for_viewport,
            scroll_rows_before_insert = plan.scroll_rows_before_insert,
        );

        Ok(plan)
    }

    fn insert_scrollback_chunk(
        &mut self,
        rows: &[Line<'static>],
        plan: ScrollbackInsertPlan,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<()> {
        ensure_terminal_snapshot(snapshot, "scrollback_insert_start")?;
        if plan.scroll_rows_before_insert > 0 {
            self.append_blank_lines_for_owned_region(plan.scroll_rows_before_insert, snapshot)?;
            self.state.area = Some(plan.viewport_after_scroll);
            self.terminal = None;
            self.ensure_inline_terminal_height(
                plan.viewport_after_scroll.height,
                Some(plan.viewport_after_scroll),
                snapshot,
            )?;
        }
        ensure_terminal_snapshot(snapshot, "scrollback_insert_before_insert")?;

        let before_area = self
            .state
            .area
            .ok_or_else(|| anyhow!("inline chat viewport missing before scrollback insert"))?;
        if before_area != plan.viewport_after_scroll {
            bail!(
                "inline scrollback insert pre-scroll mismatch: expected={:?}, actual={:?}, rows={}",
                plan.viewport_after_scroll,
                before_area,
                plan.inserted_rows
            );
        }

        let rendered_rows = rows.to_vec();
        let terminal = self
            .terminal
            .as_mut()
            .ok_or_else(|| anyhow!("inline chat terminal missing before scrollback insert"))?;
        terminal
            .insert_before(plan.inserted_rows, |buffer| {
                Paragraph::new(rendered_rows).render(buffer.area, buffer);
            })
            .context("failed to insert committed chat rows above inline viewport")?;
        ensure_terminal_snapshot(snapshot, "scrollback_insert_complete")?;
        self.request_native_inline_clear("scrollback_insert");

        let after_area =
            viewport_area_after_insert_exact(before_area, plan.inserted_rows, snapshot.height)
                .ok_or_else(|| {
                    anyhow!(
                        "inline scrollback insert could not move viewport exactly: viewport={:?}, rows={}, terminal_height={}",
                        before_area,
                        plan.inserted_rows,
                        snapshot.height
                    )
                })?;
        if after_area != plan.viewport_after_insert {
            bail!(
                "inline scrollback insert postcondition mismatch: expected={:?}, actual={:?}, rows={}",
                plan.viewport_after_insert,
                after_area,
                plan.inserted_rows
            );
        }

        self.state.area = Some(after_area);
        self.mark_area_owned(
            Rect::new(
                0,
                before_area.top().min(after_area.top()),
                snapshot.width,
                after_area.bottom().saturating_sub(before_area.top().min(after_area.top())),
            ),
            snapshot.height,
            "scrollback_insert",
        );
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_scrollback_insert_applied",
            message = "stable chat rows inserted before ratatui inline viewport",
            outcome = "success",
            inserted_rows = plan.inserted_rows,
            scroll_rows_before_insert = plan.scroll_rows_before_insert,
            viewport_top_before = plan.viewport_before.top(),
            viewport_top_after_scroll = before_area.top(),
            viewport_top_after = after_area.top(),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
            insert_action = ?plan.action,
        );

        Ok(())
    }

    fn append_blank_lines_for_owned_region(
        &mut self,
        rows: u16,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<()> {
        if rows == 0 {
            return Ok(());
        }

        ensure_terminal_snapshot(snapshot, "owned_region_growth_start")?;
        let bottom = snapshot.height.saturating_sub(1);
        let mut stdout = std::io::stdout();
        queue!(stdout, MoveTo(0, bottom)).context("failed to move cursor before owned growth")?;
        for _ in 0..rows {
            queue!(stdout, Print("\r\n")).context("failed to queue owned growth newline")?;
        }
        stdout.flush().context("failed to flush owned growth newlines")?;
        ensure_terminal_snapshot(snapshot, "owned_region_growth_complete")?;
        self.request_native_inline_clear("owned_region_growth");

        let old_top = self.state.owned_top;
        let old_bottom = self.state.owned_bottom;
        self.state.owned_top = self.state.owned_top.saturating_sub(rows);
        self.state.owned_bottom = snapshot.height;
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_owned_region_expanded",
            message = "inline chat owned region expanded by appending terminal lines",
            outcome = "success",
            rows,
            old_owned_top = old_top,
            old_owned_bottom = old_bottom,
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
            terminal_height = snapshot.height,
        );

        Ok(())
    }

    fn track_owned_region_scroll(&mut self, rows: u16, terminal_height: u16, reason: &'static str) {
        if rows == 0 || self.state.owned_top == self.state.owned_bottom {
            return;
        }

        let old_top = self.state.owned_top;
        let old_bottom = self.state.owned_bottom;
        self.state.owned_top = self.state.owned_top.saturating_sub(rows).min(terminal_height);
        self.state.owned_bottom = self.state.owned_bottom.saturating_sub(rows).min(terminal_height);
        self.state.area = self.state.area.map(|area| shift_area_up(area, rows));
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_owned_region_scrolled",
            message = "inline chat ownership range shifted after terminal scroll",
            outcome = "success",
            reason,
            rows,
            old_owned_top = old_top,
            old_owned_bottom = old_bottom,
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
            terminal_height,
        );
    }

    fn anchor_next_viewport_to_cleared_region(&mut self, area: Rect) {
        self.state.area = Some(Rect::new(area.x, area.y, area.width.max(1), 1));
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_session_boundary_anchor_reset",
            message = "next inline viewport anchored to top of cleared session region",
            outcome = "success",
            anchor_top = area.top(),
            anchor_bottom = area.top().saturating_add(1),
            cleared_top = area.top(),
            cleared_bottom = area.bottom(),
        );
    }

    fn clear_owned_area(
        &mut self,
        area: Rect,
        reason: &'static str,
        snapshot: TerminalSnapshot,
    ) -> anyhow::Result<()> {
        if area.is_empty() {
            return Ok(());
        }

        ensure_terminal_snapshot(snapshot, "clear_owned_area")?;
        let visible_bottom = self.state.owned_bottom.min(snapshot.height);
        if area.top() < self.state.owned_top || area.bottom() > visible_bottom {
            bail!(
                "refusing to clear outside inline chat owned region: clear={:?}, owned={}..{}, terminal_height={}",
                area,
                self.state.owned_top,
                self.state.owned_bottom,
                snapshot.height
            );
        }

        let mut stdout = std::io::stdout();
        for y in area.top()..area.bottom() {
            queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))
                .context("failed to queue bounded inline clear")?;
        }
        stdout.flush().context("failed to flush bounded inline clear")?;
        if self.terminal.is_some() {
            self.request_native_inline_clear(reason);
        }

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_owned_region_cleared",
            message = "inline chat rows cleared inside owned terminal region",
            outcome = "success",
            reason,
            clear_top = area.top(),
            clear_bottom = area.bottom(),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
        );

        Ok(())
    }

    fn mark_area_owned(&mut self, area: Rect, terminal_height: u16, reason: &'static str) {
        if area.is_empty() {
            return;
        }

        let old_top = self.state.owned_top;
        let old_bottom = self.state.owned_bottom;
        self.state.owned_top = self.state.owned_top.min(area.top().min(terminal_height));
        self.state.owned_bottom = self.state.owned_bottom.max(area.bottom().min(terminal_height));
        if self.state.owned_top == old_top && self.state.owned_bottom == old_bottom {
            return;
        }

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_owned_region_marked",
            message = "inline chat terminal ownership range updated",
            outcome = "success",
            reason,
            area_top = area.top(),
            area_bottom = area.bottom(),
            old_owned_top = old_top,
            old_owned_bottom = old_bottom,
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
            terminal_height,
        );
    }
}

fn create_inline_terminal(height: u16) -> anyhow::Result<StdoutTerminal> {
    Terminal::with_options(
        CrosstermBackend::new(std::io::stdout()),
        TerminalOptions { viewport: Viewport::Inline(height.max(1)) },
    )
    .context("failed to construct ratatui inline chat terminal")
}

fn move_cursor_to(area: Rect) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout();
    queue!(stdout, MoveTo(area.x, area.y)).context("failed to queue cursor move")?;
    stdout.flush().context("failed to flush cursor move")?;
    Ok(())
}

fn ensure_terminal_snapshot(expected: TerminalSnapshot, phase: &'static str) -> anyhow::Result<()> {
    let actual = TerminalSnapshot::current("failed to read terminal size during inline draw")?;
    if actual != expected {
        return Err(TerminalResizeDuringDraw::new(expected, actual, phase).into());
    }
    Ok(())
}

fn ensure_area_inside_snapshot(
    area: Rect,
    snapshot: TerminalSnapshot,
    phase: &'static str,
) -> anyhow::Result<()> {
    if area.right() > snapshot.width || area.bottom() > snapshot.height {
        bail!(
            "inline chat viewport area outside terminal snapshot at {phase}: area={area:?}, snapshot={snapshot:?}",
        );
    }
    Ok(())
}

fn replay_batch_row_budget(terminal_height: u16) -> usize {
    usize::from(terminal_height)
        .saturating_mul(4)
        .clamp(MIN_REPLAY_BATCH_ROWS, MAX_REPLAY_BATCH_ROWS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineGeometryPlan {
    pub(crate) old_area: Option<Rect>,
    pub(crate) old_area_after_scroll: Option<Rect>,
    pub(crate) target_area: Option<Rect>,
    pub(crate) height: u16,
    pub(crate) scroll_rows_before_resize: u16,
}

pub(crate) fn plan_inline_geometry(
    last_frame_area: Option<Rect>,
    desired_height: u16,
    terminal_width: u16,
    terminal_height: u16,
) -> InlineGeometryPlan {
    let screen_height = terminal_height.max(1);
    let screen_width = terminal_width.max(1);
    let height = desired_height.max(1).min(screen_height);
    let previous_area = last_frame_area.filter(|area| !area.is_empty());
    let old_area =
        previous_area.and_then(|area| clip_rect_to_terminal(area, screen_width, screen_height));
    let scroll_rows_before_resize =
        old_area.filter(|area| height > area.height).map_or(0, |area| {
            let growth = height.saturating_sub(area.height);
            let available_below = screen_height.saturating_sub(area.bottom());
            growth.saturating_sub(available_below)
        });
    let old_area_after_scroll = old_area.map(|area| shift_area_up(area, scroll_rows_before_resize));
    let anchor_area = old_area_after_scroll
        .or_else(|| previous_area.map(|area| Rect::new(0, area.y, screen_width, height)));
    let target_area = anchor_area.map(|area| {
        let max_top = screen_height.saturating_sub(height);
        Rect::new(0, area.y.min(max_top), screen_width, height)
    });
    InlineGeometryPlan {
        old_area,
        old_area_after_scroll,
        target_area,
        height,
        scroll_rows_before_resize,
    }
}

fn inline_viewport_top_after_create(cursor_y: u16, height: u16, terminal_height: u16) -> u16 {
    let screen_height = terminal_height.max(1);
    let row = cursor_y.min(screen_height.saturating_sub(1));
    let lines_after_cursor = height.max(1).saturating_sub(1);
    let available_lines = screen_height.saturating_sub(row).saturating_sub(1);
    row.saturating_sub(lines_after_cursor.saturating_sub(available_lines))
}

fn inline_viewport_scroll_rows_after_create(
    cursor_y: u16,
    height: u16,
    terminal_height: u16,
) -> u16 {
    cursor_y
        .min(terminal_height.max(1).saturating_sub(1))
        .saturating_sub(inline_viewport_top_after_create(cursor_y, height, terminal_height))
}

fn shift_area_up(area: Rect, rows: u16) -> Rect {
    Rect::new(area.x, area.y.saturating_sub(rows), area.width, area.height)
}

fn clip_rect_to_terminal(area: Rect, terminal_width: u16, terminal_height: u16) -> Option<Rect> {
    if area.is_empty() || area.x >= terminal_width || area.y >= terminal_height {
        return None;
    }

    let right = area.right().min(terminal_width);
    let bottom = area.bottom().min(terminal_height);
    let width = right.saturating_sub(area.x);
    let height = bottom.saturating_sub(area.y);
    let clipped = Rect::new(area.x, area.y, width, height);
    (!clipped.is_empty()).then_some(clipped)
}

fn inline_viewport_reconfigure_clear_area(current_area: Rect, predicted_area: Rect) -> Rect {
    union_rect(current_area, predicted_area)
}

fn union_rect(first: Rect, second: Rect) -> Rect {
    let left = first.x.min(second.x);
    let top = first.y.min(second.y);
    let right = first.x.saturating_add(first.width).max(second.x.saturating_add(second.width));
    let bottom = first.y.saturating_add(first.height).max(second.y.saturating_add(second.height));
    Rect::new(left, top, right.saturating_sub(left), bottom.saturating_sub(top))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScrollbackInsertPlan {
    inserted_rows: u16,
    scroll_rows_before_insert: u16,
    viewport_before: Rect,
    viewport_after_scroll: Rect,
    viewport_after_insert: Rect,
    available_below: u16,
    max_insert_rows_for_viewport: u16,
    action: ScrollbackInsertAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScrollbackInsertAction {
    Insert,
    SplitBatch,
    RebuildVisibleRows,
}

fn plan_owned_insert(
    viewport_area: Rect,
    owned_top: u16,
    inserted_rows: u16,
    terminal_height: u16,
) -> ScrollbackInsertPlan {
    let screen_height = terminal_height.max(1);
    let empty_area = Rect::new(viewport_area.x, viewport_area.y, viewport_area.width, 0);
    if viewport_area.is_empty()
        || viewport_area.top() < owned_top
        || viewport_area.bottom() > screen_height
    {
        return ScrollbackInsertPlan {
            inserted_rows: 0,
            scroll_rows_before_insert: 0,
            viewport_before: viewport_area,
            viewport_after_scroll: viewport_area,
            viewport_after_insert: empty_area,
            available_below: 0,
            max_insert_rows_for_viewport: 0,
            action: ScrollbackInsertAction::RebuildVisibleRows,
        };
    }

    let available_below = screen_height.saturating_sub(viewport_area.bottom());
    let max_insert_rows_for_viewport = screen_height.saturating_sub(viewport_area.height);
    let planned_rows = inserted_rows.min(max_insert_rows_for_viewport);
    if planned_rows == 0 {
        return ScrollbackInsertPlan {
            inserted_rows: 0,
            scroll_rows_before_insert: 0,
            viewport_before: viewport_area,
            viewport_after_scroll: viewport_area,
            viewport_after_insert: empty_area,
            available_below,
            max_insert_rows_for_viewport,
            action: ScrollbackInsertAction::RebuildVisibleRows,
        };
    }

    let scroll_rows_before_insert = planned_rows.saturating_sub(available_below);
    if scroll_rows_before_insert > viewport_area.top() {
        return ScrollbackInsertPlan {
            inserted_rows: 0,
            scroll_rows_before_insert: 0,
            viewport_before: viewport_area,
            viewport_after_scroll: viewport_area,
            viewport_after_insert: empty_area,
            available_below,
            max_insert_rows_for_viewport,
            action: ScrollbackInsertAction::RebuildVisibleRows,
        };
    }

    let viewport_after_scroll = shift_area_up(viewport_area, scroll_rows_before_insert);
    let Some(viewport_after_insert) =
        viewport_area_after_insert_exact(viewport_after_scroll, planned_rows, screen_height)
    else {
        return ScrollbackInsertPlan {
            inserted_rows: 0,
            scroll_rows_before_insert: 0,
            viewport_before: viewport_area,
            viewport_after_scroll,
            viewport_after_insert: empty_area,
            available_below,
            max_insert_rows_for_viewport,
            action: ScrollbackInsertAction::RebuildVisibleRows,
        };
    };

    let action = if inserted_rows > planned_rows {
        ScrollbackInsertAction::SplitBatch
    } else {
        ScrollbackInsertAction::Insert
    };

    ScrollbackInsertPlan {
        inserted_rows: planned_rows,
        scroll_rows_before_insert,
        viewport_before: viewport_area,
        viewport_after_scroll,
        viewport_after_insert,
        available_below,
        max_insert_rows_for_viewport,
        action,
    }
}

fn viewport_area_after_insert_exact(
    area: Rect,
    inserted_rows: u16,
    terminal_height: u16,
) -> Option<Rect> {
    let screen_height = terminal_height.max(1);
    if area.is_empty() || area.bottom() > screen_height {
        return None;
    }
    if inserted_rows > screen_height.saturating_sub(area.bottom()) {
        return None;
    }

    Some(Rect::new(area.x, area.y.saturating_add(inserted_rows), area.width, area.height))
}

fn log_inline_geometry_plan(plan: &InlineGeometryPlan) {
    if plan.old_area == plan.target_area {
        return;
    }

    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_geometry_reconciled",
        message = "inline viewport geometry reconciled before draw",
        outcome = "prepared",
        old_top = plan.old_area.map(Rect::top),
        old_height = plan.old_area.map(|area| area.height),
        shifted_top = plan.old_area_after_scroll.map(Rect::top),
        target_top = plan.target_area.map(Rect::top),
        target_height = plan.target_area.map(|area| area.height),
        scroll_rows_before_resize = plan.scroll_rows_before_resize,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        ChatDrawRequest, ChatTerminal, HistoryBatchKind, InlineViewportState, PendingHistoryBatch,
        RenderedHistoryRows, ScrollbackInsertAction, TerminalSnapshot,
        inline_viewport_reconfigure_clear_area, inline_viewport_scroll_rows_after_create,
        inline_viewport_top_after_create, plan_inline_geometry, plan_owned_insert, shift_area_up,
        viewport_area_after_insert_exact,
    };
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use ratatui::widgets::{Clear as RatatuiClear, Paragraph, Widget};
    use ratatui::{TerminalOptions, Viewport};

    fn trimmed_backend_lines(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(Cell::symbol).collect::<String>().trim_end().to_owned())
            .collect()
    }

    #[test]
    fn inline_viewport_state_height_derives_from_area() {
        let mut state = InlineViewportState::new(10);
        assert_eq!(state.height(), 0);

        state.area = Some(Rect::new(0, 20, 80, 5));

        assert_eq!(state.height(), 5);
    }

    #[test]
    fn area_helper_shifts_viewport_up_after_terminal_scroll() {
        let area = Rect::new(0, 28, 120, 5);

        assert_eq!(shift_area_up(area, 5), Rect::new(0, 23, 120, 5));
    }

    #[test]
    fn ratatui_inline_insert_preserves_unchanged_viewport_rows() {
        let backend = TestBackend::new(48, 8);
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(3) })
                .expect("terminal");

        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("prompt"),
                        Line::from("[Auto] [Opus]"),
                        Line::from("loc"),
                    ]),
                    frame.area(),
                );
            })
            .expect("initial draw");
        terminal
            .insert_before(3, |buffer| {
                Paragraph::new(vec![
                    Line::from("Error"),
                    Line::from("failed to set mode to auto"),
                    Line::default(),
                ])
                .render(buffer.area, buffer);
            })
            .expect("insert before");
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("prompt"),
                        Line::from("[Default] [Opus]"),
                        Line::from("loc"),
                    ]),
                    frame.area(),
                );
            })
            .expect("post-insert draw");

        let lines = trimmed_backend_lines(&terminal);

        assert!(lines.iter().any(|line| line == "Error"));
        assert!(lines.iter().any(|line| line == "failed to set mode to auto"));
        assert!(lines.windows(3).any(|rows| {
            rows[0] == "prompt" && rows[1] == "[Default] [Opus]" && rows[2] == "loc"
        }));
    }

    #[test]
    fn ratatui_inline_native_clear_after_insert_removes_shrunken_viewport_rows() {
        let backend = TestBackend::new(48, 8);
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(3) })
                .expect("terminal");

        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("live tool output that should disappear"),
                        Line::from("live detail that should disappear"),
                        Line::from("prompt"),
                    ]),
                    frame.area(),
                );
            })
            .expect("initial draw");
        terminal
            .insert_before(2, |buffer| {
                Paragraph::new(vec![
                    Line::from("static transcript row"),
                    Line::from("static transcript detail"),
                ])
                .render(buffer.area, buffer);
            })
            .expect("insert before");
        terminal.clear().expect("native inline clear");
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(RatatuiClear, area);
                frame.render_widget(Paragraph::new(vec![Line::from("prompt")]), area);
            })
            .expect("post-clear draw");

        let lines = trimmed_backend_lines(&terminal);

        assert!(lines.iter().any(|line| line == "static transcript row"));
        assert!(lines.iter().any(|line| line == "static transcript detail"));
        assert!(lines.iter().any(|line| line == "prompt"));
        assert!(!lines.iter().any(|line| line.contains("should disappear")));
    }

    #[test]
    fn owned_insert_plan_allows_insert_when_viewport_has_room_below() {
        let plan = plan_owned_insert(Rect::new(0, 22, 120, 5), 22, 9, 37);

        assert_eq!(plan.action, ScrollbackInsertAction::Insert);
        assert_eq!(plan.inserted_rows, 9);
        assert_eq!(plan.available_below, 10);
        assert_eq!(plan.scroll_rows_before_insert, 0);
        assert_eq!(plan.viewport_after_scroll, Rect::new(0, 22, 120, 5));
        assert_eq!(plan.viewport_after_insert, Rect::new(0, 31, 120, 5));
    }

    #[test]
    fn owned_insert_plan_expands_downward_before_insert_scrolls_above_owned_top() {
        let plan = plan_owned_insert(Rect::new(0, 28, 120, 5), 20, 9, 37);

        assert_eq!(plan.action, ScrollbackInsertAction::Insert);
        assert_eq!(plan.inserted_rows, 9);
        assert_eq!(plan.available_below, 4);
        assert_eq!(plan.scroll_rows_before_insert, 5);
        assert_eq!(plan.viewport_after_scroll, Rect::new(0, 23, 120, 5));
        assert_eq!(plan.viewport_after_insert, Rect::new(0, 32, 120, 5));
    }

    #[test]
    fn owned_insert_plan_splits_batches_larger_than_viewport_capacity() {
        let plan = plan_owned_insert(Rect::new(0, 28, 120, 5), 3, 40, 37);

        assert_eq!(plan.action, ScrollbackInsertAction::SplitBatch);
        assert_eq!(plan.inserted_rows, 32);
        assert_eq!(plan.max_insert_rows_for_viewport, 32);
        assert_eq!(plan.scroll_rows_before_insert, 28);
        assert_eq!(plan.viewport_after_scroll, Rect::new(0, 0, 120, 5));
        assert_eq!(plan.viewport_after_insert, Rect::new(0, 32, 120, 5));
    }

    #[test]
    fn owned_insert_plan_rebuilds_when_viewport_consumes_screen() {
        let plan = plan_owned_insert(Rect::new(0, 0, 120, 37), 0, 1, 37);

        assert_eq!(plan.action, ScrollbackInsertAction::RebuildVisibleRows);
        assert_eq!(plan.inserted_rows, 0);
        assert_eq!(plan.max_insert_rows_for_viewport, 0);
    }

    #[test]
    fn area_helpers_require_exact_viewport_movement_after_insert() {
        assert_eq!(
            viewport_area_after_insert_exact(Rect::new(0, 23, 120, 5), 9, 37),
            Some(Rect::new(0, 32, 120, 5))
        );
        assert_eq!(viewport_area_after_insert_exact(Rect::new(0, 28, 120, 6), 6, 37), None);
    }

    #[test]
    fn inline_viewport_scroll_rows_track_bottom_reconfigure_scroll() {
        assert_eq!(inline_viewport_scroll_rows_after_create(31, 8, 37), 2);
        assert_eq!(inline_viewport_scroll_rows_after_create(23, 11, 37), 0);
    }

    #[test]
    fn geometry_plan_scrolls_before_bottom_expansion_claims_transcript_rows() {
        let plan = plan_inline_geometry(Some(Rect::new(0, 32, 133, 4)), 8, 133, 37);

        assert_eq!(plan.scroll_rows_before_resize, 3);
        assert_eq!(plan.old_area_after_scroll, Some(Rect::new(0, 29, 133, 4)));
        assert_eq!(plan.target_area, Some(Rect::new(0, 29, 133, 8)));
    }

    #[test]
    fn geometry_plan_clips_stale_bottom_viewport_before_resize_clear() {
        let plan = plan_inline_geometry(Some(Rect::new(0, 32, 144, 8)), 8, 133, 37);

        assert_eq!(plan.old_area, Some(Rect::new(0, 32, 133, 5)));
        assert_eq!(plan.scroll_rows_before_resize, 3);
        assert_eq!(plan.old_area_after_scroll, Some(Rect::new(0, 29, 133, 5)));
        assert_eq!(plan.target_area, Some(Rect::new(0, 29, 133, 8)));
    }

    #[test]
    fn geometry_plan_expands_downward_when_room_exists_below_viewport() {
        let plan = plan_inline_geometry(Some(Rect::new(0, 22, 120, 4)), 8, 120, 37);

        assert_eq!(plan.scroll_rows_before_resize, 0);
        assert_eq!(plan.old_area_after_scroll, Some(Rect::new(0, 22, 120, 4)));
        assert_eq!(plan.target_area, Some(Rect::new(0, 22, 120, 8)));
    }

    #[test]
    fn reconfigure_clear_area_includes_shifted_old_rows_and_new_live_rows() {
        let clear_area = inline_viewport_reconfigure_clear_area(
            Rect::new(0, 29, 120, 4),
            Rect::new(0, 29, 120, 8),
        );

        assert_eq!(clear_area, Rect::new(0, 29, 120, 8));
    }

    #[test]
    fn reconfigure_clear_area_keeps_old_rows_when_viewport_shrinks() {
        let clear_area = inline_viewport_reconfigure_clear_area(
            Rect::new(0, 17, 120, 20),
            Rect::new(0, 17, 120, 4),
        );

        assert_eq!(clear_area, Rect::new(0, 17, 120, 20));
    }

    #[test]
    fn owned_region_tracks_inline_terminal_scroll() {
        let mut terminal = ChatTerminal::new(23);
        terminal.state.owned_bottom = 34;
        terminal.state.area = Some(Rect::new(0, 31, 120, 3));

        terminal.track_owned_region_scroll(5, 37, "test");

        assert_eq!(terminal.state.owned_top, 18);
        assert_eq!(terminal.state.owned_bottom, 29);
        assert_eq!(terminal.state.area, Some(Rect::new(0, 26, 120, 3)));
    }

    #[test]
    fn session_boundary_anchor_restarts_next_viewport_at_cleared_top() {
        let mut terminal = ChatTerminal::new(17);
        terminal.state.owned_bottom = 37;
        terminal.state.area = Some(Rect::new(0, 31, 120, 3));

        terminal.anchor_next_viewport_to_cleared_region(Rect::new(0, 17, 120, 20));

        assert_eq!(terminal.state.area, Some(Rect::new(0, 17, 120, 1)));

        let plan = plan_inline_geometry(terminal.state.area, 11, 120, 37);

        assert_eq!(plan.target_area, Some(Rect::new(0, 17, 120, 11)));
        assert_eq!(inline_viewport_top_after_create(17, plan.height, 37), 17);
    }

    #[test]
    fn visible_reset_anchor_restarts_next_viewport_at_cleared_top() {
        let mut terminal = ChatTerminal::new(17);
        terminal.state.owned_bottom = 37;
        terminal.state.area = Some(Rect::new(0, 31, 120, 3));

        terminal.reset_after_owned_region_clear(Some(Rect::new(0, 17, 120, 20)));

        assert_eq!(terminal.state.area, Some(Rect::new(0, 17, 120, 1)));

        let plan = plan_inline_geometry(terminal.state.area, 11, 120, 37);

        assert_eq!(plan.target_area, Some(Rect::new(0, 17, 120, 11)));
        assert_eq!(inline_viewport_top_after_create(17, plan.height, 37), 17);
    }

    #[test]
    fn geometry_plan_clamps_shrink_target_inside_terminal() {
        let plan = plan_inline_geometry(Some(Rect::new(0, 34, 120, 6)), 11, 80, 20);

        assert_eq!(plan.height, 11);
        assert_eq!(plan.old_area, None);
        assert_eq!(plan.target_area, Some(Rect::new(0, 9, 80, 11)));
    }

    #[test]
    fn purge_replay_uses_codex_clear_sequence() {
        assert_eq!(super::PURGE_REPLAY_CLEAR_ANSI, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H");
    }

    #[test]
    fn purge_replay_reset_anchors_viewport_at_top() {
        let mut terminal = ChatTerminal::new(12);
        terminal.state.owned_bottom = 29;
        terminal.state.area = Some(Rect::new(0, 17, 120, 6));

        terminal.reset_after_purge_replay_clear(120, 39);

        assert!(terminal.terminal.is_none());
        assert_eq!(terminal.state.owned_top, 0);
        assert_eq!(terminal.state.owned_bottom, 39);
        assert_eq!(terminal.state.area, Some(Rect::new(0, 0, 120, 1)));
    }

    #[test]
    fn scrollback_preflight_rejects_full_height_resize_replay_insert() {
        let mut terminal = ChatTerminal::new(0);
        terminal.reset_after_purge_replay_clear(124, 32);

        let can_insert = terminal.can_insert_scrollback_rows(
            ChatDrawRequest {
                requested_inline_height: 32,
                terminal: TerminalSnapshot::new(124, 32),
            },
            19,
        );

        assert!(!can_insert);
    }

    #[test]
    fn unsafe_pending_history_insert_is_deferred_without_error() {
        let mut terminal = ChatTerminal::new(0);
        terminal.state.owned_bottom = 32;
        terminal.state.area = Some(Rect::new(0, 0, 124, 32));
        let rows: Vec<Line<'static>> =
            (0..19).map(|idx| Line::from(format!("row {idx}"))).collect();
        let mut batch = PendingHistoryBatch::new(
            HistoryBatchKind::Normal,
            RenderedHistoryRows::new(124, rows),
            Vec::new(),
        );

        let inserted =
            terminal.insert_history_batch(&mut batch, TerminalSnapshot::new(124, 32)).unwrap();

        assert_eq!(inserted, 0);
        assert_eq!(batch.next_row, 0);
    }
}
