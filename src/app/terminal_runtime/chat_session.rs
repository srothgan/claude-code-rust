// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::chat_terminal::{ChatDrawRequest, ChatTerminal};
use crate::app::{App, AppStatus};
use crate::ui::footer_rows::serialize_footer_rows;
use crate::ui::inline_chat_rows::{SerializedLiveRows, serialize_live_rows_with_boundaries};
use crate::ui::input;
use crate::ui::input_rows::{blocked_input_lines, build_composer_hint_rows};
use crate::ui::theme;
use anyhow::Context;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub(super) struct ChatTerminalSession {
    terminal: ChatTerminal,
    scrollback: ScrollbackCommitState,
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

        Ok(Self {
            terminal: ChatTerminal::new(owned_top),
            scrollback: ScrollbackCommitState::default(),
        })
    }

    pub(super) fn clear(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
        self.scrollback.reset();
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
        self.scrollback.reset();
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
        self.scrollback.reset();
    }

    pub(super) fn prepare_for_fullscreen(&mut self, app: &mut App) {
        self.reset_inline_terminal(app);
    }

    pub(super) fn draw(&mut self, app: &mut App) -> anyhow::Result<()> {
        ChatTerminal::ensure_line_wrap_disabled(&mut app.chat_render.line_wrap_disabled)?;

        let screen_size =
            crossterm::terminal::size().context("failed to read chat terminal size")?;
        let width = screen_size.0.max(1);
        let terminal_height = screen_size.1.max(1);
        app.chat_render.set_terminal_size(screen_size.0, screen_size.1);

        let serialized_rows = serialize_live_rows_with_boundaries(app, width);
        let mutable_msg_idx = active_mutable_message_idx(app);
        let stable_row_count = serialized_rows.stable_row_count_before_message(mutable_msg_idx);
        let scrollback_plan =
            self.scrollback.prepare(serialized_rows.rows(), stable_row_count, width);
        let live_rows =
            serialized_rows.rows().get(scrollback_plan.committed_after..).unwrap_or_default();
        let composer = Self::build_composer_surface(app, width);
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
            serialized_rows: &serialized_rows,
            live_rows,
            layout_plan,
            composer: &composer,
            visible_composer_row_count,
            composer_preview_rows: &composer_preview_rows,
            scrollback_plan: &scrollback_plan,
        });

        let visible_live_rows = layout_plan.live_visible_rows(live_rows).to_vec();
        let visible_live_row_count = visible_live_rows.len();
        let chat_frame = ChatDrawRequest {
            requested_inline_height: requested_layout_plan.viewport_height,
            terminal_width: width,
            terminal_height,
        };
        let outcome_result = self.terminal.draw_chat_frame(
            chat_frame,
            &scrollback_plan.rows,
            |frame, viewport_area| {
                let (live_area, hint_area, editor_area, footer_area) =
                    layout_plan.areas(viewport_area);
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
            },
        );
        let outcome = match outcome_result {
            Ok(outcome) => outcome,
            Err(err) => {
                mark_chat_terminal_history_out_of_sync(app);
                return Err(err);
            }
        };
        if outcome.inserted_scrollback_rows == scrollback_plan.rows.len() {
            self.scrollback.complete(scrollback_plan.committed_after);
        }
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
                live_rows_mutable: live_rows.len(),
                live_rows_visible: visible_live_row_count,
                live_rows_hidden_above: scrollback_plan
                    .committed_after
                    .saturating_add(layout_plan.live_window.hidden_rows_above()),
                composer_rows_total: composer.total_len(),
                composer_rows_visible: visible_composer_row_count,
                scrollback_inserted_rows: outcome.inserted_scrollback_rows,
            },
        );

        app.surface_dirty.chat.take_repaint();
        Ok(())
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
    app.request_chat_visible_rebuild();
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
            .scrollback_plan
            .committed_after
            .saturating_add(prepared.layout_plan.live_window.hidden_rows_above()),
        scrollback_commit_rows: prepared.scrollback_plan.rows.len(),
        scrollback_committed_after: prepared.scrollback_plan.committed_after,
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
    scrollback_plan: &'a ScrollbackCommitPlan,
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

fn active_mutable_message_idx(app: &App) -> Option<usize> {
    (app.is_compacting || matches!(app.status, AppStatus::Thinking | AppStatus::Running))
        .then(|| app.active_turn_assistant_idx())
        .flatten()
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ScrollbackCommitState {
    width: u16,
    committed_rows: usize,
}

impl ScrollbackCommitState {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn prepare(
        &mut self,
        rows: &[Line<'static>],
        stable_row_count: usize,
        width: u16,
    ) -> ScrollbackCommitPlan {
        let width = width.max(1);
        let stable_row_count = stable_row_count.min(rows.len());

        if self.width == 0 {
            self.width = width;
        } else if self.width != width {
            self.width = width;
            self.committed_rows = stable_row_count;
            return ScrollbackCommitPlan::empty(stable_row_count);
        }

        if self.committed_rows > rows.len() {
            self.committed_rows = stable_row_count;
            return ScrollbackCommitPlan::empty(stable_row_count);
        }

        let start = self.committed_rows.min(stable_row_count);
        ScrollbackCommitPlan {
            rows: rows[start..stable_row_count].to_vec(),
            committed_after: stable_row_count,
        }
    }

    fn complete(&mut self, committed_after: usize) {
        self.committed_rows = self.committed_rows.max(committed_after);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScrollbackCommitPlan {
    rows: Vec<Line<'static>>,
    committed_after: usize,
}

impl ScrollbackCommitPlan {
    fn empty(committed_after: usize) -> Self {
        Self { rows: Vec::new(), committed_after }
    }
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
    scrollback_commit_rows: usize,
    scrollback_committed_after: usize,
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
        scrollback_commit_rows = summary.scrollback_commit_rows,
        scrollback_committed_after = summary.scrollback_committed_after,
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
    use super::{ComposerEditor, ComposerSurface, MutableLayoutPlan, ScrollbackCommitState};
    use crate::app::terminal_runtime::chat_terminal::plan_inline_geometry;
    use ratatui::layout::Rect;
    use ratatui::text::Line;

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

    #[test]
    fn scrollback_commit_state_commits_stable_prefix_once() {
        let rows = rows(5);
        let mut state = ScrollbackCommitState::default();

        let first = state.prepare(&rows, 3, 80);
        assert_eq!(first.rows, rows[0..3]);
        assert_eq!(first.committed_after, 3);
        state.complete(first.committed_after);

        let repeated = state.prepare(&rows, 3, 80);
        assert!(repeated.rows.is_empty());
        assert_eq!(repeated.committed_after, 3);

        let extended = state.prepare(&rows, 5, 80);
        assert_eq!(extended.rows, rows[3..5]);
        assert_eq!(extended.committed_after, 5);
    }

    #[test]
    fn scrollback_commit_state_does_not_duplicate_rows_after_width_change() {
        let rows = rows(6);
        let mut state = ScrollbackCommitState::default();

        let first = state.prepare(&rows, 3, 80);
        state.complete(first.committed_after);

        let width_change = state.prepare(&rows, 5, 100);
        assert!(width_change.rows.is_empty());
        assert_eq!(width_change.committed_after, 5);
        state.complete(width_change.committed_after);

        let next = state.prepare(&rows, 6, 100);
        assert_eq!(next.rows, rows[5..6]);
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
