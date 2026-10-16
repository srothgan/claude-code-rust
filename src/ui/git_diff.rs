// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use crate::app::diff_view::{Comparison, DiffView, Loading, RenderedContent};
use crate::ui::wrap::{display_width, split_line_at_width};
use crate::ui::{diff, highlight, theme};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::collections::BTreeSet;
use std::path::Path;

pub(crate) fn render(frame: &mut Frame, app: &mut App) {
    let Some(state) = &mut app.diff_view else { return };
    let area = frame.area();
    if area.width < 48 || area.height < 10 {
        frame.render_widget(Paragraph::new("Resize to at least 48 columns and 10 rows to view Git changes.\nEsc back/close | Ctrl+Q quit").wrap(Wrap { trim: false }), area);
        return;
    }
    let help = "Up/Down navigate | Enter/Right read | Left/Esc back | Tab/Shift+Tab files | v diff/full file | s comparison | r refresh | [ prev hunk | ] next hunk | Ctrl+Q quit";
    let help_height =
        u16::try_from(Paragraph::new(help).wrap(Wrap { trim: false }).line_count(area.width))
            .unwrap_or(5)
            .min(area.height.saturating_sub(5));
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(3), Constraint::Length(help_height)])
        .split(area);
    render_summary(frame, sections[0], state);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length((area.width / 3).clamp(18, 48)), Constraint::Min(1)])
        .split(sections[1]);
    render_tree(frame, columns[0], state);
    render_content(frame, columns[1], state);
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(theme::DIM)).wrap(Wrap { trim: false }),
        sections[2],
    );
    if let Some(selected) = state.comparison_picker {
        render_comparison_picker(frame, area, selected);
    }
}

fn render_summary(frame: &mut Frame, area: Rect, state: &DiffView) {
    let (count, added, removed, binary) =
        state.snapshot.as_ref().map_or((0, 0, 0, 0), |snapshot| {
            snapshot.files.iter().fold(
                (
                    snapshot.files.iter().map(|file| &file.path).collect::<BTreeSet<_>>().len(),
                    0,
                    0,
                    0,
                ),
                |(count, added, removed, binary), file| {
                    file.counts.map_or((count, added, removed, binary + 1), |(a, r)| {
                        (count, added + a, removed + r, binary)
                    })
                },
            )
        });
    let totals = if state.loading == Some(Loading::Files) {
        "Loading Git changes...".into()
    } else {
        format!(
            "{count} {}  +{added} -{removed}{}",
            if count == 1 { "file" } else { "files" },
            if binary == 0 { String::new() } else { format!("  {binary} binary/unavailable") }
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("Git diff | {} | ", state.comparison.label()),
                    Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
                ),
                Span::raw(totals),
            ]),
            Line::from(Span::styled(
                state.comparison.description(),
                Style::default().fg(theme::DIM),
            )),
        ]),
        area,
    );
}

struct TreeRow {
    label: String,
    depth: usize,
    file_index: Option<usize>,
    counts: Line<'static>,
}

fn tree_rows(state: &DiffView) -> Vec<TreeRow> {
    let mut folders = BTreeSet::new();
    let mut rows = Vec::new();
    if let Some(snapshot) = &state.snapshot {
        for (index, file) in snapshot.files.iter().enumerate() {
            let parts: Vec<_> = file.path.split('/').collect();
            for depth in 0..parts.len().saturating_sub(1) {
                if folders.insert(parts[..=depth].join("/")) {
                    rows.push(TreeRow {
                        label: format!("{}/", visible(parts[depth])),
                        depth,
                        file_index: None,
                        counts: Line::default(),
                    });
                }
            }
            rows.push(TreeRow {
                label: format!(
                    "{} {}",
                    file.status,
                    visible(parts.last().copied().unwrap_or(&file.path))
                ),
                depth: parts.len().saturating_sub(1),
                file_index: Some(index),
                counts: file.counts.map_or_else(
                    || Line::from("binary/-"),
                    |(added, removed)| {
                        diff::render_diff_count_header(added, removed).unwrap_or_default()
                    },
                ),
            });
        }
    }
    rows
}

fn pane(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { theme::RUST_ORANGE } else { theme::DIM }))
}

fn render_tree(frame: &mut Frame, area: Rect, state: &mut DiffView) {
    let block = pane(" Files ".into(), !state.content_focus);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = tree_rows(state);
    let selected_row =
        rows.iter().position(|row| row.file_index == Some(state.selected)).unwrap_or(0);
    let height = usize::from(inner.height);
    state.tree_scroll = state.tree_scroll.min(rows.len().saturating_sub(height));
    if selected_row < state.tree_scroll {
        state.tree_scroll = selected_row;
    }
    if selected_row >= state.tree_scroll + height {
        state.tree_scroll = (selected_row + 1).saturating_sub(height);
    }
    let lines: Vec<_> = rows
        .iter()
        .skip(state.tree_scroll)
        .take(height)
        .map(|row| {
            let selected = row.file_index == Some(state.selected);
            let style = if selected {
                Style::default()
                    .bg(theme::RUST_ORANGE)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else if row.file_index.is_none() {
                Style::default().fg(theme::DIM)
            } else {
                Style::default()
            };
            let counts_width = row.counts.width();
            let width = usize::from(inner.width);
            let label_width = width.saturating_sub(counts_width + usize::from(counts_width > 0));
            let indent = " ".repeat((row.depth * 2).min(label_width / 3));
            let label =
                format!("{indent}{}", fit(&row.label, label_width.saturating_sub(indent.len())));
            let gap = width.saturating_sub(display_width(&label) + counts_width);
            let mut spans = vec![Span::styled(format!("{label}{}", " ".repeat(gap)), style)];
            spans.extend(row.counts.spans.iter().cloned());
            Line::from(spans)
        })
        .collect();
    if rows.is_empty() {
        let message = if state.loading == Some(Loading::Files) {
            "Loading..."
        } else if state.error.is_some() {
            "Git unavailable"
        } else {
            "No changes"
        };
        frame.render_widget(Paragraph::new(message).wrap(Wrap { trim: false }), inner);
    } else {
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

fn render_content(frame: &mut Frame, area: Rect, state: &mut DiffView) {
    let mode = if state.full_file { "Full file" } else { "Diff" };
    let path = state.selected_file().map_or_else(String::new, |file| visible(&file.path));
    let deleted = state.content.as_ref().is_some_and(|content| content.deleted);
    let status = state.selected_file().map_or(' ', |file| file.status);
    let title = format!(
        " {mode} | {status} {path}{} ",
        if deleted && state.full_file { " (before deletion)" } else { "" }
    );
    let block = pane(title, state.content_focus);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    state.viewport_height = usize::from(inner.height).max(1);
    if let Some(error) = &state.error {
        frame.render_widget(
            Paragraph::new(error.as_str())
                .style(Style::default().fg(theme::STATUS_ERROR))
                .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }
    if state.loading.is_some() {
        frame.render_widget(
            Paragraph::new("Loading...").style(Style::default().fg(theme::DIM)),
            inner,
        );
        return;
    }
    let Some(content) = &state.content else {
        frame.render_widget(
            Paragraph::new("No changes in this comparison.").wrap(Wrap { trim: false }),
            inner,
        );
        return;
    };
    if state
        .rendered
        .as_ref()
        .is_none_or(|cache| cache.width != inner.width || cache.full_file != state.full_file)
    {
        let (mut lines, hunks) = if state.full_file {
            (
                content
                    .full_text
                    .as_ref()
                    .map_or_else(Vec::new, |text| full_file_lines(text, &path, inner.width)),
                Vec::new(),
            )
        } else {
            diff::render_repository_patch(&content.patch, inner.width)
        };
        if lines.is_empty() {
            lines.push(Line::from(
                content
                    .notice
                    .clone()
                    .unwrap_or_else(|| "No textual changes (file metadata changed).".into()),
            ));
        } else if let Some(notice) = &content.notice {
            if state.full_file {
                lines.insert(0, Line::from(notice.clone()));
            } else {
                lines.push(Line::from(notice.clone()));
            }
        }
        state.rendered =
            Some(RenderedContent { width: inner.width, full_file: state.full_file, lines, hunks });
    }
    if let Some(rendered) = &state.rendered {
        let scroll = &mut state.scroll[usize::from(state.full_file)];
        // Every hunk may start at the top, while ordinary scrolling stops at EOF.
        let end = rendered.lines.len().saturating_sub(state.viewport_height);
        let last_hunk = rendered.hunks.last().copied().unwrap_or(0);
        *scroll = (*scroll).min(end.max(last_hunk));
        // Slice instead of Paragraph::scroll so files can exceed u16::MAX rows.
        let lines = rendered
            .lines
            .iter()
            .skip(*scroll)
            .take(state.viewport_height)
            .cloned()
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

fn full_file_lines(text: &str, path: &str, width: u16) -> Vec<Line<'static>> {
    let number_width = text.lines().count().max(1).to_string().len();
    let content_width = usize::from(width).saturating_sub(number_width + 2).max(1);
    let highlighted =
        highlight::highlight_code(text, Path::new(path).extension().and_then(|ext| ext.to_str()));
    highlighted
        .into_iter()
        .enumerate()
        .flat_map(|(index, line)| {
            split_line_at_width(line, content_width)
                .into_iter()
                .enumerate()
                .map(move |(row, mut line)| {
                    let number = if row == 0 {
                        format!("{:>number_width$}  ", index + 1)
                    } else {
                        " ".repeat(number_width + 2)
                    };
                    line.spans.insert(0, Span::styled(number, Style::default().fg(theme::DIM)));
                    line
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn render_comparison_picker(frame: &mut Frame, area: Rect, selected: usize) {
    let width = area.width.min(64);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + area.height.saturating_sub(7) / 2,
        width,
        7.min(area.height),
    );
    frame.render_widget(Clear, popup);
    let block = pane(" Comparison | Enter select | Esc cancel ".into(), true);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let lines = Comparison::ALL
        .iter()
        .enumerate()
        .map(|(index, comparison)| {
            Line::from(Span::styled(
                format!(
                    "{} {}: {}",
                    if index == selected { ">" } else { " " },
                    comparison.label(),
                    comparison.description()
                ),
                if index == selected {
                    Style::default().fg(Color::Black).bg(theme::RUST_ORANGE)
                } else {
                    Style::default()
                },
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn visible(text: &str) -> String {
    text.chars().map(|ch| if ch.is_control() { '\u{fffd}' } else { ch }).collect()
}

fn fit(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.into();
    }
    let mut result = String::new();
    for ch in text.chars() {
        if display_width(&result) + unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) >= width
        {
            break;
        }
        result.push(ch);
    }
    if width > 0 {
        result.push('…');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_file_wrapping_preserves_source_whitespace_and_gutter_alignment() {
        let lines = full_file_lines("    let x = \"a  b\";", "src/lib.rs", 14);
        let text = lines.iter().map(ToString::to_string).collect::<Vec<_>>();

        assert_eq!(text, ["1      let x =", "    \"a  b\";"]);
        assert!(lines[0].spans.iter().skip(1).any(|span| span.style.fg.is_some()));
    }
}
