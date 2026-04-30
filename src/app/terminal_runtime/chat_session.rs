// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use crate::app::handoff::projection::{
    InlineOutputId, confirm_static_inserted, inline_history_replay_plan, inline_static_insert_plan,
};
use crate::ui::composer_measure::measure_composer;
use crate::ui::footer_rows::serialize_footer_rows;
use crate::ui::inline_chat_rows::{
    serialize_live_rows, serialize_live_rows_after_static_insert, serialize_transcript_rows,
};
use crate::ui::input_rows::serialize_input_rows;
use anyhow::Context;
use crossterm::queue;
use crossterm::terminal::DisableLineWrap;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Rect, Size};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::io::{self, Stdout, Write};

use super::custom_inline_terminal::Terminal;
use super::insert_history::insert_history_lines;
use super::screen_scroll::{ScreenScrollRequest, scroll_screen};

type StdoutBackend = CrosstermBackend<Stdout>;
type StdoutTerminal = Terminal<StdoutBackend>;

pub(super) struct ChatTerminalSession<B = StdoutBackend>
where
    B: Backend<Error = io::Error> + Write,
{
    terminal: Terminal<B>,
}

struct GeometryChangeLog {
    old_area: Rect,
    next_area: Rect,
    screen_size: Size,
    reclaimed_history_rows: u16,
    released_history_rows: u16,
    pending_static_rows: u16,
    scroll_down_rows: u16,
    reason: &'static str,
}

impl ChatTerminalSession<StdoutBackend> {
    pub(super) fn new() -> anyhow::Result<Self> {
        let terminal = build_terminal()?;
        let screen_size = terminal.size().context("failed to read inline terminal size")?;

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_backend_mode",
            message = "chat runtime configured for custom inline terminal",
            outcome = "success",
            backend = "custom_inline_terminal",
        );
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_terminal_initialized",
            message = "custom inline chat terminal constructed",
            outcome = "success",
            terminal_width = screen_size.width,
            terminal_height = screen_size.height,
        );

        Ok(Self { terminal })
    }
}

impl<B> ChatTerminalSession<B>
where
    B: Backend<Error = io::Error> + Write,
{
    #[cfg(test)]
    fn from_terminal(terminal: Terminal<B>) -> Self {
        Self { terminal }
    }

    pub(super) fn clear(&mut self, app: &mut App) -> anyhow::Result<()> {
        self.ensure_line_wrap_disabled(app)?;
        self.terminal.clear_visible_screen().context("failed to clear inline screen")?;
        Write::flush(self.terminal.backend_mut()).context("failed to flush inline screen clear")?;
        app.chat_render.invalidate_live_anchor();
        app.reset_committed_output_tracking();
        Ok(())
    }

    pub(super) fn clear_mutable_viewport(&mut self, app: &mut App) -> anyhow::Result<()> {
        self.ensure_line_wrap_disabled(app)?;
        self.terminal.clear().context("failed to clear inline viewport")?;
        Write::flush(self.terminal.backend_mut())
            .context("failed to flush inline viewport clear")?;
        Self::invalidate_live_region_render_state(app);
        Ok(())
    }

    pub(super) fn prepare_for_fullscreen(&mut self, app: &mut App) -> anyhow::Result<()> {
        self.clear(app)
    }

    pub(super) fn draw(&mut self, app: &mut App) -> anyhow::Result<()> {
        self.ensure_terminal_size(app)?;
        self.ensure_line_wrap_disabled(app)?;

        let screen_size = self.terminal.size().context("failed to read inline viewport size")?;
        let width = screen_size.width.max(1);
        app.chat_render.set_terminal_size(screen_size.width, screen_size.height);

        let transcript_plan = self.prepare_transcript_flush(app, width);
        let live_rows = if transcript_plan.inserted_ids.is_empty() {
            serialize_live_rows(app, width)
        } else {
            serialize_live_rows_after_static_insert(app, width, &transcript_plan.inserted_ids)
        };
        let composer_rows = Self::serialize_composer_rows(app, width);
        let layout_plan = MutableLayoutPlan::new(&live_rows, &composer_rows, screen_size.height);
        let composer_rows_total = composer_rows.all_rows();
        let visible_input_rows = layout_plan.input_visible_rows(&composer_rows.input_rows).to_vec();
        let visible_footer_rows =
            layout_plan.footer_visible_rows(&composer_rows.footer_rows).to_vec();
        let visible_composer_rows =
            ComposerRows::join_visible_rows(&visible_input_rows, &visible_footer_rows);

        let pending_static_rows = u16::try_from(transcript_plan.rows.len()).unwrap_or(u16::MAX);
        self.reconcile_mutable_viewport_geometry(
            layout_plan.viewport_height,
            screen_size,
            app,
            pending_static_rows,
        )?;

        log_inline_chat_draw(&InlineChatDrawSummary {
            app,
            transcript_rows: &transcript_plan.rows,
            live_rows_total: &live_rows,
            live_rows_visible: layout_plan.live_visible_rows(&live_rows),
            composer_rows_total: &composer_rows_total,
            composer_rows_visible: &visible_composer_rows,
            live_rows_hidden_above: layout_plan.live_window.hidden_rows_above(),
            full_rebuild: transcript_plan.full_rebuild,
        });

        self.insert_transcript_rows(&transcript_plan)?;
        complete_transcript_flush(app, &transcript_plan);
        let visible_live_rows = layout_plan.live_visible_rows(&live_rows).to_vec();
        let visible_live_row_count = visible_live_rows.len();
        let visible_composer_row_count = visible_composer_rows.len();
        let input_caret_row = layout_plan.visible_input_caret_row(composer_rows.caret_row);
        let completed = self
            .terminal
            .draw(|frame| {
                let area = frame.area();
                let (live_area, input_area, footer_area) = layout_plan.areas(area);
                if !live_area.is_empty() {
                    frame.render_widget(Paragraph::new(visible_live_rows.clone()), live_area);
                }
                if !input_area.is_empty() {
                    frame.render_widget(Paragraph::new(visible_input_rows.clone()), input_area);
                }
                if !footer_area.is_empty() {
                    frame.render_widget(Paragraph::new(visible_footer_rows.clone()), footer_area);
                }
            })
            .context("failed to draw inline chat viewport")?;

        let viewport_area = completed.area;
        let (live_area, input_area, footer_area) = layout_plan.areas(viewport_area);
        app.chat_render.live_region.anchor_valid = true;
        app.chat_render.live_region.total_rows = u16::try_from(live_rows.len()).unwrap_or(u16::MAX);
        app.chat_render.live_region.hidden_rows_above =
            u16::try_from(layout_plan.live_window.hidden_rows_above()).unwrap_or(u16::MAX);
        app.chat_render.live_region.viewport_height = live_area.height;
        app.chat_render.live_region.last_rendered_rows =
            u16::try_from(visible_live_row_count).unwrap_or(u16::MAX);
        app.chat_render.composer.last_rendered_rows =
            u16::try_from(visible_composer_row_count).unwrap_or(u16::MAX);

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_viewport_draw",
            message = "inline viewport repainted with mutable chat rows",
            outcome = "success",
            viewport_top = viewport_area.top(),
            viewport_height = viewport_area.height,
            live_top = live_area.top(),
            live_height = live_area.height,
            composer_top = input_area.top(),
            composer_height = input_area.height.saturating_add(footer_area.height),
            footer_top = footer_area.top(),
            footer_height = footer_area.height,
            terminal_width = screen_size.width,
            terminal_height = screen_size.height,
            mutable_rows = visible_live_row_count + visible_composer_row_count,
            live_rows_total = live_rows.len(),
            live_rows_visible = visible_live_row_count,
            live_rows_hidden_above = layout_plan.live_window.hidden_rows_above(),
            composer_rows_total = composer_rows.total_len(),
            composer_rows_visible = visible_composer_row_count,
            history_bounds = ?self.terminal.history_bounds(),
            caret_row = input_area.y.saturating_add(input_caret_row),
            caret_col = input_area.x.saturating_add(composer_rows.caret_col),
        );

        app.surface_dirty.chat.take_repaint();
        Ok(())
    }

    fn ensure_terminal_size(&mut self, app: &mut App) -> anyhow::Result<()> {
        let size = self.terminal.size().context("failed to read terminal size")?;
        if size.width != app.chat_render.terminal_width
            || size.height != app.chat_render.terminal_height
        {
            tracing::debug!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_resize_or_rebuild",
                message = "terminal size changed for inline chat viewport",
                outcome = "success",
                old_width = app.chat_render.terminal_width,
                old_height = app.chat_render.terminal_height,
                new_width = size.width,
                new_height = size.height,
                reason = "terminal_size_changed",
            );
            app.chat_render.set_terminal_size(size.width, size.height);
        }
        Ok(())
    }

    fn insert_transcript_rows(&mut self, plan: &TranscriptFlushPlan) -> anyhow::Result<()> {
        if plan.rows.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_committed_insert_request",
            message = "committed transcript rows scheduled for inline history insertion",
            outcome = "prepared",
            flushed_rows = plan.rows.len(),
            full_rebuild = plan.full_rebuild,
            viewport_top = self.terminal.viewport_area.top(),
            viewport_height = self.terminal.viewport_area.height,
            preview = %preview_rows(&plan.rows, 4),
        );
        let insert_outcome = insert_history_lines(&mut self.terminal, &plan.rows)
            .context("failed to insert committed transcript above inline viewport")?;
        if insert_outcome.scroll_up_amount > 0 {
            self.terminal
                .clear()
                .context("failed to clear inline viewport after committed transcript scroll")?;
        }
        Write::flush(self.terminal.backend_mut())
            .context("failed to flush committed transcript insertion")?;
        let history_bounds = self.terminal.history_bounds();
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_committed_insert_applied",
            message = "committed transcript rows inserted into terminal history",
            outcome = "success",
            insert_top = insert_outcome.insert_top,
            inserted_rows = insert_outcome.inserted_rows,
            scroll_up_amount = insert_outcome.scroll_up_amount,
            history_top = history_bounds.map(|(top, _)| top),
            history_bottom_exclusive = history_bounds.map(|(_, bottom)| bottom),
            viewport_top = self.terminal.viewport_area.top(),
            viewport_height = self.terminal.viewport_area.height,
        );
        Ok(())
    }

    fn ensure_line_wrap_disabled(&mut self, app: &mut App) -> anyhow::Result<()> {
        if app.chat_render.line_wrap_disabled {
            return Ok(());
        }
        queue!(self.terminal.backend_mut(), DisableLineWrap)
            .context("failed to disable inline viewport line wrap")?;
        Write::flush(self.terminal.backend_mut()).context("failed to flush line-wrap disable")?;
        app.chat_render.line_wrap_disabled = true;
        Ok(())
    }

    fn serialize_composer_rows(app: &mut App, width: u16) -> ComposerRows {
        let input = serialize_input_rows(app, width);
        let footer = serialize_footer_rows(app, width);
        let measurement = measure_composer(width, input.measurement, true);

        app.chat_render.composer.width = measurement.width;
        app.chat_render.composer.hint_rows = measurement.hint_rows;
        app.chat_render.composer.editor_rows = measurement.editor_rows;
        app.chat_render.composer.footer_rows = measurement.footer_rows;
        app.chat_render.composer.total_rows = measurement.total_rows;
        app.chat_render.composer.caret_row = measurement.caret_row;
        app.chat_render.composer.caret_col = measurement.caret_col;

        let mut input_rows = input.hint_rows;
        input_rows.extend(input.editor_rows);
        let footer_rows = Vec::from(footer.rows);

        ComposerRows {
            input_rows,
            footer_rows,
            caret_row: measurement.caret_row,
            caret_col: measurement.caret_col,
        }
    }

    fn reconcile_mutable_viewport_geometry(
        &mut self,
        desired_viewport_height: u16,
        screen_size: Size,
        app: &mut App,
        pending_static_rows: u16,
    ) -> anyhow::Result<()> {
        let next_area = Rect::new(
            0,
            screen_size.height.saturating_sub(desired_viewport_height),
            screen_size.width,
            desired_viewport_height,
        );
        if next_area == self.terminal.viewport_area {
            return Ok(());
        }

        let old_area = self.terminal.viewport_area;
        if old_area.is_empty() {
            self.terminal.set_viewport_area(next_area);
            self.terminal
                .clear()
                .context("failed to clear inline viewport before initial geometry set")?;
            Self::invalidate_live_region_render_state(app);
            Self::log_mutable_viewport_geometry_change(&GeometryChangeLog {
                old_area,
                next_area,
                screen_size,
                reclaimed_history_rows: 0,
                released_history_rows: 0,
                pending_static_rows,
                scroll_down_rows: 0,
                reason: "mutable_height_initialized",
            });
            return Ok(());
        }

        let reclaimed_history_rows = old_area.top().saturating_sub(next_area.top());
        let released_history_rows = next_area.top().saturating_sub(old_area.top());
        let scroll_down_rows = released_history_rows.saturating_sub(pending_static_rows);
        if reclaimed_history_rows > 0 {
            let _ =
                scroll_screen(&mut self.terminal, ScreenScrollRequest::up(reclaimed_history_rows))
                    .context("failed to scroll terminal history before viewport expansion")?;
            self.terminal.invalidate_viewport();
        } else if scroll_down_rows > 0 {
            let _ = scroll_screen(&mut self.terminal, ScreenScrollRequest::down(scroll_down_rows))
                .context("failed to scroll terminal history before viewport shrink")?;
        } else {
            self.terminal
                .clear()
                .context("failed to clear inline viewport before geometry update")?;
        }
        self.terminal.set_viewport_area(next_area);
        if reclaimed_history_rows > 0 {
            self.terminal
                .clear()
                .context("failed to clear expanded inline viewport after history scroll")?;
        } else if released_history_rows > 0 {
            self.terminal
                .clear()
                .context("failed to clear shrunken inline viewport after history scroll")?;
        }
        Self::invalidate_live_region_render_state(app);

        Self::log_mutable_viewport_geometry_change(&GeometryChangeLog {
            old_area,
            next_area,
            screen_size,
            reclaimed_history_rows,
            released_history_rows,
            pending_static_rows,
            scroll_down_rows,
            reason: "mutable_height_changed",
        });
        Ok(())
    }

    fn invalidate_live_region_render_state(app: &mut App) {
        app.chat_render.live_region.anchor_valid = false;
        app.chat_render.live_region.total_rows = 0;
        app.chat_render.live_region.hidden_rows_above = 0;
        app.chat_render.live_region.viewport_height = 0;
        app.chat_render.live_region.last_rendered_rows = 0;
    }

    fn log_mutable_viewport_geometry_change(change: &GeometryChangeLog) {
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_resize_or_rebuild",
            message = "inline viewport geometry updated in place",
            outcome = "success",
            old_top = change.old_area.top(),
            old_height = change.old_area.height,
            new_top = change.next_area.top(),
            new_height = change.next_area.height,
            reclaimed_history_rows = change.reclaimed_history_rows,
            released_history_rows = change.released_history_rows,
            pending_static_rows = change.pending_static_rows,
            scroll_down_rows = change.scroll_down_rows,
            terminal_width = change.screen_size.width,
            terminal_height = change.screen_size.height,
            reason = change.reason,
        );
    }

    fn prepare_transcript_flush(&mut self, app: &mut App, width: u16) -> TranscriptFlushPlan {
        if !app.chat_render.terminal_history_is_synced() {
            let plan = inline_history_replay_plan(app);
            let rows = serialize_transcript_rows(app, &plan.entries, false, width);
            return TranscriptFlushPlan {
                rows,
                full_rebuild: true,
                inserted_ids: plan.pending_ids,
            };
        }

        let plan = inline_static_insert_plan(app);
        if plan.items.is_empty() {
            return TranscriptFlushPlan::default();
        }
        let inserted_ids = plan.items.iter().map(|item| item.id).collect::<Vec<_>>();
        let entries = plan.items.into_iter().map(|item| item.entry).collect::<Vec<_>>();

        TranscriptFlushPlan {
            rows: serialize_transcript_rows(
                app,
                &entries,
                self.terminal.history_is_visible(),
                width,
            ),
            inserted_ids,
            full_rebuild: false,
        }
    }
}

fn build_terminal() -> anyhow::Result<StdoutTerminal> {
    Terminal::with_options(CrosstermBackend::new(std::io::stdout()))
        .context("failed to construct inline chat terminal")
}

#[derive(Default)]
struct TranscriptFlushPlan {
    rows: Vec<Line<'static>>,
    inserted_ids: Vec<InlineOutputId>,
    full_rebuild: bool,
}

fn complete_transcript_flush(app: &mut App, plan: &TranscriptFlushPlan) {
    if !plan.inserted_ids.is_empty() {
        confirm_static_inserted(&mut app.handoff_shadow, &plan.inserted_ids);
    }
    if plan.full_rebuild {
        app.chat_render.mark_terminal_history_synced();
    }
}

struct ComposerRows {
    input_rows: Vec<Line<'static>>,
    footer_rows: Vec<Line<'static>>,
    caret_row: u16,
    caret_col: u16,
}

impl ComposerRows {
    fn total_len(&self) -> usize {
        self.input_rows.len().saturating_add(self.footer_rows.len())
    }

    fn all_rows(&self) -> Vec<Line<'static>> {
        Self::join_visible_rows(&self.input_rows, &self.footer_rows)
    }

    fn join_visible_rows(
        input_rows: &[Line<'static>],
        footer_rows: &[Line<'static>],
    ) -> Vec<Line<'static>> {
        input_rows.iter().chain(footer_rows.iter()).cloned().collect::<Vec<_>>()
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

    fn translate_row(self, row: u16) -> u16 {
        if self.visible_len == 0 {
            return 0;
        }
        let visible_last = self.visible_len.saturating_sub(1);
        let visible_row = usize::from(row).saturating_sub(self.start).min(visible_last);
        u16::try_from(visible_row).unwrap_or(u16::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MutableLayoutPlan {
    live_window: RowWindow,
    input_window: RowWindow,
    footer_window: RowWindow,
    viewport_height: u16,
}

impl MutableLayoutPlan {
    fn new(live_rows: &[Line<'static>], composer_rows: &ComposerRows, screen_height: u16) -> Self {
        let footer_window = RowWindow::tail(composer_rows.footer_rows.len(), screen_height);
        let input_budget = screen_height.saturating_sub(footer_window.visible_len_u16());
        let input_window = RowWindow::tail(composer_rows.input_rows.len(), input_budget);
        let live_budget = screen_height
            .saturating_sub(footer_window.visible_len_u16())
            .saturating_sub(input_window.visible_len_u16());
        let live_window = RowWindow::tail(live_rows.len(), live_budget);
        let viewport_height = live_window
            .visible_len_u16()
            .saturating_add(input_window.visible_len_u16())
            .saturating_add(footer_window.visible_len_u16())
            .max(1)
            .min(screen_height);

        Self { live_window, input_window, footer_window, viewport_height }
    }

    fn live_visible_rows<'rows>(self, live_rows: &'rows [Line<'static>]) -> &'rows [Line<'static>] {
        self.live_window.slice(live_rows)
    }

    fn input_visible_rows<'rows>(
        self,
        input_rows: &'rows [Line<'static>],
    ) -> &'rows [Line<'static>] {
        self.input_window.slice(input_rows)
    }

    fn footer_visible_rows<'rows>(
        self,
        footer_rows: &'rows [Line<'static>],
    ) -> &'rows [Line<'static>] {
        self.footer_window.slice(footer_rows)
    }

    fn visible_input_caret_row(self, full_caret_row: u16) -> u16 {
        self.input_window.translate_row(full_caret_row)
    }

    fn areas(self, viewport_area: Rect) -> (Rect, Rect, Rect) {
        let footer_height = self.footer_window.visible_len_u16().min(viewport_area.height);
        let input_height = self
            .input_window
            .visible_len_u16()
            .min(viewport_area.height.saturating_sub(footer_height));
        let live_height =
            viewport_area.height.saturating_sub(input_height).saturating_sub(footer_height);
        let live_area =
            Rect::new(viewport_area.x, viewport_area.y, viewport_area.width, live_height);
        let input_area = Rect::new(
            viewport_area.x,
            viewport_area.y.saturating_add(live_height),
            viewport_area.width,
            input_height,
        );
        let footer_area = Rect::new(
            viewport_area.x,
            viewport_area.y.saturating_add(live_height).saturating_add(input_height),
            viewport_area.width,
            footer_height,
        );
        (live_area, input_area, footer_area)
    }
}

struct InlineChatDrawSummary<'a> {
    app: &'a App,
    transcript_rows: &'a [Line<'static>],
    live_rows_total: &'a [Line<'static>],
    live_rows_visible: &'a [Line<'static>],
    composer_rows_total: &'a [Line<'static>],
    composer_rows_visible: &'a [Line<'static>],
    live_rows_hidden_above: usize,
    full_rebuild: bool,
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
        full_rebuild = summary.full_rebuild,
        transcript_rows = summary.transcript_rows.len(),
        live_rows_total = summary.live_rows_total.len(),
        live_rows_visible = summary.live_rows_visible.len(),
        live_rows_hidden_above = summary.live_rows_hidden_above,
        composer_rows_total = summary.composer_rows_total.len(),
        composer_rows_visible = summary.composer_rows_visible.len(),
        transcript_preview = %preview_rows(summary.transcript_rows, 3),
        live_preview = %preview_rows(summary.live_rows_visible, 3),
        composer_preview = %preview_rows(summary.composer_rows_visible, 3),
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
    use super::{ChatTerminalSession, ComposerRows, MutableLayoutPlan, RowWindow};
    use crate::app::handoff::projection::{
        InlineOutputItemKind, InlineOutputStatus, inline_live_projection,
    };
    use crate::app::handoff::types::{
        SystemTranscriptEntry, TranscriptEntry, UserTranscriptBlock, UserTranscriptEntry,
    };
    use crate::app::{App, AppStatus, ChatMessage, MessageRole, SystemSeverity};
    use ratatui::backend::{Backend, ClearType, WindowSize};
    use ratatui::buffer::Cell;
    use ratatui::layout::{Position, Rect, Size};
    use ratatui::text::Line;
    use std::io;
    use std::io::Write;

    use super::Terminal;

    fn rows(count: usize) -> Vec<Line<'static>> {
        (0..count).map(|idx| Line::raw(format!("row {idx}"))).collect()
    }

    fn row_text(rows: &[Line<'static>]) -> String {
        rows.iter()
            .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn composer_rows(input_rows: usize, footer_rows: usize, caret_row: u16) -> ComposerRows {
        ComposerRows {
            input_rows: rows(input_rows),
            footer_rows: rows(footer_rows),
            caret_row,
            caret_col: 0,
        }
    }

    fn user_entry(text: &str) -> TranscriptEntry {
        TranscriptEntry::User(UserTranscriptEntry {
            blocks: vec![UserTranscriptBlock::Text(text.to_owned())],
        })
    }

    fn system_entry(text: &str) -> TranscriptEntry {
        TranscriptEntry::System(SystemTranscriptEntry {
            severity: Some(SystemSeverity::Info),
            text: text.to_owned(),
        })
    }

    fn test_terminal(backend: RecordingBackend) -> Terminal<RecordingBackend> {
        match Terminal::with_options(backend) {
            Ok(terminal) => terminal,
            Err(err) => panic!("failed to construct test terminal: {err}"),
        }
    }

    fn app_with_pending_user(text: &str) -> (App, super::InlineOutputId) {
        let mut app = App::test_default();
        app.chat_render.line_wrap_disabled = true;
        app.chat_render.mark_terminal_history_synced();
        let ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry(text)]);
        (app, ids[0])
    }

    fn app_with_pending_user_and_live_assistant(text: &str) -> App {
        let (mut app, _id) = app_with_pending_user(text);
        app.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        let _ = crate::app::handoff::shadow::begin_local_assistant_turn(&mut app.handoff_shadow);
        app.status = AppStatus::Thinking;
        crate::app::handoff::shadow::sync_handoff_commit_queue(&mut app);
        app
    }

    fn app_with_unsynced_replay() -> (App, super::InlineOutputId, super::InlineOutputId) {
        let mut app = App::test_default();
        app.chat_render.line_wrap_disabled = true;
        let inserted_ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(0, vec![user_entry("replay inserted")]);
        let pending_ids = app
            .handoff_shadow
            .inline_output
            .record_message_transcript_entries(1, vec![system_entry("replay pending")]);
        app.handoff_shadow.inline_output.confirm_static_inserted(&inserted_ids);
        (app, inserted_ids[0], pending_ids[0])
    }

    fn assert_inline_item_pending(app: &App) {
        assert!(matches!(
            app.handoff_shadow.inline_output.items()[0].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::PendingInsert, .. }
        ));
    }

    fn assert_inline_item_inserted(app: &App) {
        assert!(matches!(
            app.handoff_shadow.inline_output.items()[0].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::Inserted, .. }
        ));
    }

    fn assert_inline_item_pending_at(app: &App, idx: usize) {
        assert!(matches!(
            app.handoff_shadow.inline_output.items()[idx].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::PendingInsert, .. }
        ));
    }

    fn assert_inline_item_inserted_at(app: &App, idx: usize) {
        assert!(matches!(
            app.handoff_shadow.inline_output.items()[idx].kind,
            InlineOutputItemKind::Transcript { status: InlineOutputStatus::Inserted, .. }
        ));
    }

    #[test]
    fn row_window_keeps_tail_within_budget() {
        let window = RowWindow::tail(12, 5);

        assert_eq!(window.hidden_rows_above(), 7);
        assert_eq!(window.visible_len_u16(), 5);
    }

    #[test]
    fn mutable_layout_reserves_bottom_space_for_composer() {
        let live_rows = rows(313);
        let composer_rows = composer_rows(1, 2, 0);
        let plan = MutableLayoutPlan::new(&live_rows, &composer_rows, 40);

        assert_eq!(plan.viewport_height, 40);
        assert_eq!(plan.live_window.visible_len_u16(), 37);
        assert_eq!(plan.live_window.hidden_rows_above(), 276);
        assert_eq!(plan.input_window.visible_len_u16(), 1);
        assert_eq!(plan.footer_window.visible_len_u16(), 2);
    }

    #[test]
    fn mutable_layout_areas_pin_input_and_footer_to_bottom() {
        let plan = MutableLayoutPlan {
            live_window: RowWindow { start: 20, visible_len: 37 },
            input_window: RowWindow { start: 0, visible_len: 1 },
            footer_window: RowWindow { start: 0, visible_len: 2 },
            viewport_height: 40,
        };
        let viewport_area = Rect::new(0, 10, 120, 40);
        let (live_area, input_area, footer_area) = plan.areas(viewport_area);

        assert_eq!(live_area, Rect::new(0, 10, 120, 37));
        assert_eq!(input_area, Rect::new(0, 47, 120, 1));
        assert_eq!(footer_area, Rect::new(0, 48, 120, 2));
    }

    #[test]
    fn mutable_layout_keeps_footer_pinned_when_input_exceeds_screen() {
        let live_rows = rows(10);
        let composer_rows = composer_rows(50, 2, 49);
        let plan = MutableLayoutPlan::new(&live_rows, &composer_rows, 40);

        assert_eq!(plan.input_window.hidden_rows_above(), 12);
        assert_eq!(plan.input_window.visible_len_u16(), 38);
        assert_eq!(plan.footer_window.visible_len_u16(), 2);
        assert_eq!(plan.live_window.visible_len_u16(), 0);
        assert_eq!(plan.visible_input_caret_row(49), 37);
    }

    #[test]
    fn prepare_transcript_flush_uses_handoff_static_plan() {
        let (mut app, id) = app_with_pending_user("handoff row");
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        let plan = session.prepare_transcript_flush(&mut app, 80);

        assert_eq!(plan.inserted_ids, vec![id]);
        assert!(row_text(&plan.rows).contains("handoff row"));
    }

    #[test]
    fn prepare_transcript_flush_uses_handoff_replay_plan() {
        let (mut app, _inserted_id, pending_id) = app_with_unsynced_replay();
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        let plan = session.prepare_transcript_flush(&mut app, 80);
        let rows = row_text(&plan.rows);

        assert!(plan.full_rebuild);
        assert_eq!(plan.inserted_ids, vec![pending_id]);
        assert!(rows.contains("replay inserted"));
        assert!(rows.contains("replay pending"));
    }

    #[test]
    fn successful_insert_and_draw_marks_static_ids_inserted() {
        let (mut app, _id) = app_with_pending_user("insert me");
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        let result = session.draw(&mut app);

        assert!(result.is_ok());
        assert_inline_item_inserted(&app);
        assert!(inline_live_projection(&app).is_empty());
    }

    #[test]
    fn static_insert_targets_final_composer_viewport_without_leaving_gap() {
        let (mut app, _id) = app_with_pending_user("insert me");
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        session.draw(&mut app).expect("draw inserts pending transcript");

        let (_history_top, history_bottom) =
            session.terminal.history_bounds().expect("history bounds");
        assert_eq!(history_bottom, session.terminal.viewport_area.top());
        assert_eq!(app.chat_render.live_region.total_rows, 0);
    }

    #[test]
    fn static_insert_targets_final_live_viewport_without_leaving_gap() {
        let mut app = app_with_pending_user_and_live_assistant("insert me");
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        session.draw(&mut app).expect("draw inserts pending transcript");

        let (_history_top, history_bottom) =
            session.terminal.history_bounds().expect("history bounds");
        assert_eq!(history_bottom, session.terminal.viewport_area.top());
        assert_eq!(app.chat_render.live_region.total_rows, 2);
        assert_eq!(inline_live_projection(&app).len(), 1);
    }

    #[test]
    fn confirmed_static_history_survives_next_live_viewport_shrink() {
        let (mut app, _id) = app_with_pending_user("persist me");
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        session.draw(&mut app).expect("first draw inserts pending transcript");
        let first_bounds = session.terminal.history_bounds();
        assert!(first_bounds.is_some());
        assert!(inline_live_projection(&app).is_empty());

        session.draw(&mut app).expect("second draw shrinks live viewport after confirmation");

        assert_eq!(session.terminal.history_bounds(), first_bounds);
        assert!(inline_live_projection(&app).is_empty());
    }

    #[test]
    fn prompt_suggestion_disappearing_scrolls_history_down_through_geometry_reconciler() {
        let mut app = app_with_pending_user_and_live_assistant("insert me");
        app.prompt_suggestion = Some("Write focused tests".to_owned());
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        session.draw(&mut app).expect("draw with prompt suggestion");
        let top_with_suggestion = session.terminal.viewport_area.top();
        let (history_top_with_suggestion, history_bottom_with_suggestion) =
            session.terminal.history_bounds().expect("history bounds after first draw");
        assert_eq!(history_bottom_with_suggestion, top_with_suggestion);

        app.prompt_suggestion = None;
        session.draw(&mut app).expect("draw after prompt suggestion disappears");

        let top_without_suggestion = session.terminal.viewport_area.top();
        let (history_top_without_suggestion, history_bottom_without_suggestion) =
            session.terminal.history_bounds().expect("history bounds after shrink");
        assert_eq!(top_without_suggestion, top_with_suggestion.saturating_add(1));
        assert_eq!(history_top_without_suggestion, history_top_with_suggestion.saturating_add(1));
        assert_eq!(
            history_bottom_without_suggestion,
            history_bottom_with_suggestion.saturating_add(1)
        );
        assert_eq!(history_bottom_without_suggestion, top_without_suggestion);
    }

    #[test]
    fn live_assistant_removal_scrolls_history_down_through_geometry_reconciler() {
        let mut app = app_with_pending_user_and_live_assistant("insert me");
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        session.draw(&mut app).expect("draw with live assistant rows");
        let top_with_live = session.terminal.viewport_area.top();
        let (history_top_with_live, history_bottom_with_live) =
            session.terminal.history_bounds().expect("history bounds after first draw");
        let (msg_idx, turn_id) =
            crate::app::handoff::shadow::active_assistant_projection_anchor(&app)
                .expect("active assistant anchor");

        app.handoff_shadow.active_turn = None;
        app.handoff_shadow.inline_output.remove_assistant_live_slot(msg_idx, turn_id);
        session.draw(&mut app).expect("draw after live assistant rows are removed");

        let top_without_live = session.terminal.viewport_area.top();
        let released_rows = top_without_live.saturating_sub(top_with_live);
        let (history_top_without_live, history_bottom_without_live) =
            session.terminal.history_bounds().expect("history bounds after shrink");
        assert!(released_rows > 0);
        assert_eq!(history_top_without_live, history_top_with_live.saturating_add(released_rows));
        assert_eq!(
            history_bottom_without_live,
            history_bottom_with_live.saturating_add(released_rows)
        );
        assert_eq!(history_bottom_without_live, top_without_live);
        assert!(inline_live_projection(&app).is_empty());
    }

    #[test]
    fn pending_static_insert_consumes_released_rows_during_viewport_shrink() {
        let mut app = App::test_default();
        app.chat_render.line_wrap_disabled = true;
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(124, 29)));
        session.terminal.set_viewport_area(Rect::new(0, 9, 124, 20));
        session.terminal.record_history_insert(0, 9);

        session
            .reconcile_mutable_viewport_geometry(4, Size::new(124, 29), &mut app, 15)
            .expect("viewport shrink should account for pending static rows");
        let written = String::from_utf8_lossy(&session.terminal.backend_mut().written);
        assert!(written.contains("\x1b[1T"), "expected one released row to scroll down");
        assert!(
            !written.contains("\x1b[16T"),
            "pending static rows should not be emitted as blank scroll-down rows"
        );

        super::insert_history_lines(&mut session.terminal, &rows(15)).expect("insert static rows");
        let (_history_top, history_bottom) =
            session.terminal.history_bounds().expect("history bounds after insert");
        assert_eq!(history_bottom, session.terminal.viewport_area.top());
    }

    #[test]
    fn static_insert_clears_mutable_viewport_after_terminal_scroll() {
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 6)));
        session.terminal.set_viewport_area(Rect::new(0, 3, 80, 3));
        session.terminal.record_history_insert(0, 3);
        let plan = super::TranscriptFlushPlan {
            rows: rows(4),
            inserted_ids: Vec::new(),
            full_rebuild: false,
        };

        session.insert_transcript_rows(&plan).expect("insert should clear after scroll");

        let backend = session.terminal.backend_mut();
        assert!(
            backend.clear_region_calls > 0,
            "history insertion that scrolls the screen must physically clear the mutable viewport"
        );
    }

    #[test]
    fn force_clear_uses_full_screen_clear_and_resets_history_bounds() {
        let mut app = App::test_default();
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));
        session.terminal.set_viewport_area(Rect::new(0, 20, 80, 4));
        session.terminal.record_history_insert(0, 20);

        session.clear(&mut app).expect("force clear should clear visible screen");

        let backend = session.terminal.backend_mut();
        assert_eq!(backend.clear_calls, 1);
        assert_eq!(backend.clear_region_calls, 0);
        assert!(session.terminal.history_bounds().is_none());
    }

    #[test]
    fn successful_history_replay_marks_pending_ids_inserted_and_history_synced() {
        let (mut app, _inserted_id, _pending_id) = app_with_unsynced_replay();
        let mut session =
            ChatTerminalSession::from_terminal(test_terminal(RecordingBackend::new(80, 24)));

        let result = session.draw(&mut app);

        assert!(result.is_ok());
        assert_inline_item_inserted_at(&app, 0);
        assert_inline_item_inserted_at(&app, 1);
        assert!(app.chat_render.terminal_history_is_synced());
        assert!(inline_live_projection(&app).is_empty());
    }

    #[test]
    fn failed_history_insert_leaves_static_ids_pending_and_live() {
        let (mut app, _id) = app_with_pending_user("still live");
        let mut backend = RecordingBackend::new(80, 24);
        backend.fail_writes = true;
        let mut session = ChatTerminalSession::from_terminal(test_terminal(backend));

        let result = session.draw(&mut app);

        assert!(result.is_err());
        assert_inline_item_pending(&app);
        assert_eq!(inline_live_projection(&app).len(), 1);
    }

    #[test]
    fn failed_history_replay_insert_leaves_pending_ids_live_and_history_unsynced() {
        let (mut app, _inserted_id, _pending_id) = app_with_unsynced_replay();
        let mut backend = RecordingBackend::new(80, 24);
        backend.fail_writes = true;
        let mut session = ChatTerminalSession::from_terminal(test_terminal(backend));

        let result = session.draw(&mut app);

        assert!(result.is_err());
        assert_inline_item_inserted_at(&app, 0);
        assert_inline_item_pending_at(&app, 1);
        assert!(!app.chat_render.terminal_history_is_synced());
        assert_eq!(inline_live_projection(&app).len(), 1);
    }

    #[test]
    fn failed_backend_flush_after_history_insert_keeps_flushed_static_ids_inserted() {
        let (mut app, _id) = app_with_pending_user("flush failure");
        let mut backend = RecordingBackend::new(80, 24);
        backend.fail_backend_flush = true;
        let mut session = ChatTerminalSession::from_terminal(test_terminal(backend));

        let result = session.draw(&mut app);

        assert!(result.is_err());
        assert_inline_item_inserted(&app);
        assert!(inline_live_projection(&app).is_empty());
    }

    #[test]
    fn failed_backend_flush_after_history_replay_keeps_flushed_static_ids_inserted() {
        let (mut app, _inserted_id, _pending_id) = app_with_unsynced_replay();
        let mut backend = RecordingBackend::new(80, 24);
        backend.fail_backend_flush = true;
        let mut session = ChatTerminalSession::from_terminal(test_terminal(backend));

        let result = session.draw(&mut app);

        assert!(result.is_err());
        assert_inline_item_inserted_at(&app, 0);
        assert_inline_item_inserted_at(&app, 1);
        assert!(app.chat_render.terminal_history_is_synced());
        assert!(inline_live_projection(&app).is_empty());
    }

    #[test]
    fn viewport_resize_failures_do_not_drop_unconfirmed_static_ids() {
        let (mut app, _id) = app_with_pending_user("resize pending");
        let mut backend = RecordingBackend::new(80, 18);
        backend.fail_writes = true;
        let mut session = ChatTerminalSession::from_terminal(test_terminal(backend));

        assert!(session.draw(&mut app).is_err());
        assert_inline_item_pending(&app);

        session.terminal.backend_mut().size = Size::new(80, 10);
        assert!(session.draw(&mut app).is_err());
        assert_inline_item_pending(&app);

        session.terminal.backend_mut().size = Size::new(80, 30);
        assert!(session.draw(&mut app).is_err());
        assert_inline_item_pending(&app);
        assert_eq!(inline_live_projection(&app).len(), 1);
    }

    #[derive(Debug)]
    struct RecordingBackend {
        size: Size,
        cursor: Position,
        written: Vec<u8>,
        fail_writes: bool,
        fail_backend_flush: bool,
        clear_region_calls: usize,
        clear_calls: usize,
    }

    impl RecordingBackend {
        const fn new(width: u16, height: u16) -> Self {
            Self {
                size: Size::new(width, height),
                cursor: Position::ORIGIN,
                written: Vec::new(),
                fail_writes: false,
                fail_backend_flush: false,
                clear_region_calls: 0,
                clear_calls: 0,
            }
        }
    }

    impl Write for RecordingBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.fail_writes {
                return Err(io::Error::other("test write failure"));
            }
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for RecordingBackend {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.cursor)
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.cursor = position.into();
            Ok(())
        }

        fn clear(&mut self) -> io::Result<()> {
            self.clear_calls = self.clear_calls.saturating_add(1);
            Ok(())
        }

        fn clear_region(&mut self, _clear_type: ClearType) -> io::Result<()> {
            self.clear_region_calls = self.clear_region_calls.saturating_add(1);
            Ok(())
        }

        fn size(&self) -> io::Result<Size> {
            Ok(self.size)
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize { columns_rows: self.size, pixels: self.size })
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_backend_flush {
                return Err(io::Error::other("test backend flush failure"));
            }
            Ok(())
        }

        fn scroll_region_up(
            &mut self,
            _region: std::ops::Range<u16>,
            _line_count: u16,
        ) -> io::Result<()> {
            Ok(())
        }

        fn scroll_region_down(
            &mut self,
            _region: std::ops::Range<u16>,
            _line_count: u16,
        ) -> io::Result<()> {
            Ok(())
        }
    }
}
