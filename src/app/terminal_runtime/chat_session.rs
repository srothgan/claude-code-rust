// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use crate::app::handoff::shadow::committed_transcript_entries;
use crate::ui::composer_measure::measure_composer;
use crate::ui::footer_rows::serialize_footer_rows;
use crate::ui::inline_chat_rows::{serialize_live_rows, serialize_transcript_rows};
use crate::ui::input_rows::serialize_input_rows;
use anyhow::Context;
use crossterm::queue;
use crossterm::terminal::DisableLineWrap;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect, Size};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::io::{Stdout, Write};

use super::custom_inline_terminal::Terminal;
use super::insert_history::insert_history_lines;
use super::screen_scroll::{ScreenScrollRequest, scroll_screen};

type StdoutTerminal = Terminal<CrosstermBackend<Stdout>>;

pub(super) struct ChatTerminalSession {
    terminal: StdoutTerminal,
}

impl ChatTerminalSession {
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

    pub(super) fn clear(&mut self, app: &mut App) -> anyhow::Result<()> {
        self.ensure_line_wrap_disabled(app)?;
        self.terminal.clear().context("failed to clear inline viewport")?;
        self.terminal.backend_mut().flush().context("failed to flush inline viewport clear")?;
        app.chat_render.invalidate_live_anchor();
        app.reset_committed_output_tracking();
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
        let live_rows = serialize_live_rows(app, width);
        let composer_rows = Self::serialize_composer_rows(app, width);
        let layout_plan = MutableLayoutPlan::new(&live_rows, &composer_rows, screen_size.height);

        self.update_viewport(layout_plan.viewport_height, screen_size, app)?;

        log_inline_chat_draw(
            app,
            &transcript_plan.rows,
            &live_rows,
            layout_plan.live_visible_rows(&live_rows),
            &composer_rows.rows,
            layout_plan.composer_visible_rows(&composer_rows.rows),
            layout_plan.live_window.hidden_rows_above(),
            transcript_plan.full_rebuild,
        );

        if !transcript_plan.rows.is_empty() {
            tracing::debug!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_committed_insert_request",
                message = "committed transcript rows scheduled for inline history insertion",
                outcome = "prepared",
                flushed_rows = transcript_plan.rows.len(),
                full_rebuild = transcript_plan.full_rebuild,
                viewport_top = self.terminal.viewport_area.top(),
                viewport_height = self.terminal.viewport_area.height,
                preview = %preview_rows(&transcript_plan.rows, 4),
            );
            let insert_outcome = insert_history_lines(&mut self.terminal, &transcript_plan.rows)
                .context("failed to insert committed transcript above inline viewport")?;
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
        }
        if transcript_plan.full_rebuild {
            app.chat_render.mark_terminal_history_synced();
        }

        let visible_live_rows = layout_plan.live_visible_rows(&live_rows).to_vec();
        let visible_composer_rows = layout_plan.composer_visible_rows(&composer_rows.rows).to_vec();
        let visible_live_row_count = visible_live_rows.len();
        let visible_composer_row_count = visible_composer_rows.len();
        let composer_caret_row = layout_plan.visible_composer_caret_row(composer_rows.caret_row);
        let completed = self
            .terminal
            .draw(|frame| {
                let area = frame.area();
                let (live_area, composer_area) = layout_plan.areas(area);
                if !live_area.is_empty() {
                    frame.render_widget(Paragraph::new(visible_live_rows.clone()), live_area);
                }
                if !composer_area.is_empty() {
                    frame.render_widget(
                        Paragraph::new(visible_composer_rows.clone()),
                        composer_area,
                    );
                }
                frame.set_cursor_position(Position::new(
                    composer_area.x.saturating_add(composer_rows.caret_col),
                    composer_area.y.saturating_add(composer_caret_row),
                ));
            })
            .context("failed to draw inline chat viewport")?;

        let viewport_area = completed.area;
        let (live_area, composer_area) = layout_plan.areas(viewport_area);
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
            composer_top = composer_area.top(),
            composer_height = composer_area.height,
            terminal_width = screen_size.width,
            terminal_height = screen_size.height,
            mutable_rows = visible_live_row_count + visible_composer_row_count,
            live_rows_total = live_rows.len(),
            live_rows_visible = visible_live_row_count,
            live_rows_hidden_above = layout_plan.live_window.hidden_rows_above(),
            composer_rows_total = composer_rows.rows.len(),
            composer_rows_visible = visible_composer_row_count,
            history_bounds = ?self.terminal.history_bounds(),
            caret_row = composer_area.y.saturating_add(composer_caret_row),
            caret_col = composer_area.x.saturating_add(composer_rows.caret_col),
        );

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

    fn ensure_line_wrap_disabled(&mut self, app: &mut App) -> anyhow::Result<()> {
        if app.chat_render.line_wrap_disabled {
            return Ok(());
        }
        queue!(self.terminal.backend_mut(), DisableLineWrap)
            .context("failed to disable inline viewport line wrap")?;
        self.terminal.backend_mut().flush().context("failed to flush line-wrap disable")?;
        app.chat_render.line_wrap_disabled = true;
        Ok(())
    }

    fn serialize_composer_rows(app: &mut App, width: u16) -> ComposerRows {
        let input = serialize_input_rows(app, width);
        let footer = serialize_footer_rows(app, width);
        let measurement = measure_composer(width, input.measurement, true);

        app.rendered_input_lines = input.plain_editor_rows;
        app.chat_render.composer.width = measurement.width;
        app.chat_render.composer.hint_rows = measurement.hint_rows;
        app.chat_render.composer.editor_rows = measurement.editor_rows;
        app.chat_render.composer.footer_rows = measurement.footer_rows;
        app.chat_render.composer.total_rows = measurement.total_rows;
        app.chat_render.composer.caret_row = measurement.caret_row;
        app.chat_render.composer.caret_col = measurement.caret_col;

        let mut rows = input.hint_rows;
        rows.extend(input.editor_rows);
        rows.extend(footer.rows);

        ComposerRows { rows, caret_row: measurement.caret_row, caret_col: measurement.caret_col }
    }

    fn update_viewport(
        &mut self,
        desired_viewport_height: u16,
        screen_size: Size,
        app: &mut App,
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
        let reclaimed_history_rows = old_area.top().saturating_sub(next_area.top());
        let released_history_rows = next_area.top().saturating_sub(old_area.top());
        if reclaimed_history_rows > 0 {
            let _ =
                scroll_screen(&mut self.terminal, ScreenScrollRequest::up(reclaimed_history_rows))
                    .context("failed to scroll terminal history before viewport expansion")?;
            self.terminal.invalidate_viewport();
        } else if released_history_rows > 0 {
            let _ =
                scroll_screen(&mut self.terminal, ScreenScrollRequest::down(released_history_rows))
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
        }
        app.chat_render.live_region.anchor_valid = false;
        app.chat_render.live_region.total_rows = 0;
        app.chat_render.live_region.hidden_rows_above = 0;
        app.chat_render.live_region.viewport_height = 0;
        app.chat_render.live_region.last_rendered_rows = 0;

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_resize_or_rebuild",
            message = "inline viewport geometry updated in place",
            outcome = "success",
            old_top = old_area.top(),
            old_height = old_area.height,
            new_top = next_area.top(),
            new_height = next_area.height,
            reclaimed_history_rows,
            released_history_rows,
            terminal_width = screen_size.width,
            terminal_height = screen_size.height,
            reason = "mutable_height_changed",
        );
        Ok(())
    }

    fn prepare_transcript_flush(&mut self, app: &mut App, width: u16) -> TranscriptFlushPlan {
        if !app.chat_render.terminal_history_is_synced() {
            let entries = committed_transcript_entries(app);
            let rows = serialize_transcript_rows(app, &entries, false, width);
            let _ = app.chat_render.take_pending_transcript_entries();
            return TranscriptFlushPlan { rows, full_rebuild: true };
        }

        let pending_entries = app.chat_render.take_pending_transcript_entries();
        if pending_entries.is_empty() {
            return TranscriptFlushPlan::default();
        }

        TranscriptFlushPlan {
            rows: serialize_transcript_rows(
                app,
                &pending_entries,
                self.terminal.history_is_visible(),
                width,
            ),
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
    full_rebuild: bool,
}

struct ComposerRows {
    rows: Vec<Line<'static>>,
    caret_row: u16,
    caret_col: u16,
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

    fn slice<'a, T>(self, rows: &'a [T]) -> &'a [T] {
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
    composer_window: RowWindow,
    viewport_height: u16,
}

impl MutableLayoutPlan {
    fn new(live_rows: &[Line<'static>], composer_rows: &ComposerRows, screen_height: u16) -> Self {
        let composer_window = RowWindow::tail(composer_rows.rows.len(), screen_height);
        let live_budget = screen_height.saturating_sub(composer_window.visible_len_u16());
        let live_window = RowWindow::tail(live_rows.len(), live_budget);
        let viewport_height = live_window
            .visible_len_u16()
            .saturating_add(composer_window.visible_len_u16())
            .max(1)
            .min(screen_height);

        Self { live_window, composer_window, viewport_height }
    }

    fn live_visible_rows<'a>(self, live_rows: &'a [Line<'static>]) -> &'a [Line<'static>] {
        self.live_window.slice(live_rows)
    }

    fn composer_visible_rows<'a>(self, composer_rows: &'a [Line<'static>]) -> &'a [Line<'static>] {
        self.composer_window.slice(composer_rows)
    }

    fn visible_composer_caret_row(self, full_caret_row: u16) -> u16 {
        self.composer_window.translate_row(full_caret_row)
    }

    fn areas(self, viewport_area: Rect) -> (Rect, Rect) {
        let composer_height = self.composer_window.visible_len_u16().min(viewport_area.height);
        let live_height = viewport_area.height.saturating_sub(composer_height);
        let live_area =
            Rect::new(viewport_area.x, viewport_area.y, viewport_area.width, live_height);
        let composer_area = Rect::new(
            viewport_area.x,
            viewport_area.y.saturating_add(live_height),
            viewport_area.width,
            composer_height,
        );
        (live_area, composer_area)
    }
}

fn log_inline_chat_draw(
    app: &App,
    transcript_rows: &[Line<'static>],
    live_rows_total: &[Line<'static>],
    live_rows_visible: &[Line<'static>],
    composer_rows_total: &[Line<'static>],
    composer_rows_visible: &[Line<'static>],
    live_rows_hidden_above: usize,
    full_rebuild: bool,
) {
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_draw_summary",
        message = "inline chat draw payload prepared",
        outcome = "prepared",
        status = ?app.status,
        mode = app.mode.as_ref().map_or_else(|| "none".to_owned(), |mode| mode.current_mode_name.clone()),
        terminal_width = app.chat_render.terminal_width,
        terminal_height = app.chat_render.terminal_height,
        anchor_valid = app.chat_render.live_region.anchor_valid,
        full_rebuild,
        transcript_rows = transcript_rows.len(),
        live_rows_total = live_rows_total.len(),
        live_rows_visible = live_rows_visible.len(),
        live_rows_hidden_above,
        composer_rows_total = composer_rows_total.len(),
        composer_rows_visible = composer_rows_visible.len(),
        transcript_preview = %preview_rows(transcript_rows, 3),
        live_preview = %preview_rows(live_rows_visible, 3),
        composer_preview = %preview_rows(composer_rows_visible, 3),
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
    use super::{ComposerRows, MutableLayoutPlan, RowWindow};
    use ratatui::layout::Rect;
    use ratatui::text::Line;

    fn rows(count: usize) -> Vec<Line<'static>> {
        (0..count).map(|idx| Line::raw(format!("row {idx}"))).collect()
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
        let composer_rows = ComposerRows { rows: rows(3), caret_row: 1, caret_col: 0 };
        let plan = MutableLayoutPlan::new(&live_rows, &composer_rows, 40);

        assert_eq!(plan.viewport_height, 40);
        assert_eq!(plan.live_window.visible_len_u16(), 37);
        assert_eq!(plan.live_window.hidden_rows_above(), 276);
        assert_eq!(plan.composer_window.visible_len_u16(), 3);
    }

    #[test]
    fn mutable_layout_areas_pin_composer_to_bottom() {
        let plan = MutableLayoutPlan {
            live_window: RowWindow { start: 20, visible_len: 37 },
            composer_window: RowWindow { start: 0, visible_len: 3 },
            viewport_height: 40,
        };
        let viewport_area = Rect::new(0, 10, 120, 40);
        let (live_area, composer_area) = plan.areas(viewport_area);

        assert_eq!(live_area, Rect::new(0, 10, 120, 37));
        assert_eq!(composer_area, Rect::new(0, 47, 120, 3));
    }

    #[test]
    fn composer_window_keeps_footer_and_caret_visible_when_composer_exceeds_screen() {
        let live_rows = rows(10);
        let composer_rows = ComposerRows { rows: rows(50), caret_row: 49, caret_col: 2 };
        let plan = MutableLayoutPlan::new(&live_rows, &composer_rows, 40);

        assert_eq!(plan.composer_window.hidden_rows_above(), 10);
        assert_eq!(plan.live_window.visible_len_u16(), 0);
        assert_eq!(plan.visible_composer_caret_row(49), 39);
    }
}
