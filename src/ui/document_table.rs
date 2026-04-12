// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::markdown;
use super::wrap::{
    StyledChunk, blank_line, display_width, line_display_width, pad_line_to_width,
    wrap_styled_chunks,
};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
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
struct DocumentTable {
    header: TableRowAst,
    rows: Vec<TableRowAst>,
    alignments: Vec<ColumnAlignment>,
}

enum MarkdownBlock {
    Text(String),
    Table(DocumentTable),
}

impl DocumentTable {
    fn column_count(&self) -> usize {
        let body_cols = self.rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
        self.header.cells.len().max(self.alignments.len()).max(body_cols)
    }

    fn render_lines(&self, width: u16, bg: Option<Color>) -> Vec<Line<'static>> {
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
                render_grid_lines(self, &layout, bg)
            }
            TableLayoutMode::Stacked => render_stacked_lines(self, usize::from(width), bg),
        }
    }
}

impl TableCellAst {
    fn empty() -> Self {
        Self::default()
    }
}

pub fn render_markdown_with_tables(
    text: &str,
    width: u16,
    bg: Option<Color>,
) -> Vec<Line<'static>> {
    let blocks = split_markdown_tables(text);
    let mut out = Vec::new();
    for block in blocks {
        match block {
            MarkdownBlock::Text(chunk) => {
                if chunk.trim().is_empty() {
                    continue;
                }
                out.extend(markdown::render_markdown_safe(&chunk, bg));
            }
            MarkdownBlock::Table(table) => {
                if !out.is_empty() {
                    out.push(Line::default());
                }
                out.extend(table.render_lines(width, bg));
                out.push(Line::default());
            }
        }
    }
    out
}

fn parser_options() -> Options {
    let mut options = Options::ENABLE_STRIKETHROUGH;
    options.insert(Options::ENABLE_TABLES);
    options
}

fn split_markdown_tables(text: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut parser = Parser::new_ext(text, parser_options()).into_offset_iter().peekable();
    let mut text_start = 0usize;

    loop {
        let Some((event, range)) = parser.next() else {
            break;
        };
        if let Event::Start(Tag::Table(alignments)) = event {
            if text_start < range.start {
                blocks.push(MarkdownBlock::Text(text[text_start..range.start].to_owned()));
            }

            let mut table_end = range.end;
            let table = parse_table_ast(alignments, &mut parser, &mut table_end);
            blocks.push(MarkdownBlock::Table(table));
            text_start = table_end;
        }
    }

    if text_start < text.len() {
        blocks.push(MarkdownBlock::Text(text[text_start..].to_owned()));
    }

    blocks
}

fn parse_table_ast<'input, I>(
    alignments: Vec<Alignment>,
    parser: &mut std::iter::Peekable<I>,
    table_end: &mut usize,
) -> DocumentTable
where
    I: Iterator<Item = (Event<'input>, std::ops::Range<usize>)>,
{
    let mut header = None;
    let mut rows = Vec::new();
    let mut current_row: Option<TableRowAst> = None;
    let mut current_cell: Option<CellBuilder> = None;
    let mut in_header = false;

    for (event, range) in parser.by_ref() {
        *table_end = (*table_end).max(range.end);
        match event {
            Event::Start(Tag::TableHead) => in_header = true,
            Event::End(TagEnd::TableHead) => {
                if let Some(row) = current_row.take()
                    && !row.cells.is_empty()
                    && header.is_none()
                {
                    header = Some(row);
                }
                in_header = false;
            }
            Event::Start(Tag::TableRow) => current_row = Some(TableRowAst::default()),
            Event::End(TagEnd::TableRow) => {
                let row = current_row.take().unwrap_or_default();
                if in_header && header.is_none() {
                    header = Some(row);
                } else {
                    rows.push(row);
                }
            }
            Event::Start(Tag::TableCell) => {
                current_row.get_or_insert_with(TableRowAst::default);
                current_cell = Some(CellBuilder::new());
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(cell) = current_cell.take() {
                    current_row.get_or_insert_with(TableRowAst::default).cells.push(cell.finish());
                }
            }
            Event::End(TagEnd::Table) => break,
            Event::Start(tag) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.start_tag(&tag);
                }
            }
            Event::End(tag) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.end_tag(tag);
                }
            }
            Event::Text(text) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_text(text.as_ref());
                }
            }
            Event::Code(code) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_code(code.as_ref());
                }
            }
            Event::SoftBreak => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_text(" ");
                }
            }
            Event::HardBreak => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_text("\n");
                }
            }
            Event::Html(raw) | Event::InlineHtml(raw) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_text(raw.as_ref());
                }
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_text(math.as_ref());
                }
            }
            Event::FootnoteReference(reference) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_text(reference.as_ref());
                }
            }
            Event::TaskListMarker(done) => {
                if let Some(cell) = current_cell.as_mut() {
                    cell.push_text(if done { "[x] " } else { "[ ] " });
                }
            }
            Event::Rule => {}
        }
    }

    DocumentTable {
        header: header.unwrap_or_default(),
        rows,
        alignments: alignments.into_iter().map(ColumnAlignment::from).collect(),
    }
}

struct CellBuilder {
    chunks: Vec<StyledChunk>,
    current_text: String,
    style_stack: Vec<Style>,
    current_style: Style,
}

impl CellBuilder {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            current_text: String::new(),
            style_stack: vec![Style::default()],
            current_style: Style::default(),
        }
    }

    fn push_text(&mut self, text: &str) {
        self.current_text.push_str(text);
    }

    fn push_code(&mut self, text: &str) {
        self.flush_current();
        let style = self.current_style.add_modifier(Modifier::REVERSED);
        self.chunks.push(StyledChunk { text: text.to_owned(), style });
    }

    fn start_tag(&mut self, tag: &Tag<'_>) {
        let next = match tag {
            Tag::Strong => self.current_style.add_modifier(Modifier::BOLD),
            Tag::Emphasis => self.current_style.add_modifier(Modifier::ITALIC),
            Tag::Strikethrough => self.current_style.add_modifier(Modifier::CROSSED_OUT),
            Tag::Link { .. } => self.current_style.add_modifier(Modifier::UNDERLINED),
            _ => return,
        };

        self.flush_current();
        self.style_stack.push(next);
        self.current_style = next;
    }

    fn end_tag(&mut self, tag: TagEnd) {
        let styled =
            matches!(tag, TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link);
        if !styled {
            return;
        }

        self.flush_current();
        let _ = self.style_stack.pop();
        self.current_style = self.style_stack.last().copied().unwrap_or_default();
    }

    fn flush_current(&mut self) {
        if self.current_text.is_empty() {
            return;
        }
        self.chunks.push(StyledChunk {
            text: std::mem::take(&mut self.current_text),
            style: self.current_style,
        });
    }

    fn finish(mut self) -> TableCellAst {
        self.flush_current();
        let plain_text: String = self.chunks.iter().map(|chunk| chunk.text.as_str()).collect();
        let (preferred_width, soft_min_width) = measure_cell_widths(&plain_text);
        TableCellAst {
            chunks: self.chunks,
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
        .map(display_width)
        .max()
        .unwrap_or(preferred);
    (preferred, soft_min)
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
) -> Vec<Line<'static>> {
    let header_style = bg.map_or_else(
        || Style::default().add_modifier(Modifier::BOLD),
        |bg_color| Style::default().bg(bg_color).add_modifier(Modifier::BOLD),
    );
    let row_style = bg.map_or_else(Style::default, |bg_color| Style::default().bg(bg_color));
    let indent = 2usize;
    let mut lines = Vec::new();

    for (row_idx, row) in table.rows.iter().enumerate() {
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

    fn render_strings(text: &str, width: u16) -> Vec<String> {
        render_markdown_with_tables(text, width, None)
            .into_iter()
            .map(|line| line.spans.into_iter().map(|span| span.content.into_owned()).collect())
            .collect()
    }

    #[test]
    fn structural_parser_extracts_markdown_tables() {
        let blocks =
            split_markdown_tables("before\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nafter");
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], MarkdownBlock::Text(_)));
        assert!(matches!(blocks[1], MarkdownBlock::Table(_)));
        assert!(matches!(blocks[2], MarkdownBlock::Text(_)));
    }

    #[test]
    fn structural_parser_does_not_treat_code_fences_as_tables() {
        let input = "```text\n| not | a table |\n| --- | --- |\n```\n";
        let blocks = split_markdown_tables(input);
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], MarkdownBlock::Text(_)));
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
    fn malformed_markdown_falls_back_to_text_block() {
        let blocks = split_markdown_tables("| a | b |\n| this is not a separator |\n| 1 | 2 |\n");
        assert_eq!(blocks.len(), 1);
        assert!(matches!(blocks[0], MarkdownBlock::Text(_)));
    }

    #[test]
    fn inline_markdown_spans_render_inside_cells() {
        let input = "| col |\n| --- |\n| **bold** and `code` |\n";
        let rendered = render_markdown_with_tables(input, 24, None);
        let body = &rendered[2];
        assert!(body.spans.iter().any(|span| span.style.add_modifier.contains(Modifier::BOLD)));
        assert!(body.spans.iter().any(|span| span.style.add_modifier.contains(Modifier::REVERSED)));
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
