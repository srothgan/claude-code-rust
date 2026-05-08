// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::history_insert::RenderedHistoryRows;
use crate::app::handoff::projection::InlineOutputId;
use anyhow::{Context, anyhow, bail};
use crossterm::SynchronizedUpdate;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal::{Clear, ClearType, DisableLineWrap};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::widgets::{Clear as RatatuiClear, Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::collections::VecDeque;
use std::io::{Stdout, Write};

type StdoutBackend = CrosstermBackend<Stdout>;
type StdoutTerminal = Terminal<StdoutBackend>;

const MIN_REPLAY_BATCH_ROWS: usize = 32;
const MAX_REPLAY_BATCH_ROWS: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryBatchKind {
    Normal,
    Replay,
}

#[derive(Debug, Clone)]
pub(super) struct PendingHistoryBatch {
    pub kind: HistoryBatchKind,
    pub rows: RenderedHistoryRows,
    pub confirm_ids: Vec<InlineOutputId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ChatDrawRequest {
    pub requested_inline_height: u16,
    pub terminal_width: u16,
    pub terminal_height: u16,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct FlushedHistory {
    pub confirmed_ids: Vec<InlineOutputId>,
    pub flushed_rows: usize,
    pub replay_complete: bool,
    pub replay_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChatDrawOutcome {
    pub viewport_area: Rect,
    pub flushed_history: FlushedHistory,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct InlineViewportState {
    pub(super) area: Option<Rect>,
    pub(super) visible_history_rows: u16,
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

#[derive(Debug)]
struct ReplayState {
    render_width: u16,
    pending_batches: VecDeque<PendingHistoryBatch>,
    confirm_ids: Vec<InlineOutputId>,
}

impl ReplayState {
    fn new(
        render_width: u16,
        batches: Vec<PendingHistoryBatch>,
        confirm_ids: Vec<InlineOutputId>,
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

    fn pending_ids(&self) -> &[InlineOutputId] {
        &self.confirm_ids
    }

    #[cfg(test)]
    fn drain_next_batches(&mut self, terminal_height: u16) -> Vec<PendingHistoryBatch> {
        let budget = replay_batch_row_budget(terminal_height);
        let mut drained = Vec::new();
        let mut drained_rows = 0usize;

        while let Some(batch) = self.pending_batches.pop_front() {
            drained_rows = drained_rows.saturating_add(batch.rows.len());
            drained.push(batch);

            if drained_rows >= budget {
                break;
            }
        }

        drained
    }

    fn is_complete(&self) -> bool {
        self.pending_batches.is_empty()
    }
}

pub(super) struct ChatTerminal {
    terminal: Option<StdoutTerminal>,
    state: InlineViewportState,
    pending_history: VecDeque<PendingHistoryBatch>,
    replay: Option<ReplayState>,
}

impl ChatTerminal {
    pub(super) fn new(owned_top: u16) -> Self {
        Self {
            terminal: None,
            state: InlineViewportState::new(owned_top),
            pending_history: VecDeque::new(),
            replay: None,
        }
    }

    pub(super) fn reset_visible(&mut self) -> anyhow::Result<()> {
        self.reset_visible_with_reason("visible_reset")
    }

    pub(super) fn reset_session_boundary(&mut self) -> anyhow::Result<()> {
        let cleared_area = self.clear_owned_region("session_boundary_reset")?;
        self.reset_visible_state();
        if let Some(area) = cleared_area {
            self.anchor_next_viewport_to_cleared_region(area);
        } else {
            self.state.area = None;
        }
        Ok(())
    }

    fn reset_visible_with_reason(&mut self, reason: &'static str) -> anyhow::Result<()> {
        let _ = self.clear_owned_region(reason)?;
        self.reset_visible_state();
        Ok(())
    }

    fn reset_visible_state(&mut self) {
        self.terminal = None;
        self.state.visible_history_rows = 0;
        self.pending_history.clear();
        self.replay = None;
    }

    pub(super) fn reset_mutable_viewport(&mut self) -> anyhow::Result<()> {
        self.clear_inline_terminal_viewport("mutable_reset")?;
        self.terminal = None;
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
                message = "static transcript insert left pending while replay is active",
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
        confirm_ids: Vec<InlineOutputId>,
    ) {
        self.pending_history.clear();
        self.replay = Some(ReplayState::new(render_width, batches, confirm_ids));
    }

    pub(super) fn replay_pending_ids(&self) -> &[InlineOutputId] {
        self.replay.as_ref().map_or(&[], ReplayState::pending_ids)
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
                let geometry_plan = plan_inline_geometry(
                    self.state.area,
                    chat_frame.requested_inline_height,
                    chat_frame.terminal_width,
                    chat_frame.terminal_height,
                );
                log_inline_geometry_plan(&geometry_plan);

                self.ensure_inline_terminal_height(
                    geometry_plan.height,
                    geometry_plan.target_area,
                    chat_frame.terminal_width,
                    chat_frame.terminal_height,
                )?;
                let flushed_history = self
                    .flush_queued_history(chat_frame.terminal_width, chat_frame.terminal_height)?;
                let viewport_area = self.draw_mutable_viewport(render_mutable)?;

                self.state.area = Some(viewport_area);
                self.mark_area_owned(viewport_area, chat_frame.terminal_height, "viewport_draw");
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
                    queued_rows = flushed_history.flushed_rows,
                    confirmed_ids = flushed_history.confirmed_ids.len(),
                    replay_complete = flushed_history.replay_complete,
                    replay_incomplete = flushed_history.replay_incomplete,
                );

                Ok(ChatDrawOutcome { viewport_area, flushed_history })
            })
            .context("failed synchronized inline chat terminal update")?
    }

    fn ensure_inline_terminal_height(
        &mut self,
        desired_height: u16,
        anchor_area: Option<Rect>,
        terminal_width: u16,
        terminal_height: u16,
    ) -> anyhow::Result<()> {
        let screen_height = terminal_height.max(1);
        let next_height = desired_height.max(1).min(screen_height);
        let current_anchor = self.state.area;
        let anchor_changed = anchor_area.zip(current_anchor).is_some_and(|(next, current)| {
            next.x != current.x || next.y != current.y || next.width != current.width
        });
        if self.terminal.is_some() && self.state.height() == next_height && !anchor_changed {
            return Ok(());
        }

        if self.terminal.is_some()
            && let Some(area) = current_anchor
        {
            self.clear_owned_area(area, "viewport_reconfigure")?;
        }

        let cursor_before = crossterm::cursor::position()
            .context("failed to read cursor before inline terminal ensure")?;
        let anchor = anchor_area.or(self.state.area);
        if let Some(area) = anchor {
            move_cursor_to(area).context("failed to restore inline viewport anchor")?;
        }
        let cursor_y = anchor.map_or(cursor_before.1, |area| area.y);
        let predicted_area = Rect::new(
            0,
            inline_viewport_top_after_create(cursor_y, next_height, screen_height),
            terminal_width.max(1),
            next_height,
        );

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
        Ok(())
    }

    fn clear_inline_terminal_viewport(&mut self, reason: &'static str) -> anyhow::Result<()> {
        let Some(area) = self.state.area else {
            return Ok(());
        };
        self.clear_owned_area(area, reason)
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
        self.clear_owned_area(area, reason)?;
        Ok(Some(area))
    }

    fn flush_queued_history(
        &mut self,
        terminal_width: u16,
        terminal_height: u16,
    ) -> anyhow::Result<FlushedHistory> {
        if self.replay.is_some() {
            return self.flush_replay_history(terminal_width, terminal_height);
        }

        let mut flushed = FlushedHistory::default();
        while let Some(batch) = self.pending_history.front().cloned() {
            flushed.flushed_rows = flushed.flushed_rows.saturating_add(self.insert_history_batch(
                &batch,
                terminal_width,
                terminal_height,
            )?);
            self.pending_history.pop_front();
            flushed.confirmed_ids.extend(batch.confirm_ids);
        }
        Ok(flushed)
    }

    fn flush_replay_history(
        &mut self,
        terminal_width: u16,
        terminal_height: u16,
    ) -> anyhow::Result<FlushedHistory> {
        if self.replay.is_none() {
            return Ok(FlushedHistory::default());
        }

        let budget = replay_batch_row_budget(terminal_height);
        let mut flushed_rows = 0usize;
        let mut drained_rows = 0usize;
        while let Some(batch) =
            self.replay.as_ref().and_then(|replay| replay.pending_batches.front().cloned())
        {
            drained_rows = drained_rows.saturating_add(batch.rows.len());
            flushed_rows = flushed_rows.saturating_add(self.insert_history_batch(
                &batch,
                terminal_width,
                terminal_height,
            )?);
            let Some(replay) = self.replay.as_mut() else {
                break;
            };
            replay.pending_batches.pop_front();

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
        batch: &PendingHistoryBatch,
        terminal_width: u16,
        terminal_height: u16,
    ) -> anyhow::Result<usize> {
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_history_insert_request",
            message = "history rows scheduled for ratatui inline insertion",
            outcome = "prepared",
            rows = batch.rows.len(),
            confirm_ids = batch.confirm_ids.len(),
            batch_kind = ?batch.kind,
        );

        if batch.rows.is_empty() {
            return Ok(0);
        }
        let terminal_width = terminal_width.max(1);
        if batch.rows.width != terminal_width {
            bail!(
                "refusing inline history insert rendered at stale width: rendered_width={}, terminal_width={}, rows={}",
                batch.rows.width,
                terminal_width,
                batch.rows.len()
            );
        }

        let row_count = u16::try_from(batch.rows.len()).unwrap_or(u16::MAX).max(1);
        self.ensure_insert_within_owned_region(row_count, terminal_width, terminal_height)?;
        let before_area = self
            .state
            .area
            .ok_or_else(|| anyhow!("inline chat viewport missing before history insert"))?;
        let rows = batch.rows.iter().cloned().collect::<Vec<_>>();
        let terminal = self
            .terminal
            .as_mut()
            .ok_or_else(|| anyhow!("inline chat terminal missing before history insert"))?;
        terminal
            .insert_before(row_count, |buffer| {
                Paragraph::new(rows).render(buffer.area, buffer);
            })
            .context("failed to insert committed transcript above inline viewport")?;

        let after_area = viewport_area_after_insert(before_area, row_count, terminal_height);
        self.state.area = Some(after_area);
        self.mark_area_owned(
            Rect::new(
                0,
                before_area.top().min(after_area.top()),
                terminal_width.max(1),
                after_area.bottom().saturating_sub(before_area.top().min(after_area.top())),
            ),
            terminal_height,
            "history_insert",
        );
        self.state.visible_history_rows = self.state.visible_history_rows.saturating_add(row_count);
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_history_insert_applied",
            message = "history rows inserted before ratatui inline viewport",
            outcome = "success",
            inserted_rows = row_count,
            viewport_top_before = before_area.top(),
            viewport_top_after = after_area.top(),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
            visible_history_rows = self.state.visible_history_rows,
            batch_kind = ?batch.kind,
        );

        Ok(usize::from(row_count))
    }

    fn draw_mutable_viewport<F>(&mut self, render_mutable: F) -> anyhow::Result<Rect>
    where
        F: FnOnce(&mut ratatui::Frame<'_>, Rect),
    {
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

        Ok(viewport_area)
    }

    fn ensure_insert_within_owned_region(
        &mut self,
        row_count: u16,
        terminal_width: u16,
        terminal_height: u16,
    ) -> anyhow::Result<()> {
        let area = self
            .state
            .area
            .ok_or_else(|| anyhow!("inline chat viewport missing before insert guard"))?;
        let plan = plan_owned_insert(area, self.state.owned_top, row_count, terminal_height.max(1));

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_insert_ownership_plan",
            message = "history insert checked against owned terminal region",
            outcome = if plan.allowed { "allowed" } else { "blocked" },
            rows = row_count,
            viewport_top = area.top(),
            viewport_bottom = area.bottom(),
            owned_top = self.state.owned_top,
            owned_bottom = self.state.owned_bottom,
            terminal_height,
            available_below = plan.available_below,
            scroll_down_rows = plan.scroll_down_rows,
        );

        if !plan.allowed {
            bail!(
                "refusing inline history insert outside owned region: rows={}, viewport={:?}, owned={}..{}, terminal_height={}",
                row_count,
                area,
                self.state.owned_top,
                self.state.owned_bottom,
                terminal_height
            );
        }

        if plan.scroll_down_rows > 0 {
            self.append_blank_lines_for_owned_region(plan.scroll_down_rows, terminal_height)?;
            let area_after_scroll = shift_area_up(area, plan.scroll_down_rows);
            self.state.area = Some(area_after_scroll);
            self.terminal = None;
            self.ensure_inline_terminal_height(
                area_after_scroll.height,
                Some(area_after_scroll),
                terminal_width,
                terminal_height,
            )?;
        }

        Ok(())
    }

    fn append_blank_lines_for_owned_region(
        &mut self,
        rows: u16,
        terminal_height: u16,
    ) -> anyhow::Result<()> {
        if rows == 0 {
            return Ok(());
        }

        let bottom = terminal_height.saturating_sub(1);
        let mut stdout = std::io::stdout();
        queue!(stdout, MoveTo(0, bottom)).context("failed to move cursor before owned growth")?;
        for _ in 0..rows {
            queue!(stdout, Print("\r\n")).context("failed to queue owned growth newline")?;
        }
        stdout.flush().context("failed to flush owned growth newlines")?;

        let old_top = self.state.owned_top;
        let old_bottom = self.state.owned_bottom;
        self.state.owned_top = self.state.owned_top.saturating_sub(rows);
        self.state.owned_bottom = terminal_height.max(1);
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
            terminal_height,
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

    fn clear_owned_area(&mut self, area: Rect, reason: &'static str) -> anyhow::Result<()> {
        if area.is_empty() {
            return Ok(());
        }

        let (_terminal_width, terminal_height) =
            crossterm::terminal::size().context("failed to read terminal size before clear")?;
        let visible_bottom = self.state.owned_bottom.min(terminal_height);
        if area.top() < self.state.owned_top || area.bottom() > visible_bottom {
            bail!(
                "refusing to clear outside inline chat owned region: clear={:?}, owned={}..{}, terminal_height={}",
                area,
                self.state.owned_top,
                self.state.owned_bottom,
                terminal_height
            );
        }

        let mut stdout = std::io::stdout();
        for y in area.top()..area.bottom() {
            queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))
                .context("failed to queue bounded inline clear")?;
        }
        stdout.flush().context("failed to flush bounded inline clear")?;

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

fn replay_batch_row_budget(terminal_height: u16) -> usize {
    usize::from(terminal_height)
        .saturating_mul(2)
        .clamp(MIN_REPLAY_BATCH_ROWS, MAX_REPLAY_BATCH_ROWS)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlineGeometryPlan {
    pub(crate) old_area: Option<Rect>,
    pub(crate) target_area: Option<Rect>,
    pub(crate) height: u16,
}

pub(crate) fn plan_inline_geometry(
    last_frame_area: Option<Rect>,
    desired_height: u16,
    terminal_width: u16,
    terminal_height: u16,
) -> InlineGeometryPlan {
    let screen_height = terminal_height.max(1);
    let height = desired_height.max(1).min(screen_height);
    let old_area = last_frame_area.filter(|area| !area.is_empty());
    let target_area = old_area.map(|area| {
        Rect::new(0, area.y.min(screen_height.saturating_sub(1)), terminal_width.max(1), height)
    });
    InlineGeometryPlan { old_area, target_area, height }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnedInsertPlan {
    allowed: bool,
    available_below: u16,
    scroll_down_rows: u16,
}

fn plan_owned_insert(
    viewport_area: Rect,
    owned_top: u16,
    inserted_rows: u16,
    terminal_height: u16,
) -> OwnedInsertPlan {
    let screen_height = terminal_height.max(1);
    if viewport_area.is_empty()
        || viewport_area.top() < owned_top
        || viewport_area.bottom() > screen_height
    {
        return OwnedInsertPlan { allowed: false, available_below: 0, scroll_down_rows: 0 };
    }

    let available_below = screen_height.saturating_sub(viewport_area.bottom());
    if inserted_rows <= available_below {
        return OwnedInsertPlan { allowed: true, available_below, scroll_down_rows: 0 };
    }

    let overflow = inserted_rows.saturating_sub(available_below);
    let scroll_down_rows = overflow.min(owned_top);
    let owned_top_after_scroll = owned_top.saturating_sub(scroll_down_rows);
    let available_after_scroll = available_below.saturating_add(scroll_down_rows);
    let allowed = inserted_rows <= available_after_scroll || owned_top_after_scroll == 0;

    OwnedInsertPlan { allowed, available_below, scroll_down_rows }
}

fn shift_area_up(area: Rect, rows: u16) -> Rect {
    Rect::new(area.x, area.y.saturating_sub(rows), area.width, area.height)
}

fn viewport_area_after_insert(area: Rect, inserted_rows: u16, terminal_height: u16) -> Rect {
    let max_top = terminal_height.max(1).saturating_sub(area.height);
    Rect::new(area.x, area.y.saturating_add(inserted_rows).min(max_top), area.width, area.height)
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
        target_top = plan.target_area.map(Rect::top),
        target_height = plan.target_area.map(|area| area.height),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        ChatTerminal, HistoryBatchKind, InlineViewportState, PendingHistoryBatch, ReplayState,
        inline_viewport_scroll_rows_after_create, inline_viewport_top_after_create,
        plan_inline_geometry, plan_owned_insert, replay_batch_row_budget, shift_area_up,
        viewport_area_after_insert,
    };
    use crate::app::terminal_runtime::history_insert::RenderedHistoryRows;
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    fn batch(row_count: usize) -> PendingHistoryBatch {
        PendingHistoryBatch {
            kind: HistoryBatchKind::Replay,
            rows: RenderedHistoryRows::new(
                80,
                (0..row_count).map(|idx| Line::from(format!("row {idx}"))).collect(),
            ),
            confirm_ids: Vec::new(),
        }
    }

    fn batch_with_kind(
        kind: HistoryBatchKind,
        width: u16,
        row_count: usize,
    ) -> PendingHistoryBatch {
        PendingHistoryBatch {
            kind,
            rows: RenderedHistoryRows::new(
                width,
                (0..row_count).map(|idx| Line::from(format!("row {idx}"))).collect(),
            ),
            confirm_ids: Vec::new(),
        }
    }

    #[test]
    fn replay_budget_is_bounded_by_terminal_height() {
        assert_eq!(replay_batch_row_budget(10), 32);
        assert_eq!(replay_batch_row_budget(40), 80);
        assert_eq!(replay_batch_row_budget(120), 160);
    }

    #[test]
    fn replay_drains_batches_oldest_first_with_budget() {
        let mut replay =
            ReplayState::new(80, vec![batch(10), batch(15), batch(20), batch(10)], vec![]);

        let drained = replay.drain_next_batches(16);

        assert_eq!(drained.len(), 3);
        assert_eq!(drained.iter().map(|batch| batch.rows.len()).sum::<usize>(), 45);
        assert!(!replay.is_complete());
    }

    #[test]
    fn replay_state_tracks_render_width() {
        let replay = ReplayState::new(80, vec![batch(10)], vec![]);

        assert!(replay.matches_width(80));
        assert!(!replay.matches_width(120));
    }

    #[test]
    fn history_insert_rejects_rows_rendered_for_stale_width_for_all_batch_kinds() {
        for kind in [HistoryBatchKind::Normal, HistoryBatchKind::Replay] {
            let mut terminal = ChatTerminal::new(0);
            let batch = batch_with_kind(kind, 80, 1);

            let err = terminal
                .insert_history_batch(&batch, 120, 40)
                .expect_err("stale width must be rejected before terminal mutation");

            assert!(err.to_string().contains("stale width"));
        }
    }

    #[test]
    fn inline_viewport_state_height_derives_from_area() {
        let mut state = InlineViewportState::new(10);
        assert_eq!(state.height(), 0);

        state.area = Some(Rect::new(0, 20, 80, 5));

        assert_eq!(state.height(), 5);
    }

    #[test]
    fn owned_insert_plan_allows_insert_when_viewport_has_room_below() {
        let plan = plan_owned_insert(Rect::new(0, 22, 120, 5), 22, 9, 37);

        assert!(plan.allowed);
        assert_eq!(plan.available_below, 10);
        assert_eq!(plan.scroll_down_rows, 0);
    }

    #[test]
    fn owned_insert_plan_expands_downward_before_insert_scrolls_above_owned_top() {
        let plan = plan_owned_insert(Rect::new(0, 28, 120, 5), 20, 9, 37);

        assert!(plan.allowed);
        assert_eq!(plan.available_below, 4);
        assert_eq!(plan.scroll_down_rows, 5);
    }

    #[test]
    fn owned_insert_plan_allows_screen_scroll_after_entire_visible_screen_is_owned() {
        let plan = plan_owned_insert(Rect::new(0, 28, 120, 5), 3, 20, 37);

        assert!(plan.allowed);
        assert_eq!(plan.available_below, 4);
        assert_eq!(plan.scroll_down_rows, 3);
    }

    #[test]
    fn owned_insert_plan_blocks_viewport_above_owned_region() {
        let plan = plan_owned_insert(Rect::new(0, 18, 120, 5), 20, 1, 37);

        assert!(!plan.allowed);
    }

    #[test]
    fn area_helpers_track_viewport_after_owned_scroll_and_insert() {
        let area = Rect::new(0, 28, 120, 5);

        assert_eq!(shift_area_up(area, 5), Rect::new(0, 23, 120, 5));
        assert_eq!(
            viewport_area_after_insert(Rect::new(0, 23, 120, 5), 9, 37),
            Rect::new(0, 32, 120, 5)
        );
    }

    #[test]
    fn inline_viewport_scroll_rows_track_bottom_reconfigure_scroll() {
        assert_eq!(inline_viewport_scroll_rows_after_create(31, 8, 37), 2);
        assert_eq!(inline_viewport_scroll_rows_after_create(23, 11, 37), 0);
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
}
