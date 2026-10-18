// SPDX-License-Identifier: Apache-2.0

//! Document layout from one parse. Code blocks use app layout; lists, tables and their
//! enclosing containers render through `tui-markdown`.

use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Tag};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::StyleSheet as _;

use super::super::highlight;
use super::super::wrap::{
    StyledChunk, display_width, split_line_at_width, wrap_styled_chunks_with_hanging_prefix,
};
use super::MarkdownStyle;
use crate::ui::layout_control::LayoutControl;

#[derive(Clone, Copy)]
struct Layout<'s> {
    source: &'s str,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    control: &'s LayoutControl<'s>,
}

enum Node<'a> {
    Block(Tag<'a>, Range<usize>, Vec<Self>),
    Inline(Event<'a>, Range<usize>),
}

impl Node<'_> {
    fn into_owned(self) -> Node<'static> {
        match self {
            Self::Block(tag, range, children) => Node::Block(
                tag.into_static(),
                range,
                children.into_iter().map(Node::into_owned).collect(),
            ),
            Self::Inline(event, range) => Node::Inline(event.into_static(), range),
        }
    }

    fn range(&self) -> &Range<usize> {
        match self {
            Self::Block(_, range, _) | Self::Inline(_, range) => range,
        }
    }

    fn contains_list_or_table(&self) -> bool {
        match self {
            Self::Block(Tag::List(_) | Tag::Table(_), _, _) => true,
            Self::Block(_, _, children) => children.iter().any(Self::contains_list_or_table),
            Self::Inline(_, _) => false,
        }
    }

    /// Keep lists, tables and their enclosing containers together in the crate renderer.
    fn needs_layout(&self) -> bool {
        if self.contains_list_or_table() {
            return false;
        }
        match self {
            Self::Block(Tag::CodeBlock(_), _, _) => true,
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
    control: &LayoutControl<'_>,
) -> (Vec<Node<'a>>, usize) {
    let mut out = Vec::new();
    let mut end = 0;
    while let Some((event, range)) = events.next() {
        if control.cancelled() {
            return (Vec::new(), end);
        }
        end = range.end;
        match event {
            Event::Start(tag) => {
                let (children, block_end) = nodes(events, control);
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
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    if control.cancelled() {
        return Vec::new();
    }
    let (nodes, _) = nodes(&mut crate::markdown::parser(source).into_offset_iter(), control);
    let mut out = Vec::new();
    render_document(&nodes, Layout { source, width, bg, preserve_newlines, control }, "", &mut out);
    out
}

/// Active document state. All callers drain or resume this same layout path.
#[derive(Default)]
struct DocumentState {
    next_node: usize,
    section_start: usize,
    separate_next: bool,
    previous_nonblank: bool,
    code: Option<CodeRows>,
}

pub(crate) struct DocumentCursor {
    nodes: Vec<Node<'static>>,
    state: DocumentState,
}

impl DocumentCursor {
    pub(crate) fn new(source: &str, control: &LayoutControl<'_>) -> Self {
        let (parsed, _) = nodes(&mut crate::markdown::parser(source).into_offset_iter(), control);
        Self {
            nodes: parsed.into_iter().map(Node::into_owned).collect(),
            state: DocumentState::default(),
        }
    }

    pub(crate) fn next(
        &mut self,
        source: &str,
        width: u16,
        bg: Option<Color>,
        preserve_newlines: bool,
        control: &LayoutControl<'_>,
    ) -> Option<Vec<Line<'static>>> {
        self.state.next(&self.nodes, Layout { source, width, bg, preserve_newlines, control }, "")
    }
}

impl DocumentState {
    fn next(
        &mut self,
        nodes: &[Node<'_>],
        layout: Layout<'_>,
        prefix: &str,
    ) -> Option<Vec<Line<'static>>> {
        if layout.control.cancelled() {
            return None;
        }
        let mut out = Vec::new();
        if let Some(code) = &mut self.code {
            if let Some(lines) = code.next(prefix, layout) {
                self.remember(&lines);
                return Some(lines);
            }
            self.code = None;
        }
        let node = nodes.get(self.next_node)?;
        if node.needs_layout() {
            if std::mem::take(&mut self.separate_next)
                || matches!(node, Node::Block(Tag::CodeBlock(_), _, _))
            {
                push_gap(layout.bg, &mut out, self.previous_nonblank);
            }
            preserve_separator(
                layout.source,
                node.range().start,
                layout.bg,
                &mut out,
                self.previous_nonblank,
            );
            match node {
                Node::Block(Tag::CodeBlock(kind), _, content) => {
                    let (label, code) = CodeRows::new(kind, content);
                    out.extend(prefix_block_lines(vec![label], prefix, layout.width, layout.bg));
                    self.code = Some(code);
                    self.separate_next = true;
                }
                Node::Block(Tag::BlockQuote(_), _, children) => {
                    render_document(children, layout, &format!("{prefix}> "), &mut out);
                }
                Node::Block(_, _, children) => render_document(children, layout, prefix, &mut out),
                Node::Inline(_, _) => {}
            }
            self.section_start = node.range().end;
            self.next_node += 1;
        } else {
            if std::mem::take(&mut self.separate_next) {
                push_gap(layout.bg, &mut out, self.previous_nonblank);
            }
            self.plain(nodes, layout, prefix, &mut out);
        }
        self.remember(&out);
        Some(out)
    }

    fn plain(
        &mut self,
        nodes: &[Node<'_>],
        layout: Layout<'_>,
        prefix: &str,
        out: &mut Vec<Line<'static>>,
    ) {
        let node = &nodes[self.next_node];
        if prefix.is_empty() {
            preserve_separator(
                layout.source,
                node.range().start,
                layout.bg,
                out,
                self.previous_nonblank,
            );
            let start = self.next_node;
            while self.next_node < nodes.len() && !nodes[self.next_node].needs_layout() {
                self.next_node += 1;
            }
            let run = nodes[start..self.next_node].iter().collect::<Vec<_>>();
            render_plain(&run, self.section_start, layout, out);
        } else {
            if let Node::Block(Tag::BlockQuote(_), _, children) = node {
                render_document(children, layout, &format!("{prefix}> "), out);
            } else {
                let mut spans = Vec::new();
                inline_spans(node, MarkdownStyle.blockquote(), layout, &mut spans);
                out.extend(prefix_lines(vec![Line::from(spans)], prefix, layout.width, layout.bg));
            }
            self.next_node += 1;
        }
    }

    fn remember(&mut self, rows: &[Line<'static>]) {
        if let Some(last) = rows.last() {
            self.previous_nonblank = last.spans.iter().any(|span| !span.content.trim().is_empty());
        }
    }
}

fn render_document(
    nodes: &[Node<'_>],
    layout: Layout<'_>,
    prefix: &str,
    out: &mut Vec<Line<'static>>,
) {
    let mut state = DocumentState {
        previous_nonblank: out
            .last()
            .is_some_and(|line| line.spans.iter().any(|span| !span.content.trim().is_empty())),
        ..Default::default()
    };
    while let Some(rows) = state.next(nodes, layout, prefix) {
        out.extend(rows);
    }
}

struct CodeRows {
    code: String,
    language: String,
    highlighter: Option<highlight::SourceHighlighter>,
    next_byte: usize,
    empty_pending: bool,
}

impl CodeRows {
    fn new(kind: &CodeBlockKind<'_>, content: &[Node<'_>]) -> (Line<'static>, Self) {
        let mut code = content
            .iter()
            .filter_map(|node| match node {
                Node::Inline(Event::Text(text), _) => Some(text.as_ref()),
                _ => None,
            })
            .collect::<String>();
        if code.ends_with('\n') {
            code.pop();
        }
        let language = match kind {
            CodeBlockKind::Fenced(info) => info.split([' ', '\t', ',']).next().unwrap_or_default(),
            CodeBlockKind::Indented => "",
        }
        .to_owned();
        let label = if language.is_empty() { "code" } else { &language };
        let line = Line::from(Span::styled(label.to_owned(), MarkdownStyle::code_label()));
        let empty_pending = code.is_empty() || code.ends_with('\n');
        (line, Self { code, language, highlighter: None, next_byte: 0, empty_pending })
    }

    fn next(&mut self, prefix: &str, layout: Layout<'_>) -> Option<Vec<Line<'static>>> {
        let line = if self.next_byte < self.code.len() {
            let rest = &self.code[self.next_byte..];
            let bytes = rest.find('\n').map_or(rest.len(), |at| at + 1);
            self.next_byte += bytes;
            let highlighter = self.highlighter.get_or_insert_with(|| {
                highlight::SourceHighlighter::for_code(
                    (!self.language.is_empty()).then_some(self.language.as_str()),
                )
            });
            highlighter.line(&rest[..bytes], true).unwrap_or_default()
        } else if std::mem::take(&mut self.empty_pending) {
            self.highlighter.get_or_insert_with(|| {
                highlight::SourceHighlighter::for_code(
                    (!self.language.is_empty()).then_some(self.language.as_str()),
                )
            });
            Line::default()
        } else {
            return None;
        };
        Some(prefix_block_lines(vec![line], prefix, layout.width, layout.bg))
    }
}

/// Consecutive blocks without layout render as one source slice, so reference definitions and
/// block spacing between them stay intact.
fn render_plain(run: &[&Node<'_>], start: usize, layout: Layout<'_>, out: &mut Vec<Line<'static>>) {
    if layout.control.cancelled() {
        return;
    }
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
    out.extend(super::render_with_tui_markdown(&text, layout.width, layout.bg));
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
fn push_gap(bg: Option<Color>, out: &mut Vec<Line<'static>>, previous_nonblank: bool) {
    if out.last().map_or(previous_nonblank, |line| {
        line.spans.iter().any(|span| !span.content.trim().is_empty())
    }) {
        out.push(
            Line::default().style(bg.map_or(Style::default(), |color| Style::default().bg(color))),
        );
    }
}

fn preserve_separator(
    source: &str,
    start: usize,
    bg: Option<Color>,
    out: &mut Vec<Line<'static>>,
    previous_nonblank: bool,
) {
    let blank_line_before =
        source[..start].rsplit('\n').take_while(|line| line.trim().is_empty()).take(2).count() == 2;
    if blank_line_before {
        push_gap(bg, out, previous_nonblank);
    }
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
    if layout.control.cancelled() {
        return;
    }
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
    prefix: &str,
    width: u16,
    bg: Option<Color>,
) -> Vec<Line<'static>> {
    let style = bg.map_or(Style::default(), |color| Style::default().bg(color));
    let mut out = Vec::new();
    for line in lines {
        let chunks = line
            .spans
            .into_iter()
            .map(|span| StyledChunk {
                text: span.content.into_owned(),
                style: line.style.patch(span.style).patch(style),
            })
            .collect::<Vec<_>>();
        let rows = wrap_styled_chunks_with_hanging_prefix(
            &[StyledChunk { text: prefix.to_owned(), style }],
            &chunks,
            usize::from(width),
            style,
        );
        out.extend(rows);
    }
    out
}

/// Code retains its leading whitespace.
fn prefix_block_lines(
    lines: Vec<Line<'static>>,
    prefix: &str,
    width: u16,
    bg: Option<Color>,
) -> Vec<Line<'static>> {
    let prefix_width = u16::try_from(display_width(prefix)).unwrap_or(u16::MAX);
    if prefix_width >= width {
        return prefix_lines(lines, prefix, width, bg);
    }
    let style = bg.map_or(Style::default(), |color| Style::default().bg(color));
    lines
        .into_iter()
        .flat_map(|line| split_line_at_width(line, usize::from(width - prefix_width)))
        .map(|mut line| {
            line.spans.insert(0, Span::styled(prefix.to_owned(), style));
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
