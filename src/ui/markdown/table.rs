// SPDX-License-Identifier: Apache-2.0

//! Table layout for Markdown tables parsed by the document renderer.

use super::super::wrap::{
    StyledChunk, blank_line, display_width, line_display_width, pad_line_to_width,
    wrap_styled_chunks,
};
use pulldown_cmark::Alignment;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ColumnAlignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TableLayoutMode {
    Grid,
    DenseGrid,
    Stacked,
}

#[derive(Clone, Copy, Debug)]
struct TableRenderPolicy {
    preferred_spacing: usize,
    min_spacing: usize,
    min_column_width: usize,
    allow_stacked_fallback: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResolvedTableLayout {
    mode: TableLayoutMode,
    column_widths: Vec<usize>,
    spacing: usize,
}

#[derive(Clone, Copy)]
struct ColumnMetrics {
    preferred: usize,
    soft_min: usize,
}

#[derive(Clone, Debug, Default)]
struct TableRowAst {
    cells: Vec<TableCellAst>,
}

#[derive(Clone, Debug, Default)]
struct TableCellAst {
    chunks: Vec<StyledChunk>,
    plain_text: String,
    preferred_width: usize,
    soft_min_width: usize,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DocumentTable {
    header: TableRowAst,
    rows: Vec<TableRowAst>,
    alignments: Vec<ColumnAlignment>,
}

const URL_TOKEN_SOFT_MIN_WIDTH: usize = 24;

impl DocumentTable {
    pub(super) fn new(
        alignments: &[Alignment],
        header: Vec<Vec<StyledChunk>>,
        rows: Vec<Vec<Vec<StyledChunk>>>,
    ) -> Self {
        let row = |cells: Vec<Vec<StyledChunk>>| TableRowAst {
            cells: cells.into_iter().map(TableCellAst::from_chunks).collect(),
        };
        Self {
            header: row(header),
            rows: rows.into_iter().map(row).collect(),
            alignments: alignments.iter().copied().map(ColumnAlignment::from).collect(),
        }
    }

    fn column_count(&self) -> usize {
        let body_cols = self.rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
        self.header.cells.len().max(self.alignments.len()).max(body_cols)
    }

    pub(super) fn render_lines(
        &self,
        width: u16,
        bg: Option<Color>,
        control: &crate::ui::layout_control::LayoutControl<'_>,
    ) -> Vec<Line<'static>> {
        if self.column_count() == 0 || width == 0 {
            return Vec::new();
        }

        let policy = TableRenderPolicy {
            preferred_spacing: 3,
            min_spacing: 1,
            min_column_width: 4,
            allow_stacked_fallback: true,
        };
        let layout = resolve_layout(self, usize::from(width), policy);
        match layout.mode {
            TableLayoutMode::Grid | TableLayoutMode::DenseGrid => {
                render_grid_lines(self, &layout, bg, control)
            }
            TableLayoutMode::Stacked => render_stacked_lines(self, usize::from(width), bg, control),
        }
    }
}

impl TableCellAst {
    fn empty() -> Self {
        Self::default()
    }

    fn from_chunks(chunks: Vec<StyledChunk>) -> Self {
        let plain_text: String = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
        let (preferred_width, soft_min_width) = measure_cell_widths(&plain_text);
        Self {
            chunks,
            plain_text,
            preferred_width: preferred_width.max(1),
            soft_min_width: soft_min_width.max(1),
        }
    }
}

fn resolve_layout(
    table: &DocumentTable,
    total_width: usize,
    policy: TableRenderPolicy,
) -> ResolvedTableLayout {
    let cols = table.column_count();
    if cols == 0 {
        return ResolvedTableLayout {
            mode: TableLayoutMode::Stacked,
            column_widths: Vec::new(),
            spacing: 0,
        };
    }

    let metrics = collect_column_metrics(table, cols);
    if let Some(layout) = resolve_grid_layout(
        &metrics,
        total_width,
        policy.preferred_spacing,
        TableLayoutMode::Grid,
        policy.min_column_width,
    ) {
        return layout;
    }
    if let Some(layout) = resolve_grid_layout(
        &metrics,
        total_width,
        policy.min_spacing,
        TableLayoutMode::DenseGrid,
        policy.min_column_width,
    ) {
        return layout;
    }

    let _ = policy.allow_stacked_fallback;
    ResolvedTableLayout { mode: TableLayoutMode::Stacked, column_widths: Vec::new(), spacing: 0 }
}

fn resolve_grid_layout(
    metrics: &[ColumnMetrics],
    total_width: usize,
    spacing: usize,
    mode: TableLayoutMode,
    min_column_width: usize,
) -> Option<ResolvedTableLayout> {
    if metrics.is_empty() {
        return Some(ResolvedTableLayout { mode, column_widths: Vec::new(), spacing });
    }

    let spacing_budget = spacing.saturating_mul(metrics.len().saturating_sub(1));
    let available = total_width.saturating_sub(spacing_budget);
    let soft_floor_total: usize =
        metrics.iter().map(|metric| metric.soft_min.max(min_column_width)).sum();
    if available < soft_floor_total {
        return None;
    }
    if available < min_column_width.saturating_mul(metrics.len()) {
        return None;
    }

    let column_widths = solve_column_widths(metrics, available, min_column_width);
    Some(ResolvedTableLayout { mode, column_widths, spacing })
}

fn measure_cell_widths(text: &str) -> (usize, usize) {
    let preferred = text.lines().map(display_width).max().unwrap_or(0);
    let soft_min = text
        .lines()
        .flat_map(|line| line.split_whitespace())
        .map(token_soft_min_width)
        .max()
        .unwrap_or(preferred);
    (preferred, soft_min)
}

fn token_soft_min_width(token: &str) -> usize {
    let width = display_width(token);
    if is_url_like_token(token) { width.min(URL_TOKEN_SOFT_MIN_WIDTH) } else { width }
}

fn is_url_like_token(token: &str) -> bool {
    token.starts_with("http://") || token.starts_with("https://") || token.starts_with("www.")
}

fn collect_column_metrics(table: &DocumentTable, cols: usize) -> Vec<ColumnMetrics> {
    let mut metrics = vec![ColumnMetrics { preferred: 1, soft_min: 1 }; cols];
    for row in std::iter::once(&table.header).chain(table.rows.iter()) {
        for (idx, cell) in row.cells.iter().enumerate() {
            metrics[idx].preferred = metrics[idx].preferred.max(cell.preferred_width);
            metrics[idx].soft_min = metrics[idx].soft_min.max(cell.soft_min_width);
        }
    }
    metrics
}

fn solve_column_widths(
    metrics: &[ColumnMetrics],
    available: usize,
    min_column_width: usize,
) -> Vec<usize> {
    if metrics.is_empty() {
        return Vec::new();
    }

    let mut widths: Vec<usize> =
        metrics.iter().map(|metric| metric.preferred.max(min_column_width)).collect();
    let soft_floor: Vec<usize> = metrics
        .iter()
        .zip(widths.iter())
        .map(|(metric, width)| metric.soft_min.clamp(min_column_width, *width))
        .collect();

    reduce_widths(&mut widths, available, &soft_floor);
    if widths.iter().sum::<usize>() > available {
        let hard_floor = vec![min_column_width; widths.len()];
        reduce_widths(&mut widths, available, &hard_floor);
    }
    widths
}

fn reduce_widths(widths: &mut [usize], available: usize, floor: &[usize]) {
    while widths.iter().sum::<usize>() > available {
        let candidate = widths
            .iter()
            .enumerate()
            .filter(|(idx, width)| **width > floor[*idx])
            .max_by_key(|(idx, width)| (*width - floor[*idx], *width, std::cmp::Reverse(*idx)));
        let Some((idx, _)) = candidate else {
            break;
        };
        widths[idx] -= 1;
    }
}

fn render_grid_lines(
    table: &DocumentTable,
    layout: &ResolvedTableLayout,
    bg: Option<Color>,
    control: &crate::ui::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let header_style = bg.map_or_else(
        || Style::default().add_modifier(Modifier::BOLD),
        |bg_color| Style::default().bg(bg_color).add_modifier(Modifier::BOLD),
    );
    let row_style = bg.map_or_else(Style::default, |bg_color| Style::default().bg(bg_color));

    let mut lines = render_row_lines(
        &table.header,
        &layout.column_widths,
        &table.alignments,
        header_style,
        layout.spacing,
    );
    if !lines.is_empty() {
        lines.push(render_separator_line(&layout.column_widths, layout.spacing, row_style));
    }
    for row in &table.rows {
        if control.cancelled() {
            return Vec::new();
        }
        lines.extend(render_row_lines(
            row,
            &layout.column_widths,
            &table.alignments,
            row_style,
            layout.spacing,
        ));
    }
    lines
}

fn render_row_lines(
    row: &TableRowAst,
    widths: &[usize],
    alignments: &[ColumnAlignment],
    base_style: Style,
    spacing: usize,
) -> Vec<Line<'static>> {
    let mut cell_lines = Vec::with_capacity(widths.len());
    let mut row_height = 1usize;

    for (idx, width) in widths.iter().copied().enumerate() {
        let alignment = alignments.get(idx).copied().unwrap_or(ColumnAlignment::Left);
        let cell = row.cells.get(idx).cloned().unwrap_or_else(TableCellAst::empty);
        let rendered = render_cell_lines(&cell, width, alignment, base_style);
        row_height = row_height.max(rendered.len());
        cell_lines.push(rendered);
    }

    for (idx, width) in widths.iter().copied().enumerate() {
        while cell_lines[idx].len() < row_height {
            cell_lines[idx].push(blank_line(width, base_style));
        }
    }

    let mut lines = Vec::with_capacity(row_height);
    for line_idx in 0..row_height {
        let mut spans = Vec::new();
        for (idx, cell) in cell_lines.iter().enumerate() {
            if idx > 0 {
                spans.push(Span::styled(" ".repeat(spacing), base_style));
            }
            spans.extend(cell[line_idx].spans.clone());
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn render_separator_line(widths: &[usize], spacing: usize, base_style: Style) -> Line<'static> {
    let separator_style = base_style.add_modifier(Modifier::DIM);
    let mut spans = Vec::new();
    for (idx, width) in widths.iter().copied().enumerate() {
        if idx > 0 {
            spans.push(Span::styled(" ".repeat(spacing), base_style));
        }
        spans.push(Span::styled("─".repeat(width), separator_style));
    }
    Line::from(spans)
}

fn render_cell_lines(
    cell: &TableCellAst,
    width: usize,
    alignment: ColumnAlignment,
    base_style: Style,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }
    if cell.chunks.is_empty() {
        return vec![blank_line(width, base_style)];
    }

    wrap_styled_chunks(
        &cell
            .chunks
            .iter()
            .map(|chunk| StyledChunk {
                text: chunk.text.clone(),
                style: base_style.patch(chunk.style),
            })
            .collect::<Vec<_>>(),
        width,
    )
    .into_iter()
    .map(|line| align_line_to_width(line, width, alignment, base_style))
    .collect()
}

fn align_line_to_width(
    mut line: Line<'static>,
    width: usize,
    alignment: ColumnAlignment,
    base_style: Style,
) -> Line<'static> {
    let content_width = line_display_width(&line);
    if content_width >= width {
        return line;
    }

    let padding = width - content_width;
    let (left_pad, right_pad) = match alignment {
        ColumnAlignment::Left => (0, padding),
        ColumnAlignment::Center => (padding / 2, padding - (padding / 2)),
        ColumnAlignment::Right => (padding, 0),
    };

    if left_pad > 0 {
        line.spans.insert(0, Span::styled(" ".repeat(left_pad), base_style));
    }
    if right_pad > 0 {
        line.spans.push(Span::styled(" ".repeat(right_pad), base_style));
    }
    line
}

fn render_stacked_lines(
    table: &DocumentTable,
    width: usize,
    bg: Option<Color>,
    control: &crate::ui::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let header_style = bg.map_or_else(
        || Style::default().add_modifier(Modifier::BOLD),
        |bg_color| Style::default().bg(bg_color).add_modifier(Modifier::BOLD),
    );
    let row_style = bg.map_or_else(Style::default, |bg_color| Style::default().bg(bg_color));
    let indent = 2usize;
    let mut lines = Vec::new();

    for (row_idx, row) in table.rows.iter().enumerate() {
        if control.cancelled() {
            return Vec::new();
        }
        if row_idx > 0 {
            lines.push(Line::default());
        }
        for col_idx in 0..table.column_count() {
            let label = table
                .header
                .cells
                .get(col_idx)
                .map(|cell| cell.plain_text.trim())
                .filter(|text| !text.is_empty())
                .map_or_else(|| format!("Column {}", col_idx + 1), str::to_owned);
            let value = row.cells.get(col_idx).cloned().unwrap_or_else(TableCellAst::empty);
            lines.extend(render_stacked_pair(
                &label,
                &value,
                width,
                indent,
                header_style,
                row_style,
            ));
        }
    }

    lines
}

fn render_stacked_pair(
    label: &str,
    value: &TableCellAst,
    width: usize,
    indent: usize,
    label_style: Style,
    value_style: Style,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::default()];
    }

    let inline_prefix = format!("{label}: ");
    let inline_width = display_width(&inline_prefix);
    let inline_value_width = width.saturating_sub(inline_width).max(1);
    let multiline_value_width = width.saturating_sub(indent).max(1);
    let styled_value = value
        .chunks
        .iter()
        .map(|chunk| StyledChunk {
            text: chunk.text.clone(),
            style: value_style.patch(chunk.style),
        })
        .collect::<Vec<_>>();

    if inline_value_width >= 8 {
        let value_lines = if styled_value.is_empty() {
            vec![Line::default()]
        } else {
            wrap_styled_chunks(&styled_value, inline_value_width)
        };
        let mut lines = Vec::new();
        for (idx, value_line) in value_lines.into_iter().enumerate() {
            let mut line = if idx == 0 {
                Line::from(vec![Span::styled(inline_prefix.clone(), label_style)])
            } else {
                Line::from(Span::styled(" ".repeat(inline_width), label_style))
            };
            line.spans.extend(value_line.spans);
            lines.push(pad_line_to_width(line, width, value_style));
        }
        return lines;
    }

    let mut lines = vec![Line::from(Span::styled(format!("{label}:"), label_style))];
    let indent_prefix = " ".repeat(indent);
    let value_lines = if styled_value.is_empty() {
        vec![Line::default()]
    } else {
        wrap_styled_chunks(&styled_value, multiline_value_width)
    };
    for value_line in value_lines {
        let mut line = Line::from(Span::styled(indent_prefix.clone(), value_style));
        line.spans.extend(value_line.spans);
        lines.push(pad_line_to_width(line, width, value_style));
    }
    lines
}

impl From<Alignment> for ColumnAlignment {
    fn from(value: Alignment) -> Self {
        match value {
            Alignment::Center => Self::Center,
            Alignment::Right => Self::Right,
            Alignment::Left | Alignment::None => Self::Left,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn render_lines(text: &str, width: u16) -> Vec<Line<'static>> {
        super::super::render_markdown_at_width(text, width, None, false)
    }

    fn render_strings(text: &str, width: u16) -> Vec<String> {
        render_lines(text, width)
            .into_iter()
            .map(|line| line.spans.into_iter().map(|span| span.content.into_owned()).collect())
            .collect()
    }

    #[test]
    fn code_fences_are_not_laid_out_as_tables() {
        let input = "```text\n| not | a table |\n| --- | --- |\n```\n";
        let rendered = render_strings(input, 40);
        assert!(rendered.iter().any(|line| line == "| not | a table |"));
        assert!(rendered.iter().any(|line| line == "| --- | --- |"));
        assert!(!rendered.iter().any(|line| line.contains('─')));
    }

    #[test]
    fn table_render_keeps_text_before_and_after() {
        let input = "Intro\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\nOutro";
        let rendered = render_strings(input, 40);
        assert!(rendered.iter().any(|line| line.contains("Intro")));
        assert!(rendered.iter().any(|line| line.contains("Outro")));
        assert!(rendered.iter().any(|line| line.contains('A')));
        assert!(rendered.iter().any(|line| line.contains('─')));
    }

    #[test]
    fn alignment_markers_preserve_column_order_and_relative_alignment() {
        let input = "| left | center | right |\n| :--- | :----: | ----: |\n| a | bb | c |\n";
        let rendered = render_strings(input, 32);
        let header = &rendered[0];
        let separator = &rendered[1];
        let row = &rendered[2];

        let left_header = header.find("left").expect("left header");
        let center_header = header.find("center").expect("center header");
        let right_header = header.find("right").expect("right header");
        assert!(left_header < center_header && center_header < right_header);

        assert!(!separator.trim().is_empty());
        assert!(separator.chars().any(|ch| !ch.is_whitespace()));

        let left_cell = row.find('a').expect("left cell");
        let center_cell = row.find("bb").expect("center cell");
        let right_cell = row.rfind('c').expect("right cell");
        assert!(left_cell <= left_header);
        assert!(center_cell > left_cell);
        assert!(right_cell > center_cell);
    }

    #[test]
    fn width_solver_wraps_cells_across_multiple_widths() {
        let input = "| feature | details |\n| --- | --- |\n| wrapping | this sentence should wrap cleanly |\n";
        let wide = render_strings(input, 40);
        let narrow = render_strings(input, 22);

        assert!(wide.len() < narrow.len());
        assert!(narrow.iter().any(|line| line.contains("sentence")));
        assert!(narrow.iter().any(|line| line.contains("cleanly")));
    }

    #[test]
    fn unicode_width_is_accounted_for() {
        let input = "| col | value |\n| --- | --- |\n| 你好 | 宽字符 |\n";
        let rendered = render_strings(input, 20);
        assert!(rendered.iter().any(|line| line.contains("你好")));
        assert!(rendered.iter().any(|line| line.contains("宽字符")));
    }

    #[test]
    fn malformed_markdown_renders_as_text() {
        let rendered = render_strings("| a | b |\n| this is not a separator |\n| 1 | 2 |\n", 60);
        assert!(rendered.iter().any(|line| line.contains("this is not a separator")));
        assert!(!rendered.iter().any(|line| line.contains('─') || line.contains("a:")));
    }

    #[test]
    fn empty_cells_keep_their_columns() {
        let rendered = render_strings(
            "| Hassle | What users report | Refs |\n\
             | --- | --- | --- |\n\
             | Paste freeze | Pasting many lines froze the terminal |  |\n\
             | Scrollback destroyed |  | #42002 |\n",
            100,
        );

        let column = |line: &str, text: &str| {
            line.find(text).map(|offset| display_width(&line[..offset])).expect("cell text")
        };
        let header = rendered.iter().find(|line| line.contains("Refs")).expect("header");
        let paste = rendered.iter().find(|line| line.contains("Paste freeze")).expect("row");
        let scrollback = rendered.iter().find(|line| line.contains("#42002")).expect("row");
        assert!(scrollback.contains("Scrollback destroyed"));
        assert_eq!(column(scrollback, "#42002"), column(header, "Refs"));
        assert_eq!(
            column(paste, "Pasting many lines"),
            column(header, "What users report"),
            "{rendered:?}"
        );
    }

    #[test]
    fn valid_long_rows_after_empty_cells_stay_in_table_grid() {
        let input = concat!(
            "| Hassle | What users report | Refs |\n",
            "| --- | --- | --- |\n",
            "| Input lag | Keystrokes echo late as context fills | #18943, #41501 |\n",
            "| Paste freeze | Pasting many lines wrote a large amount to stdout and froze the terminal | apiyi writeup, #9935 |\n",
            "| Scrollback destroyed | Uses alternate screen and erases native scrollback | #42002, #43643 |\n",
        );
        let rendered = render_strings(input, 180);

        assert!(rendered.iter().any(|line| line.contains("Paste freeze")));
        assert!(rendered.iter().any(|line| line.contains("Scrollback destroyed")));
        assert!(!rendered.iter().any(|line| line.contains("Hassle:")));
        assert!(!rendered.iter().any(|line| line.trim_start().starts_with('|')));
    }

    #[test]
    fn long_url_refs_do_not_force_stacked_layout_when_grid_can_wrap() {
        let input = concat!(
            "| Hassle | What users report | Refs |\n",
            "| --- | --- | --- |\n",
            "| Silent sync failures | Progress freezes at ninety-nine percent for hours on large folders, the bar never finishes, and the only workaround anyone finds is force-quitting the whole app | https://support.example.com/community/forums/sync/threads/1042-progress-stuck-at-99-percent-on-large-folders?sort=top&page=3#latest-reply |\n",
            "| Disappearing files | Files vanish from the sync folder with no warning and reappear later | https://support.example.com/community/forums/data-loss/threads/1043-files-disappear-then-reappear-hours-later?utm_source=digest&ref=panel |\n",
            "| Version conflicts | Conflicting copies pile up after editing on two devices, with cryptic suffixes added to filenames, and no clear way to tell which version is the latest one here | https://github.com/example/desktop-client/issues/887?q=is%3Aissue+conflicted+copy+filename+suffix+multi-device&utm_campaign=triage |\n",
        );
        let rendered = render_strings(input, 120);

        assert!(rendered.iter().any(|line| line.contains("Silent sync failures")));
        assert!(rendered.iter().any(|line| line.contains("progress-stuck-at-99")));
        assert!(!rendered.iter().any(|line| line.contains("Hassle:")));
        assert!(!rendered.iter().any(|line| line.contains("Refs:")));
        assert!(!rendered.iter().any(|line| line.trim_start().starts_with('|')));
    }

    #[test]
    fn url_tokens_have_capped_soft_min_width() {
        let (_preferred, soft_min) = measure_cell_widths(
            "https://support.example.com/community/forums/sync/threads/1042-progress-stuck-at-99-percent-on-large-folders?sort=top&page=3#latest-reply",
        );

        assert_eq!(soft_min, URL_TOKEN_SOFT_MIN_WIDTH);
    }

    #[test]
    fn inline_markdown_spans_render_inside_cells() {
        let input = "| col |\n| --- |\n| **bold** and `code` |\n";
        let rendered = render_lines(input, 24);
        let body = &rendered[2];
        assert!(body.spans.iter().any(|span| span.content.contains("bold")
            && span.style.add_modifier.contains(Modifier::BOLD)));
        assert!(body.spans.iter().any(|span| span.content.contains("code")
            && span.style.fg == Some(Color::White)
            && span.style.bg == Some(Color::Black)));
    }

    #[test]
    fn narrow_tables_fall_back_to_stacked_layout() {
        let input = "| Name | Description |\n| --- | --- |\n| foo | long wrapped value |\n";
        let rendered = render_strings(input, 12);
        assert!(rendered.iter().any(|line| line.contains("Name:")));
        assert!(rendered.iter().any(|line| line.contains("foo")));
        assert!(rendered.iter().any(|line| line.contains("Description")));
        assert!(!rendered.iter().any(|line| line.contains('─')));
    }
}
