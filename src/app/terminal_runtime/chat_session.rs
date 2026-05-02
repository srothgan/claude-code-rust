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
use anyhow::{Context, anyhow};
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::terminal::{Clear, ClearType, DisableLineWrap};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear as RatatuiClear, Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::{Stdout, Write};

type StdoutBackend = CrosstermBackend<Stdout>;
type StdoutTerminal = Terminal<StdoutBackend>;

pub(super) struct ChatTerminalSession {
    terminal: Option<StdoutTerminal>,
    inline_height: u16,
    last_frame_area: Option<Rect>,
    has_committed_output: bool,
}

impl ChatTerminalSession {
    pub(super) fn new() -> anyhow::Result<Self> {
        let (width, height) =
            crossterm::terminal::size().context("failed to read chat terminal size")?;

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
        );

        Ok(Self {
            terminal: None,
            inline_height: 0,
            last_frame_area: None,
            has_committed_output: false,
        })
    }

    pub(super) fn clear(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
        app.reset_committed_output_tracking();
    }

    pub(super) fn clear_mutable_viewport(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
    }

    pub(super) fn prepare_for_fullscreen(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
    }

    pub(super) fn draw(&mut self, app: &mut App) -> anyhow::Result<()> {
        Self::ensure_line_wrap_disabled(app)?;

        let screen_size =
            crossterm::terminal::size().context("failed to read chat terminal size")?;
        let width = screen_size.0.max(1);
        let terminal_height = screen_size.1.max(1);
        app.chat_render.set_terminal_size(screen_size.0, screen_size.1);

        let transcript_plan = self.prepare_transcript_flush(app, width);
        let live_rows = if transcript_plan.inserted_ids.is_empty() {
            serialize_live_rows(app, width)
        } else {
            serialize_live_rows_after_static_insert(app, width, &transcript_plan.inserted_ids)
        };
        let composer_rows = Self::serialize_composer_rows(app, width);
        let layout_plan = MutableLayoutPlan::new(&live_rows, &composer_rows, terminal_height);
        let composer_rows_total = composer_rows.all_rows();
        let visible_input_rows = layout_plan.input_visible_rows(&composer_rows.input_rows).to_vec();
        let visible_footer_rows =
            layout_plan.footer_visible_rows(&composer_rows.footer_rows).to_vec();
        let visible_composer_rows =
            ComposerRows::join_visible_rows(&visible_input_rows, &visible_footer_rows);

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

        self.prepare_mutable_viewport_for_draw(layout_plan.viewport_height)?;
        self.insert_transcript_rows(&transcript_plan)?;
        complete_transcript_flush(app, &transcript_plan);
        self.has_committed_output = self.has_committed_output || !transcript_plan.rows.is_empty();

        let visible_live_rows = layout_plan.live_visible_rows(&live_rows).to_vec();
        let visible_live_row_count = visible_live_rows.len();
        let visible_composer_row_count = visible_composer_rows.len();
        let input_caret_row = layout_plan.visible_input_caret_row(composer_rows.caret_row);
        let terminal = self
            .terminal
            .as_mut()
            .ok_or_else(|| anyhow!("inline chat terminal missing before draw"))?;
        let mut viewport_area = Rect::new(0, 0, 0, 0);
        terminal
            .draw(|frame| {
                viewport_area = frame.area();
                frame.render_widget(RatatuiClear, viewport_area);
                let (live_area, input_area, footer_area) = layout_plan.areas(viewport_area);
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
            .context("failed to draw ratatui inline chat viewport")?;

        let (live_area, input_area, footer_area) = layout_plan.areas(viewport_area);
        self.last_frame_area = Some(viewport_area);
        app.chat_render.live_region.anchor_valid = true;
        app.chat_render.live_region.total_rows = u16::try_from(live_rows.len()).unwrap_or(u16::MAX);
        app.chat_render.live_region.hidden_rows_above =
            u16::try_from(layout_plan.live_window.hidden_rows_above()).unwrap_or(u16::MAX);
        app.chat_render.live_region.viewport_height = viewport_area.height;
        app.chat_render.live_region.last_rendered_rows =
            u16::try_from(visible_live_row_count).unwrap_or(u16::MAX);
        app.chat_render.composer.last_rendered_rows =
            u16::try_from(visible_composer_row_count).unwrap_or(u16::MAX);

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_viewport_draw",
            message = "ratatui inline viewport repainted with mutable chat rows",
            outcome = "success",
            viewport_top = viewport_area.top(),
            viewport_height = viewport_area.height,
            live_top = live_area.top(),
            live_height = live_area.height,
            composer_top = input_area.top(),
            composer_height = input_area.height.saturating_add(footer_area.height),
            footer_top = footer_area.top(),
            footer_height = footer_area.height,
            requested_inline_height = layout_plan.viewport_height,
            terminal_width = screen_size.0,
            terminal_height = screen_size.1,
            mutable_rows = visible_live_row_count + visible_composer_row_count,
            live_rows_total = live_rows.len(),
            live_rows_visible = visible_live_row_count,
            live_rows_hidden_above = layout_plan.live_window.hidden_rows_above(),
            composer_rows_total = composer_rows.total_len(),
            composer_rows_visible = visible_composer_row_count,
            caret_row = input_area.y.saturating_add(input_caret_row),
            caret_col = input_area.x.saturating_add(composer_rows.caret_col),
        );

        app.surface_dirty.chat.take_repaint();
        Ok(())
    }

    fn reset_inline_terminal(&mut self, app: &mut App) {
        if let Err(err) = self.clear_previous_mutable_viewport() {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "inline_chat_mutable_viewport_clear_failed",
                message = "failed to clear mutable viewport before inline terminal reset",
                outcome = "failure",
                error_message = %err,
            );
        }
        self.terminal = None;
        self.inline_height = 0;
        // Keep the cleared anchor so the next inline terminal is recreated in
        // the same screen slot instead of bottom-aligning and leaving a gap.
        self.has_committed_output = false;
        app.chat_render.invalidate_live_anchor();
    }

    fn ensure_inline_terminal_height(&mut self, desired_height: u16) -> anyhow::Result<()> {
        let next_height = desired_height.max(1);
        if self.terminal.is_some() && self.inline_height == next_height {
            return Ok(());
        }

        if let Some(area) = self.last_frame_area {
            move_cursor_to(area).context("failed to restore inline viewport anchor")?;
        }

        let mut terminal = create_inline_terminal(next_height)?;
        terminal.clear().context("failed to clear new inline terminal viewport")?;
        self.terminal = Some(terminal);
        self.inline_height = next_height;
        Ok(())
    }

    fn prepare_mutable_viewport_for_draw(&mut self, desired_height: u16) -> anyhow::Result<()> {
        self.ensure_inline_terminal_height(desired_height)
    }

    fn clear_previous_mutable_viewport(&self) -> anyhow::Result<()> {
        let Some(area) = self.last_frame_area else {
            return Ok(());
        };
        clear_area_rows(area).context("failed to clear previous inline viewport rows")
    }

    fn insert_transcript_rows(&mut self, plan: &TranscriptFlushPlan) -> anyhow::Result<()> {
        if plan.rows.is_empty() {
            return Ok(());
        }

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_committed_insert_request",
            message = "committed transcript rows scheduled for ratatui inline insertion",
            outcome = "prepared",
            flushed_rows = plan.rows.len(),
            full_rebuild = plan.full_rebuild,
            preview = %preview_rows(&plan.rows, 4),
        );

        let row_count = u16::try_from(plan.rows.len()).unwrap_or(u16::MAX).max(1);
        let rows = plan.rows.clone();
        let terminal = self
            .terminal
            .as_mut()
            .ok_or_else(|| anyhow!("inline chat terminal missing before transcript insert"))?;
        terminal
            .insert_before(row_count, |buffer| {
                Paragraph::new(rows).render(buffer.area, buffer);
            })
            .context("failed to insert committed transcript above inline viewport")?;

        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_committed_insert_applied",
            message = "committed transcript rows inserted before ratatui inline viewport",
            outcome = "success",
            inserted_rows = row_count,
        );
        Ok(())
    }

    fn ensure_line_wrap_disabled(app: &mut App) -> anyhow::Result<()> {
        if app.chat_render.line_wrap_disabled {
            return Ok(());
        }
        let mut stdout = std::io::stdout();
        queue!(stdout, DisableLineWrap).context("failed to disable inline viewport line wrap")?;
        stdout.flush().context("failed to flush line-wrap disable")?;
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
            rows: serialize_transcript_rows(app, &entries, self.has_committed_output, width),
            inserted_ids,
            full_rebuild: false,
        }
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

fn clear_area_rows(area: Rect) -> anyhow::Result<()> {
    if area.is_empty() {
        return Ok(());
    }

    let mut stdout = std::io::stdout();
    for offset in 0..area.height {
        queue!(
            stdout,
            MoveTo(area.x, area.y.saturating_add(offset)),
            Clear(ClearType::CurrentLine)
        )
        .context("failed to queue inline row clear")?;
    }
    stdout.flush().context("failed to flush inline row clear")?;
    Ok(())
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
            .min(screen_height.max(1));

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
