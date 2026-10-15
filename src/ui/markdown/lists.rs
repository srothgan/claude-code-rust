// SPDX-License-Identifier: Apache-2.0

//! List layout retains parser ownership through paragraphs, nesting and wrapping.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::{DefaultStyleSheet, StyleSheet};

use super::super::wrap::{
    StyledChunk, display_width, wrap_lines_to_physical_rows, wrap_styled_chunks_with_hanging_prefix,
};

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

    fn contains_list(&self) -> bool {
        match self {
            Self::Block(Tag::List(_), _, _) => true,
            Self::Block(_, _, children) => children.iter().any(Self::contains_list),
            Self::Inline(_, _) => false,
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

pub(super) fn render(source: &str, width: u16, bg: Option<Color>) -> Vec<Line<'static>> {
    let (nodes, _) =
        nodes(&mut Parser::new_ext(source, super::parser_options()).into_offset_iter());
    let mut out = Vec::new();
    render_document(&nodes, source, width, bg, "", &mut out);
    out
}

fn render_document(
    nodes: &[Node<'_>],
    source: &str,
    width: u16,
    bg: Option<Color>,
    prefix: &str,
    out: &mut Vec<Line<'static>>,
) {
    let mut plain_start = None;
    let mut plain_end = 0;
    let mut section_start = 0;
    for node in nodes {
        if !prefix.is_empty() && !node.contains_list() {
            if let Node::Block(Tag::CodeBlock(kind), _, content) = node {
                out.extend(prefix_block_lines(
                    code_lines(kind, content),
                    prefix,
                    prefix,
                    width,
                    bg,
                ));
            } else if let Node::Block(Tag::BlockQuote(_), _, children) = node {
                render_document(children, source, width, bg, &format!("{prefix}> "), out);
            } else {
                let mut spans = Vec::new();
                inline_spans(node, DefaultStyleSheet.blockquote(), &mut spans);
                out.extend(prefix_lines(vec![Line::from(spans)], prefix, prefix, width, bg));
            }
            continue;
        }
        if !node.contains_list() {
            if plain_start.is_none() {
                preserve_separator(source, node.range().start, bg, out);
            }
            plain_start.get_or_insert(section_start);
            plain_end = node.range().end;
            continue;
        }
        if let Some(start) = plain_start.take() {
            out.extend(super::render_with_tui_markdown(&source[start..plain_end], bg));
        }
        preserve_separator(source, node.range().start, bg, out);
        match node {
            Node::Block(Tag::List(start), _, items) => {
                render_list(items, *start, source, width, bg, &format!("{prefix}  "), out);
            }
            Node::Block(Tag::BlockQuote(_), _, children) => {
                render_document(children, source, width, bg, &format!("{prefix}> "), out);
            }
            Node::Block(_, _, children) => {
                render_document(children, source, width, bg, prefix, out);
            }
            Node::Inline(_, _) => {}
        }
        section_start = node.range().end;
    }
    if let Some(start) = plain_start {
        out.extend(super::render_with_tui_markdown(&source[start..plain_end], bg));
    }
}

fn preserve_separator(source: &str, start: usize, bg: Option<Color>, out: &mut Vec<Line<'static>>) {
    let blank_line_before =
        source[..start].rsplit('\n').take_while(|line| line.trim().is_empty()).take(2).count() == 2;
    if blank_line_before
        && out
            .last()
            .is_some_and(|line| line.spans.iter().any(|span| !span.content.trim().is_empty()))
    {
        out.push(
            Line::default().style(bg.map_or(Style::default(), |color| Style::default().bg(color))),
        );
    }
}

fn render_list(
    items: &[Node<'_>],
    mut number: Option<u64>,
    source: &str,
    width: u16,
    bg: Option<Color>,
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
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, width, bg, out);
                    if first {
                        out.push(Line::from(prefix.trim_end().to_owned()));
                        first = false;
                    }
                    render_list(nested, *start, source, width, bg, &body_prefix, out);
                }
                Node::Block(Tag::Paragraph, _, paragraph) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, width, bg, out);
                    if !first {
                        out.push(Line::default());
                    }
                    inline.extend(paragraph);
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, width, bg, out);
                }
                Node::Block(Tag::CodeBlock(kind), _, content) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, width, bg, out);
                    if !first {
                        out.push(Line::default());
                    }
                    let lines = code_lines(kind, content);
                    out.extend(prefix_block_lines(
                        lines,
                        if first { &prefix } else { &body_prefix },
                        &body_prefix,
                        width,
                        bg,
                    ));
                    first = false;
                }
                Node::Block(Tag::Table(_), range, _) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, width, bg, out);
                    let table = source[range.clone()]
                        .lines()
                        .map(str::trim_start)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let inner = width.saturating_sub(
                        u16::try_from(display_width(&body_prefix)).unwrap_or(u16::MAX),
                    );
                    let lines = super::super::document_table::render_markdown_with_tables(
                        &table, inner, bg,
                    );
                    out.extend(prefix_block_lines(
                        lines,
                        if first { &prefix } else { &body_prefix },
                        &body_prefix,
                        width,
                        bg,
                    ));
                    first = false;
                }
                Node::Block(Tag::BlockQuote(_), _, quoted) => {
                    flush_inline(&mut inline, &prefix, &body_prefix, &mut first, width, bg, out);
                    if first {
                        out.push(Line::from(prefix.trim_end().to_owned()));
                        first = false;
                    }
                    render_document(quoted, source, width, bg, &format!("{body_prefix}> "), out);
                }
                node => inline.push(node),
            }
        }
        flush_inline(&mut inline, &prefix, &body_prefix, &mut first, width, bg, out);
        if first {
            out.push(Line::from(prefix.trim_end().to_owned()));
        }
        if ordered
            && let Some(span) = out.get_mut(marker_row).and_then(|line| line.spans.first_mut())
        {
            span.style = span.style.patch(DefaultStyleSheet.list_marker());
        }
    }
}

fn code_lines(kind: &CodeBlockKind<'_>, content: &[Node<'_>]) -> Vec<Line<'static>> {
    let text = content
        .iter()
        .filter_map(|node| match node {
            Node::Inline(Event::Text(text), _) => Some(text.as_ref()),
            _ => None,
        })
        .collect::<String>();
    let language = match kind {
        CodeBlockKind::Fenced(info) => info.as_ref(),
        CodeBlockKind::Indented => "",
    };
    let mut lines = vec![Line::from(format!("```{language}"))];
    lines.extend(super::super::highlight::highlight_code(
        &text,
        (!language.is_empty()).then_some(language),
    ));
    lines.push(Line::from("```"));
    lines
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
    width: u16,
    bg: Option<Color>,
    out: &mut Vec<Line<'static>>,
) {
    if nodes.is_empty() {
        return;
    }
    let mut spans = Vec::new();
    for node in nodes.drain(..) {
        inline_spans(node, Style::default(), &mut spans);
    }
    if spans.is_empty() {
        return;
    }
    out.extend(prefix_lines(
        vec![Line::from(spans)],
        if *first { prefix } else { continuation },
        continuation,
        width,
        bg,
    ));
    *first = false;
}

fn inline_spans(node: &Node<'_>, style: Style, spans: &mut Vec<Span<'static>>) {
    let styles = DefaultStyleSheet;
    match node {
        Node::Block(tag, _, children) => {
            let style = match tag {
                Tag::Strong | Tag::Heading { .. } => style.add_modifier(Modifier::BOLD),
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
                inline_spans(child, style, spans);
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
    wrap_lines_to_physical_rows(&lines, width - prefix_width)
        .into_iter()
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
