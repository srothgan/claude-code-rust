// SPDX-License-Identifier: Apache-2.0

//! Markdown source dialect shared by rendering and streaming boundaries.

use pulldown_cmark::{Event, Options, Parser, Tag};
use std::ops::Range;

pub(crate) fn parser(source: &str) -> Parser<'_> {
    // Match the dialect used by tui-markdown for blocks it renders.
    let options = Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT
        | Options::ENABLE_MATH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_GFM;
    Parser::new_ext(source, options)
}

/// Tables and code blocks must stay whole when streaming text is cached.
pub(crate) fn table_and_code_ranges(source: &str) -> Vec<Range<usize>> {
    parser(source)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(event, Event::Start(Tag::Table(_) | Tag::CodeBlock(_))).then_some(range)
        })
        .collect()
}

/// A render handoff must retain blocks and unfinished inline syntax that cross it.
pub(crate) fn context_crosses_boundary(
    source: &str,
    current_end: usize,
    next_start: usize,
) -> bool {
    parser(source).into_offset_iter().any(|(event, range)| {
        if range.start >= current_end || range.end <= next_start {
            return false;
        }
        match event {
            Event::Start(Tag::List(_) | Tag::Table(_) | Tag::CodeBlock(_)) => true,
            Event::Start(Tag::Paragraph) => {
                has_open_inline_delimiter(&source[range.start..current_end])
            }
            _ => false,
        }
    })
}

// A partial emphasis/code delimiter is still literal text to the parser until it
// closes. Inspect only its parsed paragraph, never literal table or code content.
fn has_open_inline_delimiter(text: &str) -> bool {
    let mut in_code = false;
    let mut strong_asterisk_open = false;
    let mut strong_underscore_open = false;
    let mut chars = text.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch == '\\' {
            let _ = chars.next();
            continue;
        }
        if ch == '`' {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let rest = &text[idx..];
        if rest.starts_with("**") {
            strong_asterisk_open = !strong_asterisk_open;
            let _ = chars.next();
        } else if rest.starts_with("__") {
            strong_underscore_open = !strong_underscore_open;
            let _ = chars.next();
        }
    }
    in_code || strong_asterisk_open || strong_underscore_open
}
