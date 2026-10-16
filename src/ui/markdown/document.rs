// SPDX-License-Identifier: Apache-2.0

//! Document layout from one parse. Lists, code blocks and tables retain parser ownership through
//! nesting and wrapping; the remaining blocks render through `tui-markdown`.

use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Tag};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::StyleSheet as _;

use super::super::highlight;
use super::super::wrap::{
    StyledChunk, display_width, split_line_at_width, wrap_styled_chunks_with_hanging_prefix,
};
use super::MarkdownStyle;
use super::table::DocumentTable;

#[derive(Clone, Copy)]
struct Layout<'s> {
    source: &'s str,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
}

enum Node<'a> {
    Block(Tag<'a>, Range<usize>, Vec<Self>),
    Inline(Event<'a>, Range<usize>),
}

impl Node<'_> {
    fn range(&self) -> &Range<usize> {
        match self {
            Self::Block(_, range, _) | Self::Inline(_, range) => range,
        }
    }

    /// Lists, code blocks and tables are laid out here instead of by `tui-markdown`.
    fn needs_layout(&self) -> bool {
        match self {
            Self::Block(Tag::List(_) | Tag::CodeBlock(_) | Tag::Table(_), _, _) => true,
            Self::Block(_, _, children) => children.iter().any(Self::needs_layout),
            Self::Inline(_, _) => false,
        }
    }

    fn soft_breaks(&self, out: &mut Vec<Range<usize>>) {
        match self {
            Self::Block(_, _, children) => {
                for child in children {
                    child.soft_breaks(out);
                }
            }
            Self::Inline(Event::SoftBreak, range) => out.push(range.clone()),
            Self::Inline(_, _) => {}
        }
    }
}

fn nodes<'a>(
    events: &mut impl Iterator<Item = (Event<'a>, Range<usize>)>,
) -> (Vec<Node<'a>>, usize) {
    let mut out = Vec::new();
    let mut end = 0;
    while let Some((event, range)) = events.next() {
        end = range.end;
        match event {
            Event::Start(tag) => {
                let (children, block_end) = nodes(events);
                out.push(Node::Block(tag, range.start..block_end, children));
            }
            Event::End(_) => break,
            _ => out.push(Node::Inline(event, range)),
        }
    }
    (out, end)
}

pub(super) fn render(
    source: &str,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
) -> Vec<Line<'static>> {
    let (nodes, _) = nodes(&mut crate::markdown::parser(source).into_offset_iter());
    let mut out = Vec::new();
    render_document(&nodes, Layout { source, width, bg, preserve_newlines }, "", &mut out);
    out
}

fn render_document(
    nodes: &[Node<'_>],
    layout: Layout<'_>,
    prefix: &str,
    out: &mut Vec<Line<'static>>,
) {
    let mut plain = Vec::new();
    let mut plain_start = 0;
    let mut section_start = 0;
    let mut separate_next = false;
    for node in nodes {
        if !node.needs_layout() {
            if std::mem::take(&mut separate_next) {
                push_gap(layout.bg, out);
            }
            if !prefix.is_empty() {
                if let Node::Block(Tag::BlockQuote(_), _, children) = node {
                    render_document(children, layout, &format!("{prefix}> "), out);
                } else {
                    let mut spans = Vec::new();
                    inline_spans(node, MarkdownStyle.blockquote(), layout, &mut spans);
                    out.extend(prefix_lines(
                        vec![Line::from(spans)],
                        prefix,
                        prefix,
                        layout.width,
                        layout.bg,
                    ));
                }
                continue;
            }
            if plain.is_empty() {
                preserve_separator(layout.source, node.range().start, layout.bg, out);
                plain_start = section_start;
            }
            plain.push(node);
            continue;
        }
        render_plain(&std::mem::take(&mut plain), plain_start, layout, out);
        if std::mem::take(&mut separate_next)
            || matches!(node, Node::Block(Tag::CodeBlock(_) | Tag::Table(_), _, _))
        {
            push_gap(layout.bg, out);
        }
        preserve_separator(layout.source, node.range().start, layout.bg, out);
        match node {
            Node::Block(Tag::List(start), _, items) => {
                render_list(items, *start, layout, &format!("{prefix}  "), out);
            }
            Node::Block(Tag::CodeBlock(kind), _, content) => {
                out.extend(code_block_lines(kind, content, prefix, prefix, layout));
                separate_next = true;
            }
            Node::Block(Tag::Table(alignments), _, sections) => {
                let lines =
                    table_lines(alignments, sections, inner_width(prefix, layout.width), layout);
                out.extend(prefix_block_lines(lines, prefix, prefix, layout.width, layout.bg));
                out.push(Line::default());
            }
            Node::Block(Tag::BlockQuote(_), _, children) => {
                render_document(children, layout, &format!("{prefix}> "), out);
            }
            Node::Block(_, _, children) => render_document(children, layout, prefix, out),
            Node::Inline(_, _) => {}
        }
        section_start = node.range().end;
    }
    render_plain(&plain, plain_start, layout, out);
}

/// Consecutive blocks without layout render as one source slice, so reference definitions and
/// block spacing between them stay intact.
fn render_plain(run: &[&Node<'_>], start: usize, layout: Layout<'_>, out: &mut Vec<Line<'static>>) {
    let Some(end) = run.last().map(|node| node.range().end) else {
        return;
    };
    let source = &layout.source[start..end];
    let text = if layout.preserve_newlines {
        let mut breaks = Vec::new();
        for node in run {
            node.soft_breaks(&mut breaks);
        }
        with_hard_breaks(source, start, &breaks)
    } else {
        Cow::Borrowed(source)
    };
    out.extend(super::render_with_tui_markdown(&text, layout.bg));
}

/// Two trailing spaces before each parsed soft break turn it into a hard break.
fn with_hard_breaks<'s>(source: &'s str, start: usize, breaks: &[Range<usize>]) -> Cow<'s, str> {
    if breaks.is_empty() {
        return Cow::Borrowed(source);
    }
    let mut text = String::with_capacity(source.len() + breaks.len() * 2);
    let mut copied = 0;
    for range in breaks {
        let local = range.start - start..range.end - start;
        let Some(newline) = source[local.clone()].find(['\r', '\n']) else {
            continue;
        };
        let at = local.start + newline;
        text.push_str(&source[copied..at]);
        text.push_str("  ");
        copied = at;
    }
    text.push_str(&source[copied..]);
    Cow::Owned(text)
}

/// Separates blocks with one blank row, never stacking blank rows.
fn push_gap(bg: Option<Color>, out: &mut Vec<Line<'static>>) {
    if out.last().is_some_and(|line| line.spans.iter().any(|span| !span.content.trim().is_empty()))
    {
        out.push(
            Line::default().style(bg.map_or(Style::default(), |color| Style::default().bg(color))),
        );
    }
}

fn preserve_separator(source: &str, start: usize, bg: Option<Color>, out: &mut Vec<Line<'static>>) {
    let blank_line_before =
        source[..start].rsplit('\n').take_while(|line| line.trim().is_empty()).take(2).count() == 2;
    if blank_line_before {
        push_gap(bg, out);
    }
}

fn inner_width(prefix: &str, width: u16) -> u16 {
    width.saturating_sub(u16::try_from(display_width(prefix)).unwrap_or(u16::MAX))
}

fn render_list(
    items: &[Node<'_>],
    mut number: Option<u64>,
    layout: Layout<'_>,
    indent: &str,
    out: &mut Vec<Line<'static>>,
) {
    for node in items {
        let Node::Block(Tag::Item, _, children) = node else {
            continue;
        };
        let marker = number.map_or_else(|| "- ".to_owned(), |value| format!("{value}. "));
        let ordered = number.is_some();
        let marker_row = out.len();
        number = number.map(|value| value.saturating_add(1));
        let mut prefix = format!("{indent}{marker}");
        if let Some(checked) = first_task_marker(children) {
            prefix.push_str(if checked { "[x] " } else { "[ ] " });
        }
        let body_prefix =
            format!("{indent}{}", " ".repeat(display_width(&prefix) - display_width(indent)));
        let mut first = true;
        let mut inline = Vec::new();
        for child in children {
            match child {
                Node::Block(Tag::List(start), _, nested) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, layout, out);
                    if first {
                        out.push(Line::from(prefix.trim_end().to_owned()));
                        first = false;
                    }
                    render_list(nested, *start, layout, &body_prefix, out);
                }
                Node::Block(Tag::Paragraph, _, paragraph) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, layout, out);
                    if !first {
                        out.push(Line::default());
                    }
                    inline.extend(paragraph);
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, layout, out);
                }
                Node::Block(Tag::CodeBlock(kind), _, content) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, layout, out);
                    if !first {
                        out.push(Line::default());
                    }
                    let first_prefix = if first { &prefix } else { &body_prefix };
                    out.extend(code_block_lines(kind, content, first_prefix, &body_prefix, layout));
                    first = false;
                }
                Node::Block(Tag::Table(alignments), _, sections) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, layout, out);
                    let lines = table_lines(
                        alignments,
                        sections,
                        inner_width(&body_prefix, layout.width),
                        layout,
                    );
                    out.extend(prefix_block_lines(
                        lines,
                        if first { &prefix } else { &body_prefix },
                        &body_prefix,
                        layout.width,
                        layout.bg,
                    ));
                    first = false;
                }
                Node::Block(Tag::BlockQuote(_), _, quoted) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, layout, out);
                    if first {
                        out.push(Line::from(prefix.trim_end().to_owned()));
                        first = false;
                    }
                    render_document(quoted, layout, &format!("{body_prefix}> "), out);
                }
                node => inline.push(node),
            }
        }
        flush_inline(&mut inline, &prefix, &body_prefix, &mut first, layout, out);
        if first {
            out.push(Line::from(prefix.trim_end().to_owned()));
        }
        if ordered
            && let Some(span) = out.get_mut(marker_row).and_then(|line| line.spans.first_mut())
        {
            span.style = span.style.patch(MarkdownStyle.list_marker());
        }
    }
}

/// A labelled, highlighted block whose source lines wrap inside the container's column.
fn code_block_lines(
    kind: &CodeBlockKind<'_>,
    content: &[Node<'_>],
    first: &str,
    continuation: &str,
    layout: Layout<'_>,
) -> Vec<Line<'static>> {
    let code = content
        .iter()
        .filter_map(|node| match node {
            Node::Inline(Event::Text(text), _) => Some(text.as_ref()),
            _ => None,
        })
        .collect::<String>();
    let language = match kind {
        CodeBlockKind::Fenced(info) => info.split([' ', '\t', ',']).next().unwrap_or_default(),
        CodeBlockKind::Indented => "",
    };
    let label = if language.is_empty() { "code" } else { language };
    let mut lines = vec![Line::from(Span::styled(label.to_owned(), MarkdownStyle::code_label()))];
    lines.extend(highlight::highlight_code(
        code.strip_suffix('\n').unwrap_or(&code),
        (!language.is_empty()).then_some(language),
    ));
    prefix_block_lines(lines, first, continuation, layout.width, layout.bg)
}

fn table_lines(
    alignments: &[Alignment],
    sections: &[Node<'_>],
    width: u16,
    layout: Layout<'_>,
) -> Vec<Line<'static>> {
    let cells = |row: &[Node<'_>]| {
        row.iter()
            .filter_map(|cell| {
                let Node::Block(Tag::TableCell, _, children) = cell else {
                    return None;
                };
                let mut spans = Vec::new();
                for child in children {
                    inline_spans(child, Style::default(), layout, &mut spans);
                }
                Some(
                    spans
                        .into_iter()
                        .map(|span| StyledChunk {
                            text: span.content.into_owned(),
                            style: span.style,
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut header = Vec::new();
    let mut rows = Vec::new();
    for section in sections {
        match section {
            Node::Block(Tag::TableHead, _, head) => {
                header = match head.first() {
                    Some(Node::Block(Tag::TableRow, _, row)) => cells(row),
                    _ => cells(head),
                };
            }
            Node::Block(Tag::TableRow, _, row) => rows.push(cells(row)),
            _ => {}
        }
    }
    DocumentTable::new(alignments, header, rows).render_lines(width, layout.bg)
}

fn first_task_marker(nodes: &[Node<'_>]) -> Option<bool> {
    match nodes.first()? {
        Node::Inline(Event::TaskListMarker(checked), _) => Some(*checked),
        Node::Block(Tag::Paragraph, _, children) => first_task_marker(children),
        _ => None,
    }
}

fn flush_inline(
    nodes: &mut Vec<&Node<'_>>,
    prefix: &str,
    continuation: &str,
    first: &mut bool,
    layout: Layout<'_>,
    out: &mut Vec<Line<'static>>,
) {
    if nodes.is_empty() {
        return;
    }
    let mut spans = Vec::new();
    for node in nodes.drain(..) {
        inline_spans(node, Style::default(), layout, &mut spans);
    }
    if spans.is_empty() {
        return;
    }
    out.extend(prefix_lines(
        vec![Line::from(spans)],
        if *first { prefix } else { continuation },
        continuation,
        layout.width,
        layout.bg,
    ));
    *first = false;
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn inline_spans(node: &Node<'_>, style: Style, layout: Layout<'_>, spans: &mut Vec<Span<'static>>) {
    let styles = MarkdownStyle;
    match node {
        Node::Block(tag, _, children) => {
            let style = match tag {
                Tag::Heading { level, .. } => style.patch(styles.heading(heading_level(*level))),
                Tag::Strong => style.add_modifier(Modifier::BOLD),
                Tag::Emphasis => style.add_modifier(Modifier::ITALIC),
                Tag::Subscript | Tag::Superscript => {
                    style.add_modifier(Modifier::DIM | Modifier::ITALIC)
                }
                Tag::Strikethrough => style.add_modifier(Modifier::CROSSED_OUT),
                Tag::Link { .. } => style.patch(styles.link()),
                Tag::Image { .. } => style.patch(styles.image_alt()),
                _ => style,
            };
            if matches!(tag, Tag::Image { .. }) {
                spans.push(Span::styled("[img] ", style));
            }
            for child in children {
                inline_spans(child, style, layout, spans);
            }
            if let Tag::Link { dest_url, .. } = tag {
                spans.push(Span::styled(format!(" ({dest_url})"), style));
            }
            if let Tag::Image { dest_url, .. } = tag
                && children.is_empty()
            {
                spans.push(Span::styled(dest_url.to_string(), style));
            }
        }
        Node::Inline(event, _) => {
            let (text, style) = match event {
                Event::Text(text) => (text.to_string(), style),
                Event::Code(text) => (text.to_string(), style.patch(styles.code())),
                Event::SoftBreak if layout.preserve_newlines => ("\n".to_owned(), style),
                Event::SoftBreak => (" ".to_owned(), style),
                Event::HardBreak => ("\n".to_owned(), style),
                Event::Html(text) | Event::InlineHtml(text) => {
                    (text.to_string(), style.patch(styles.html()))
                }
                Event::InlineMath(text) => (format!("${text}$"), style.patch(styles.math_inline())),
                Event::DisplayMath(text) => {
                    (format!("$${text}$$"), style.patch(styles.math_display()))
                }
                Event::FootnoteReference(text) => {
                    (format!("[{text}]"), style.patch(styles.footnote_ref()))
                }
                Event::Rule => ("---".to_owned(), style),
                _ => return,
            };
            spans.push(Span::styled(text, style));
        }
    }
}

fn prefix_lines(
    lines: Vec<Line<'static>>,
    first: &str,
    continuation: &str,
    width: u16,
    bg: Option<Color>,
) -> Vec<Line<'static>> {
    let style = bg.map_or(Style::default(), |color| Style::default().bg(color));
    let mut out = Vec::new();
    for (index, line) in lines.into_iter().enumerate() {
        let prefix = if index == 0 { first } else { continuation };
        let chunks = line
            .spans
            .into_iter()
            .map(|span| StyledChunk {
                text: span.content.into_owned(),
                style: line.style.patch(span.style).patch(style),
            })
            .collect::<Vec<_>>();
        let mut rows = wrap_styled_chunks_with_hanging_prefix(
            &[StyledChunk { text: prefix.to_owned(), style }],
            &chunks,
            usize::from(width),
            style,
        );
        if display_width(prefix) < usize::from(width) {
            for row in rows.iter_mut().skip(1) {
                if let Some(span) = row.spans.first_mut() {
                    span.content = continuation.to_owned().into();
                }
            }
        }
        out.extend(rows);
    }
    out
}

/// Code and pre-laid-out tables retain their own leading whitespace.
fn prefix_block_lines(
    lines: Vec<Line<'static>>,
    first: &str,
    continuation: &str,
    width: u16,
    bg: Option<Color>,
) -> Vec<Line<'static>> {
    let prefix_width = u16::try_from(display_width(first)).unwrap_or(u16::MAX);
    if prefix_width >= width {
        return prefix_lines(lines, first, continuation, width, bg);
    }
    let style = bg.map_or(Style::default(), |color| Style::default().bg(color));
    lines
        .into_iter()
        .flat_map(|line| split_line_at_width(line, usize::from(width - prefix_width)))
        .enumerate()
        .map(|(index, mut line)| {
            line.spans.insert(
                0,
                Span::styled(
                    if index == 0 { first.to_owned() } else { continuation.to_owned() },
                    style,
                ),
            );
            if let Some(color) = bg {
                line.style = line.style.bg(color);
                for span in &mut line.spans {
                    span.style = span.style.bg(color);
                }
            }
            line
        })
        .collect()
}
