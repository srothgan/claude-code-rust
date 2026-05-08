// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Content rendering for Execute/Bash tool calls.

use crate::agent::model;
use crate::app::ToolCallInfo;
use crate::ui::highlight;
use crate::ui::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{TOOL_BODY_MAX_LINES, errors::render_failed_tool_text_content};

/// Render Execute/Bash content lines WITHOUT any border decoration.
/// This is width-independent and safe to cache across resizes.
/// Returns command and output lines only; permission/question controls are appended
/// by the standard body renderer so they are never hidden by the content cap.
pub(super) fn render_execute_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    if let Some(ref cmd) = tc.terminal_command {
        let mut spans =
            vec![Span::styled("$ ", Style::default().fg(theme::DIM).add_modifier(Modifier::BOLD))];
        spans.push(Span::styled(cmd.clone(), Style::default().fg(theme::DIM)));
        lines.push(Line::from(spans));
    }

    let mut body_lines: Vec<Line<'static>> = Vec::new();

    if let Some(ref output) = tc.terminal_output {
        if let Some(failed_lines) = render_failed_tool_text_content(tc.status, output) {
            body_lines = failed_lines;
        } else {
            body_lines =
                highlight::render_terminal_output(output).into_iter().map(dim_line).collect();
        }
    } else if matches!(tc.status, model::ToolCallStatus::InProgress) {
        body_lines.push(Line::from(Span::styled("running...", Style::default().fg(theme::DIM))));
    }

    let output_budget = TOOL_BODY_MAX_LINES.saturating_sub(lines.len());
    lines.extend(cap_execute_output_lines(body_lines, output_budget));

    lines
}

fn cap_execute_output_lines(lines: Vec<Line<'static>>, max_lines: usize) -> Vec<Line<'static>> {
    if lines.len() <= max_lines {
        return lines;
    }
    if max_lines == 0 {
        return Vec::new();
    }
    let tail = max_lines.saturating_sub(1);
    let omitted = lines.len().saturating_sub(tail);
    let mut out = Vec::with_capacity(max_lines);
    out.push(Line::from(Span::styled(
        format!("... {omitted} lines hidden ..."),
        Style::default().fg(theme::DIM).add_modifier(Modifier::ITALIC),
    )));
    out.extend(lines.into_iter().skip(omitted));
    out
}

fn dim_line(line: Line<'static>) -> Line<'static> {
    Line::from(
        line.spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), Style::default().fg(theme::DIM)))
            .collect::<Vec<_>>(),
    )
}
