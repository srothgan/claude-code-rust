// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{BlockCache, IncrementalMarkdown, MarkdownRenderKey, TextBlock, WelcomeBlock};
#[cfg(test)]
use crate::app::{ChatMessage, MessageRole};
use crate::ui::theme;
use crate::ui::tool_call;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

#[cfg(test)]
use super::message_rows::build_message_rows;

const FERRIS_ART: &[&str] =
    &[r"    _~^~^~_     ", r"\) /  o o  \ (/ ", r"  '_   -   _'   ", r"  / '-----' \   "];

// Prepared for future randomized welcome-tip selection. Intentionally unused
// until the welcome UI is switched from a single hard-coded tip.
const WELCOME_TIPS: &[&str] = &[
    "Use /mode plan before larger changes, then switch back to code once the plan is clear",
    "Use /mcp to connect live tools and docs instead of pasting stale context into chat",
    "Keep repo instructions short in CLAUDE.md and update them when mistakes repeat",
    "Start prompts with the goal, relevant context, and constraints so Claude needs fewer corrections",
    "Ask Claude for a plan first on multi-step work instead of jumping straight to edits",
    "Give success criteria Claude can verify: tests, lint, screenshots, or exact outputs",
    "For visual work, paste screenshots or mockups so Claude can verify UI changes instead of guessing",
    "Start a fresh thread with /new-session when the task changes and old context is noise",
    "Use /compact when a session gets long and you want to keep the thread but trim context",
    "Use /resume <session_id> to jump back into earlier work without rebuilding context",
    "Use /docs shortcuts to see the live keyboard shortcuts for the current app state",
    "Use /docs commands to inspect the slash commands this app and the SDK expose",
    "If Claude drifts, refine or restate the plan early instead of piling on corrective prompts",
    "For tricky bugs, provide clear repro steps and runtime evidence instead of guessing fixes",
    "Point Claude at the relevant files, errors, and constraints instead of pasting everything",
    "If you do not know the exact file, let Claude search first and only pin the files that matter",
    "Ask codebase questions first in unfamiliar areas instead of coding blind",
    "Review diffs carefully even when the output looks plausible on first read",
    "Use hooks for checks that must run every time instead of relying on reminder text alone",
    "Turn repeated workflows into CLAUDE.md guidance only after they work reliably by hand",
    "For larger features, let Claude clarify requirements and edge cases through structured questions",
    "Use separate sessions for unrelated work so planning, debugging, and review stay clean",
];

/// Snapshot of the app state needed by the spinner -- extracted before
/// the message loop so we don't need `&App` (which conflicts with `&mut msg`).
#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct SpinnerState {
    pub frame: usize,
    /// True when this message owns the currently active assistant turn.
    pub is_active_turn_assistant: bool,
    /// True when this message should show the initial empty-turn thinking indicator.
    pub show_empty_thinking: bool,
    /// True when this message should show the thinking indicator.
    pub show_thinking: bool,
    /// True when this message should show the compaction indicator.
    pub show_compacting: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct MessageRenderContext<'a> {
    pub(crate) tool_render_context: tool_call::ToolCallRenderContext<'a>,
    pub(crate) width: u16,
    pub(crate) layout_generation: u64,
    pub(crate) options: MessageRenderOptions,
}

impl<'a> MessageRenderContext<'a> {
    pub(crate) fn new(
        current_mode_id: Option<&'a str>,
        width: u16,
        layout_generation: u64,
        options: MessageRenderOptions,
    ) -> Self {
        Self {
            tool_render_context: tool_call::ToolCallRenderContext { current_mode_id },
            width,
            layout_generation,
            options,
        }
    }
}

#[cfg(test)]
pub(crate) fn render_message(
    msg: &mut ChatMessage,
    spinner: &SpinnerState,
    width: u16,
    out: &mut Vec<Line<'static>>,
) {
    let render_context = MessageRenderContext::new(
        None,
        width,
        0,
        MessageRenderOptions { include_trailing_separator: true },
    );
    render_message_rows(msg, spinner, render_context, out);
}

#[cfg(test)]
pub(crate) fn render_message_with_separator(
    msg: &mut ChatMessage,
    spinner: &SpinnerState,
    width: u16,
    include_trailing_separator: bool,
    out: &mut Vec<Line<'static>>,
) {
    let render_context = MessageRenderContext::new(
        None,
        width,
        0,
        MessageRenderOptions { include_trailing_separator },
    );
    render_message_rows(msg, spinner, render_context, out);
}

#[cfg(test)]
pub(crate) fn render_message_with_separator_and_layout_generation(
    msg: &mut ChatMessage,
    spinner: &SpinnerState,
    width: u16,
    layout_generation: u64,
    include_trailing_separator: bool,
    out: &mut Vec<Line<'static>>,
) {
    let render_context = MessageRenderContext::new(
        None,
        width,
        layout_generation,
        MessageRenderOptions { include_trailing_separator },
    );
    render_message_rows(msg, spinner, render_context, out);
}

#[derive(Clone, Copy)]
pub(crate) struct MessageRenderOptions {
    pub include_trailing_separator: bool,
}

#[cfg(test)]
fn render_message_rows(
    msg: &mut ChatMessage,
    spinner: &SpinnerState,
    render_context: MessageRenderContext<'_>,
    out: &mut Vec<Line<'static>>,
) {
    let rows = build_message_rows(msg, spinner, render_context);
    for segment in rows.segments {
        match segment {
            super::message_rows::MessageRowSegment::Blank => out.push(Line::default()),
            super::message_rows::MessageRowSegment::Lines { lines } => out.extend(lines),
        }
    }
}

fn welcome_lines(block: &WelcomeBlock, _width: u16) -> Vec<Line<'static>> {
    welcome_overview_lines(block, None)
}

pub(crate) fn welcome_overview_lines(
    block: &WelcomeBlock,
    loading_status: Option<&str>,
) -> Vec<Line<'static>> {
    let pad = "  ";
    let loading = loading_status.unwrap_or("Loading");
    let subscription_missing = welcome_value_missing(&block.subscription);
    let session_missing = welcome_value_missing(&block.session_id);
    let subscription_value =
        if subscription_missing { loading.to_owned() } else { block.subscription.clone() };
    let session_value = if session_missing { loading.to_owned() } else { block.session_id.clone() };
    let subscription_style = if subscription_missing {
        Style::default().fg(theme::DIM)
    } else {
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
    };
    let text_rows = vec![
        welcome_field_line("Version", &block.version, Style::default().fg(theme::DIM)),
        welcome_field_line("Subscription", &subscription_value, subscription_style),
        welcome_field_line("Cwd", &block.cwd, Style::default().fg(theme::DIM)),
        welcome_field_line("Session ID", &session_value, Style::default().fg(theme::DIM)),
        Line::default(),
        Line::from(Span::styled(
            format!("Tips: {}", selected_welcome_tip(block)),
            Style::default().fg(theme::DIM),
        )),
    ];

    let art_width = FERRIS_ART.iter().map(|line| line.chars().count()).max().unwrap_or(0);
    let row_count = FERRIS_ART.len().max(text_rows.len());
    let mut lines = Vec::with_capacity(row_count + 1);
    for idx in 0..row_count {
        let art = FERRIS_ART.get(idx).copied().unwrap_or_default();
        let mut spans = vec![Span::styled(
            format!("{pad}{art:<art_width$}{pad}"),
            Style::default().fg(theme::RUST_ORANGE),
        )];
        if let Some(text_row) = text_rows.get(idx) {
            spans.extend(text_row.spans.clone());
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::default());
    lines
}

fn welcome_field_line(label: &str, value: &str, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(theme::DIM)),
        Span::styled(value.to_owned(), value_style),
    ])
}

fn welcome_value_missing(value: &str) -> bool {
    value.trim().is_empty() || value == "-"
}

pub(crate) fn selected_welcome_tip(block: &WelcomeBlock) -> &'static str {
    let Some(first_tip) = WELCOME_TIPS.first().copied() else {
        return "Enter sends, Shift+Enter inserts a newline, and Ctrl+C quits";
    };
    let len_u64 = u64::try_from(WELCOME_TIPS.len()).unwrap_or(1);
    let idx_u64 = block.tip_seed % len_u64;
    let idx = usize::try_from(idx_u64).unwrap_or(0);
    WELCOME_TIPS.get(idx).copied().unwrap_or(first_tip)
}

pub(super) fn render_welcome_cached(
    block: &mut WelcomeBlock,
    width: u16,
    out: &mut Vec<Line<'static>>,
) {
    if let Some(cached_lines) = block.cache.get() {
        out.extend_from_slice(cached_lines);
        return;
    }

    let fresh = welcome_lines(block, width);
    let h = {
        let _t = crate::perf::start_with("msg::wrap_height", "lines", fresh.len());
        Paragraph::new(Text::from(fresh.clone())).wrap(Wrap { trim: false }).line_count(width)
    };
    block.cache.store(fresh);
    block.cache.set_height(h, width);
    if let Some(stored) = block.cache.get() {
        out.extend_from_slice(stored);
    }
}

/// Preprocess markdown that `tui_markdown` doesn't handle well.
/// Headings (`# Title`) become `**Title**` (bold) with a blank line before.
/// Handles variations: `#Title`, `#  Title`, `  ## Title  `, etc.
/// Links are left as-is -- `tui_markdown` handles `[title](url)` natively.
fn preprocess_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            // Strip all leading '#' characters
            let after_hashes = trimmed.trim_start_matches('#');
            // Extract heading content (trim spaces between # and text, and trailing)
            let content = after_hashes.trim();
            if !content.is_empty() {
                // Blank line before heading for visual separation
                if !result.is_empty() && !result.ends_with("\n\n") {
                    result.push('\n');
                }
                result.push_str("**");
                result.push_str(content);
                result.push_str("**\n");
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    if !text.ends_with('\n') {
        result.pop();
    }
    result
}

/// Render a text block with caching. Uses paragraph-level incremental markdown
/// during streaming to avoid re-parsing the entire text every frame.
///
/// Cache hierarchy:
/// 1. `BlockCache` (full block) -- hit for completed messages (no changes).
/// 2. `IncrementalMarkdown` (per-paragraph) -- only tail paragraph re-parsed during streaming.
pub(super) fn render_text_cached(
    text: &str,
    cache: &mut BlockCache,
    incr: &mut IncrementalMarkdown,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    out: &mut Vec<Line<'static>>,
) {
    // Fast path only when the cached lines were measured at this width.
    // Markdown tables produce width-dependent logical lines before paragraph
    // wrapping, so a fresh cache from another width is not safe to reuse.
    if cache.height_at(width).is_some()
        && let Some(cached_lines) = cache.get()
    {
        crate::perf::mark_with("msg::cache_hit", "lines", cached_lines.len());
        out.extend_from_slice(cached_lines);
        return;
    }
    crate::perf::mark("msg::cache_miss");

    let _t = crate::perf::start("msg::render_text");

    // Build a render function that handles preprocessing + tui_markdown
    let render_fn = |src: &str| -> Vec<Line<'static>> {
        let mut preprocessed = preprocess_markdown(src);
        if preserve_newlines {
            preprocessed = force_markdown_line_breaks(&preprocessed);
        }
        super::document_table::render_markdown_with_tables(&preprocessed, width, bg)
    };
    let render_key = MarkdownRenderKey { width, bg, preserve_newlines };

    // Ensure any previously invalidated paragraph caches are re-rendered
    let _ = text;
    incr.ensure_rendered(render_key, &render_fn);

    // Render: cached paragraphs + fresh tail
    let fresh = incr.lines(render_key, &render_fn);

    // Store in the full block cache with wrapped height.
    // For streaming messages this will be invalidated on the next chunk,
    // but for completed messages it persists.
    let h = {
        let _t = crate::perf::start_with("msg::wrap_height", "lines", fresh.len());
        Paragraph::new(Text::from(fresh.clone())).wrap(Wrap { trim: false }).line_count(width)
    };
    cache.store(fresh);
    cache.set_height(h, width);
    if let Some(stored) = cache.get() {
        out.extend_from_slice(stored);
    }
}

pub(super) fn render_text_block_cached(
    block: &mut TextBlock,
    width: u16,
    bg: Option<Color>,
    preserve_newlines: bool,
    out: &mut Vec<Line<'static>>,
) {
    render_text_cached(
        &block.text,
        &mut block.cache,
        &mut block.markdown,
        width,
        bg,
        preserve_newlines,
        out,
    );
}

/// Convert single line breaks into hard breaks so user-entered newlines persist.
fn force_markdown_line_breaks(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        if !line.is_empty() {
            out.push_str(line);
            out.push_str("  ");
        }
        if i + 1 < lines.len() || text.ends_with('\n') {
            out.push('\n');
        }
    }
    if text.ends_with('\n') {
        // preserve trailing newline
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{
        ChatMessage, InlinePermission, MessageBlock, NoticeBlock, SystemSeverity, TextBlock,
        TextBlockSpacing,
    };
    use pretty_assertions::assert_eq;
    use ratatui::widgets::{Paragraph, Wrap};

    // preprocess_markdown

    #[test]
    fn preprocess_h1_heading() {
        let result = preprocess_markdown("# Hello");
        assert!(result.contains("**Hello**"));
        assert!(!result.contains('#'));
    }

    #[test]
    fn preprocess_h3_heading() {
        let result = preprocess_markdown("### Deeply Nested");
        assert!(result.contains("**Deeply Nested**"));
    }

    #[test]
    fn preprocess_non_heading_passthrough() {
        let input = "Just normal text\nwith multiple lines";
        let result = preprocess_markdown(input);
        assert_eq!(result, input);
    }

    #[test]
    fn preprocess_mixed_headings_and_text() {
        let input = "# Title\nSome text\n## Subtitle\nMore text";
        let result = preprocess_markdown(input);
        assert!(result.contains("**Title**"));
        assert!(result.contains("Some text"));
        assert!(result.contains("**Subtitle**"));
        assert!(result.contains("More text"));
    }

    #[test]
    fn preprocess_heading_no_space() {
        let result = preprocess_markdown("#Title");
        assert!(result.contains("**Title**"));
    }

    #[test]
    fn preprocess_heading_extra_spaces() {
        let result = preprocess_markdown("#   Spaced Out   ");
        assert!(result.contains("**Spaced Out**"));
    }

    #[test]
    fn preprocess_indented_heading() {
        let result = preprocess_markdown("  ## Indented");
        assert!(result.contains("**Indented**"));
    }

    #[test]
    fn preprocess_empty_heading() {
        let result = preprocess_markdown("# ");
        assert_eq!(result, "# ");
    }

    #[test]
    fn preprocess_empty_string() {
        assert_eq!(preprocess_markdown(""), "");
    }

    #[test]
    fn preprocess_preserves_trailing_newline() {
        let result = preprocess_markdown("hello\n");
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn preprocess_no_trailing_newline() {
        let result = preprocess_markdown("hello");
        assert!(!result.ends_with('\n'));
    }

    #[test]
    fn preprocess_blank_line_before_heading() {
        let input = "text\n\n# Heading";
        let result = preprocess_markdown(input);
        assert!(!result.contains("\n\n\n"));
        assert!(result.contains("**Heading**"));
    }

    #[test]
    fn preprocess_consecutive_headings() {
        let input = "# First\n# Second";
        let result = preprocess_markdown(input);
        assert!(result.contains("**First**"));
        assert!(result.contains("**Second**"));
    }

    #[test]
    fn preprocess_hash_in_code_not_heading() {
        let result = preprocess_markdown("# actual heading");
        assert!(result.contains("**actual heading**"));
    }

    /// H6 heading (6 `#` chars).
    #[test]
    fn preprocess_h6_heading() {
        let result = preprocess_markdown("###### Deep H6");
        assert!(result.contains("**Deep H6**"));
        assert!(!result.contains('#'));
    }

    /// Heading with markdown formatting inside.
    #[test]
    fn preprocess_heading_with_bold_inside() {
        let result = preprocess_markdown("# **bold** and *italic*");
        assert!(result.contains("****bold** and *italic***"));
    }

    /// Heading at end of file with no trailing newline.
    #[test]
    fn preprocess_heading_at_eof_no_newline() {
        let result = preprocess_markdown("text\n# Final");
        assert!(result.contains("**Final**"));
        assert!(!result.ends_with('\n'));
    }

    /// Only hashes with no text: `###` - content after stripping is empty, passthrough.
    #[test]
    fn preprocess_only_hashes() {
        let result = preprocess_markdown("###");
        assert_eq!(result, "###");
    }

    /// Very long heading.
    #[test]
    fn preprocess_very_long_heading() {
        let long_text = "A".repeat(1000);
        let input = format!("# {long_text}");
        let result = preprocess_markdown(&input);
        assert!(result.starts_with("**"));
        assert!(result.contains(&long_text));
    }

    /// Unicode emoji in heading.
    #[test]
    fn preprocess_unicode_heading() {
        let result = preprocess_markdown("# \u{1F680} Launch \u{4F60}\u{597D}");
        assert!(result.contains("**\u{1F680} Launch \u{4F60}\u{597D}**"));
    }

    /// Quoted heading: `> # Heading` - starts with `>` not `#`, so passthrough.
    #[test]
    fn preprocess_blockquote_heading_passthrough() {
        let result = preprocess_markdown("> # Quoted heading");
        // Line starts with `>`, not `#`, so trimmed starts with `>` not `#`
        assert!(!result.contains("**"));
        assert!(result.contains("> # Quoted heading"));
    }

    /// All heading levels in sequence.
    #[test]
    fn preprocess_all_heading_levels() {
        let input = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
        let result = preprocess_markdown(input);
        for label in ["H1", "H2", "H3", "H4", "H5", "H6"] {
            assert!(result.contains(&format!("**{label}**")), "missing {label}");
        }
    }

    #[test]
    fn welcome_lines_render_expected_fields() {
        let message = ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-");
        let MessageBlock::Welcome(block) = &message.blocks[0] else {
            panic!("expected welcome block");
        };
        let rendered = welcome_lines(block, 120);
        let lines: Vec<String> = rendered
            .into_iter()
            .map(|line| line.spans.into_iter().map(|s| s.content).collect())
            .collect();
        assert!(lines.iter().any(|line| line.contains("_~^~^~_")));
        assert!(!lines.iter().any(|line| line.contains("Welcome back to Claude, in Rust!")));
        assert!(lines.iter().any(|line| line.contains("Version:")));
        assert!(lines.iter().any(|line| line.contains("Subscription: Loading")));
        assert!(lines.iter().any(|line| line.contains("Cwd: /cwd")));
        assert!(lines.iter().any(|line| line.contains("Session ID: Loading")));
        assert!(lines.iter().any(|line| line.contains("Tips: ")));
        assert!(
            WELCOME_TIPS.iter().any(|tip| lines.iter().any(|line| line.contains(tip))),
            "expected one welcome tip to be rendered"
        );
    }

    // force_markdown_line_breaks

    #[test]
    fn force_breaks_adds_trailing_spaces() {
        let result = force_markdown_line_breaks("line1\nline2");
        assert!(result.contains("line1  \n"));
        assert!(result.contains("line2  "));
    }

    #[test]
    fn force_breaks_preserves_trailing_newline() {
        let result = force_markdown_line_breaks("hello\n");
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn force_breaks_empty_lines_no_trailing_spaces() {
        let result = force_markdown_line_breaks("a\n\nb");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("  "));
        assert_eq!(lines[1], "");
        assert!(lines[2].ends_with("  "));
    }

    #[test]
    fn force_breaks_single_line_no_trailing_newline() {
        let result = force_markdown_line_breaks("hello");
        assert_eq!(result, "hello  ");
    }

    #[test]
    fn force_breaks_many_consecutive_empty_lines() {
        let result = force_markdown_line_breaks("a\n\n\nb");
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 4);
    }

    /// Empty input.
    #[test]
    fn force_breaks_empty_input() {
        let result = force_markdown_line_breaks("");
        assert_eq!(result, "");
    }

    /// Only empty lines.
    #[test]
    fn force_breaks_only_empty_lines() {
        let result = force_markdown_line_breaks("\n\n\n");
        let lines: Vec<&str> = result.lines().collect();
        // All lines are empty, so no trailing spaces added
        for line in &lines {
            assert!(line.is_empty(), "empty line got content: {line:?}");
        }
    }

    /// Line already ending with two spaces - gets two more.
    #[test]
    fn force_breaks_already_has_trailing_spaces() {
        let result = force_markdown_line_breaks("hello  \nworld");
        // "hello  " + "  " = "hello    "
        assert!(result.starts_with("hello    "));
    }

    /// Single newline (no content).
    #[test]
    fn force_breaks_single_newline() {
        let result = force_markdown_line_breaks("\n");
        // One empty line, should stay empty with trailing newline
        assert_eq!(result, "\n");
    }

    fn make_text_message(role: MessageRole, text: &str) -> ChatMessage {
        ChatMessage::new(role, vec![MessageBlock::Text(TextBlock::from_complete(text))], None)
    }

    fn make_assistant_split_message(first: &str, second: &str) -> ChatMessage {
        ChatMessage::new(
            MessageRole::Assistant,
            vec![
                MessageBlock::Text(
                    TextBlock::from_complete(first)
                        .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
                ),
                MessageBlock::Text(TextBlock::from_complete(second)),
            ],
            None,
        )
    }

    fn make_assistant_notice_message() -> ChatMessage {
        ChatMessage::new(
            MessageRole::Assistant,
            vec![
                MessageBlock::Text(TextBlock::from_complete("Before notice")),
                MessageBlock::Notice(NoticeBlock::from_complete(
                    SystemSeverity::Warning,
                    "Warning inline",
                )),
                MessageBlock::Text(TextBlock::from_complete("After notice")),
            ],
            None,
        )
    }

    fn make_tool_call_info(
        id: &str,
        sdk_tool_name: &str,
        status: crate::agent::model::ToolCallStatus,
        text: &str,
    ) -> crate::app::ToolCallInfo {
        crate::app::ToolCallInfo {
            id: id.to_owned(),
            title: id.to_owned(),
            sdk_tool_name: sdk_tool_name.to_owned(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status,
            content: if text.is_empty() {
                Vec::new()
            } else {
                vec![crate::agent::model::ToolCallContent::from(text.to_owned())]
            },
            hidden: false,
            terminal_id: None,
            terminal_command: None,
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: BlockCache::default(),
            pending_permission: None,
            pending_question: None,
        }
    }

    fn pending_permission(focused: bool) -> InlinePermission {
        let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
        InlinePermission {
            options: vec![
                crate::agent::model::PermissionOption::new(
                    "allow",
                    "Allow",
                    crate::agent::model::PermissionOptionKind::AllowOnce,
                ),
                crate::agent::model::PermissionOption::new(
                    "deny",
                    "Deny",
                    crate::agent::model::PermissionOptionKind::RejectOnce,
                ),
            ],
            display: None,
            response_tx,
            selected_index: 0,
            focused,
        }
    }

    fn render_lines_to_strings(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    fn line_index_containing(lines: &[String], needle: &str) -> usize {
        lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("expected line containing {needle:?}"))
    }

    fn make_welcome_message(subscription: &str, cwd: &str, session_id: &str) -> ChatMessage {
        let mut message =
            ChatMessage::welcome(env!("CARGO_PKG_VERSION"), subscription, cwd, session_id);
        let Some(MessageBlock::Welcome(block)) = message.blocks.first_mut() else {
            panic!("expected welcome block");
        };
        block.tip_seed = 0;
        message
    }

    fn idle_spinner() -> SpinnerState {
        SpinnerState {
            frame: 0,
            is_active_turn_assistant: false,
            show_empty_thinking: false,
            show_thinking: false,
            show_compacting: false,
        }
    }

    fn ground_truth_height(msg: &mut ChatMessage, spinner: &SpinnerState, width: u16) -> usize {
        let mut lines = Vec::new();
        render_message(msg, spinner, width, &mut lines);
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }).line_count(width)
    }

    fn measure_message_height(msg: &mut ChatMessage, spinner: &SpinnerState, width: u16) -> usize {
        measure_message_height_with_separator(msg, spinner, width, true)
    }

    fn measure_message_height_with_separator(
        msg: &mut ChatMessage,
        spinner: &SpinnerState,
        width: u16,
        include_trailing_separator: bool,
    ) -> usize {
        build_message_rows(
            msg,
            spinner,
            MessageRenderContext::new(
                None,
                width,
                0,
                MessageRenderOptions { include_trailing_separator },
            ),
        )
        .height
    }

    #[test]
    fn measure_height_matches_ground_truth_for_long_soft_wrap() {
        let text = "A".repeat(500);
        let spinner = idle_spinner();

        let mut measured_msg = make_text_message(MessageRole::User, &text);
        let mut truth_msg = make_text_message(MessageRole::User, &text);

        let h = measure_message_height(&mut measured_msg, &spinner, 32);
        let truth = ground_truth_height(&mut truth_msg, &spinner, 32);

        assert_eq!(h, truth);
    }

    #[test]
    fn user_role_label_wrap_height_matches_ground_truth() {
        let spinner = idle_spinner();
        let mut measured_msg = make_text_message(MessageRole::User, "ok");
        let mut truth_msg = make_text_message(MessageRole::User, "ok");

        let h = measure_message_height(&mut measured_msg, &spinner, 2);
        let truth = ground_truth_height(&mut truth_msg, &spinner, 2);

        assert_eq!(h, truth);
        assert!(h >= 3);
    }

    #[test]
    fn system_role_label_wrap_height_matches_ground_truth() {
        let spinner = idle_spinner();
        let mut measured_msg =
            make_text_message(MessageRole::System(Some(SystemSeverity::Warning)), "rate limit");
        let mut truth_msg =
            make_text_message(MessageRole::System(Some(SystemSeverity::Warning)), "rate limit");

        let h = measure_message_height(&mut measured_msg, &spinner, 4);
        let truth = ground_truth_height(&mut truth_msg, &spinner, 4);

        assert_eq!(h, truth);
        assert!(h >= 4);
    }

    #[test]
    fn welcome_role_label_wrap_height_matches_ground_truth() {
        let spinner = idle_spinner();
        let mut measured_msg = make_welcome_message("Max", "~/project", "session-1");
        let mut truth_msg = make_welcome_message("Max", "~/project", "session-1");

        let h = measure_message_height(&mut measured_msg, &spinner, 4);
        let truth = ground_truth_height(&mut truth_msg, &spinner, 4);

        assert_eq!(h, truth);
    }

    #[test]
    fn assistant_split_paragraph_inserts_a_structural_blank_line_between_blocks() {
        let spinner = idle_spinner();
        let mut msg = make_assistant_split_message("First paragraph", "Second paragraph");
        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 80, &mut lines);

        let rendered = render_lines_to_strings(&lines);
        let first_idx =
            rendered.iter().position(|line| line.contains("First paragraph")).expect("first block");
        let second_idx = rendered
            .iter()
            .position(|line| line.contains("Second paragraph"))
            .expect("second block");

        assert_eq!(rendered.first().map(String::as_str), Some("Claude"));
        assert!(second_idx > first_idx + 1);
        assert!(rendered[first_idx + 1].is_empty());
    }

    #[test]
    fn assistant_notice_block_renders_inline_between_neighboring_text_blocks() {
        let spinner = idle_spinner();
        let mut msg = make_assistant_notice_message();
        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 80, &mut lines);

        let rendered = render_lines_to_strings(&lines);
        let before_idx =
            rendered.iter().position(|line| line.contains("Before notice")).expect("before text");
        let notice_idx =
            rendered.iter().position(|line| line.contains("Warning inline")).expect("notice");
        let after_idx =
            rendered.iter().position(|line| line.contains("After notice")).expect("after text");

        assert_eq!(rendered.first().map(String::as_str), Some("Claude"));
        assert!(before_idx < notice_idx && notice_idx < after_idx);
    }

    #[test]
    fn assistant_notice_block_is_tinted_by_severity() {
        let spinner = idle_spinner();
        let mut msg = make_assistant_notice_message();
        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 80, &mut lines);

        let notice_line = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content == "Warning inline"))
            .expect("expected notice line");
        assert!(
            notice_line
                .spans
                .iter()
                .filter(|span| !span.content.is_empty())
                .all(|span| span.style.fg == Some(theme::STATUS_WARNING))
        );
    }

    #[test]
    fn assistant_notice_height_matches_ground_truth() {
        let spinner = idle_spinner();
        let mut measured_msg = make_assistant_notice_message();
        let mut truth_msg = make_assistant_notice_message();

        let h = measure_message_height(&mut measured_msg, &spinner, 16);
        let truth = ground_truth_height(&mut truth_msg, &spinner, 16);

        assert_eq!(h, truth);
    }

    #[test]
    fn assistant_split_paragraph_height_matches_rendered_gap() {
        let spinner = idle_spinner();
        let mut measured = make_assistant_split_message("First paragraph", "Second paragraph");
        let mut truth = make_assistant_split_message("First paragraph", "Second paragraph");

        let h = measure_message_height(&mut measured, &spinner, 80);
        let truth_h = ground_truth_height(&mut truth, &spinner, 80);
        assert_eq!(h, truth_h);
        assert_eq!(h, 5);
    }

    #[test]
    fn assistant_message_can_render_without_trailing_separator() {
        let spinner = idle_spinner();
        let mut msg = make_text_message(MessageRole::Assistant, "hello");
        let mut lines = Vec::new();

        render_message_with_separator(&mut msg, &spinner, 80, false, &mut lines);

        assert_eq!(render_lines_to_strings(&lines), vec!["Claude".to_owned(), "hello".to_owned()]);

        let h = measure_message_height_with_separator(&mut msg, &spinner, 80, false);
        assert_eq!(h, 2);
    }

    #[test]
    fn empty_last_assistant_thinking_omits_trailing_separator() {
        let spinner = SpinnerState {
            is_active_turn_assistant: true,
            show_empty_thinking: true,
            ..idle_spinner()
        };
        let mut msg = ChatMessage::new(MessageRole::Assistant, Vec::new(), None);
        let mut lines = Vec::new();

        render_message_with_separator(&mut msg, &spinner, 80, false, &mut lines);

        let rendered = render_lines_to_strings(&lines);
        assert_eq!(rendered.len(), 2);
        assert_eq!(rendered[0], "Claude");
        assert!(rendered[1].contains("Thinking..."));

        let h = measure_message_height_with_separator(&mut msg, &spinner, 80, false);
        assert_eq!(h, 2);
    }

    #[test]
    fn empty_last_assistant_thinking_wrap_height_matches_ground_truth() {
        let spinner = SpinnerState {
            is_active_turn_assistant: true,
            show_empty_thinking: true,
            ..idle_spinner()
        };
        let mut measured_msg = ChatMessage::new(MessageRole::Assistant, Vec::new(), None);
        let mut truth_msg = ChatMessage::new(MessageRole::Assistant, Vec::new(), None);

        let h = measure_message_height_with_separator(&mut measured_msg, &spinner, 6, false);
        let mut truth_lines = Vec::new();
        render_message_with_separator(&mut truth_msg, &spinner, 6, false, &mut truth_lines);
        let truth =
            Paragraph::new(Text::from(truth_lines)).wrap(Wrap { trim: false }).line_count(6);

        assert_eq!(h, truth);
        assert!(h > 2);
    }

    #[test]
    fn empty_last_assistant_compacting_omits_trailing_separator() {
        let spinner = SpinnerState {
            is_active_turn_assistant: true,
            show_compacting: true,
            ..idle_spinner()
        };
        let mut msg = ChatMessage::new(MessageRole::Assistant, Vec::new(), None);
        let mut lines = Vec::new();

        render_message_with_separator(&mut msg, &spinner, 80, false, &mut lines);

        let rendered = render_lines_to_strings(&lines);
        assert_eq!(rendered.len(), 2);
        assert_eq!(rendered[0], "Claude");
        assert!(rendered[1].contains("Compacting context..."));

        let h = measure_message_height_with_separator(&mut msg, &spinner, 80, false);
        assert_eq!(h, 2);
    }

    #[test]
    fn measure_height_matches_ground_truth_after_resize() {
        let text =
            "This is a single very long line without explicit line breaks to stress soft wrapping."
                .repeat(20);
        let spinner = idle_spinner();

        let mut measured_msg = make_text_message(MessageRole::Assistant, &text);
        let mut truth_wide = make_text_message(MessageRole::Assistant, &text);
        let mut truth_narrow = make_text_message(MessageRole::Assistant, &text);

        let h_wide = measure_message_height(&mut measured_msg, &spinner, 100);
        let wide_truth = ground_truth_height(&mut truth_wide, &spinner, 100);
        assert_eq!(h_wide, wide_truth);

        let h_narrow = measure_message_height(&mut measured_msg, &spinner, 28);
        let narrow_truth = ground_truth_height(&mut truth_narrow, &spinner, 28);
        assert_eq!(h_narrow, narrow_truth);
    }

    #[test]
    fn markdown_table_rerenders_when_width_changes_in_both_directions() {
        let spinner = idle_spinner();
        let table = concat!(
            "| Name | Description |\n",
            "| --- | --- |\n",
            "| foo | long wrapped value |\n",
        );
        let mut msg = make_text_message(MessageRole::Assistant, table);

        let mut wide_lines = Vec::new();
        render_message_with_separator_and_layout_generation(
            &mut msg,
            &spinner,
            40,
            1,
            true,
            &mut wide_lines,
        );
        let wide_rendered = render_lines_to_strings(&wide_lines);
        assert!(wide_rendered.iter().any(|line| line.contains("Name")));
        assert!(wide_rendered.iter().any(|line| line.contains('─')));
        assert!(!wide_rendered.iter().any(|line| line.contains("Name:")));

        let mut narrow_lines = Vec::new();
        render_message_with_separator_and_layout_generation(
            &mut msg,
            &spinner,
            12,
            2,
            true,
            &mut narrow_lines,
        );
        let narrow_rendered = render_lines_to_strings(&narrow_lines);
        assert!(narrow_rendered.iter().any(|line| line.contains("Name:")));
        assert!(narrow_rendered.iter().any(|line| line.contains("Description")));
        assert!(!narrow_rendered.iter().any(|line| line.contains('─')));

        let mut wide_again_lines = Vec::new();
        render_message_with_separator_and_layout_generation(
            &mut msg,
            &spinner,
            40,
            3,
            true,
            &mut wide_again_lines,
        );
        let wide_again_rendered = render_lines_to_strings(&wide_again_lines);
        assert!(wide_again_rendered.iter().any(|line| line.contains("Name")));
        assert!(wide_again_rendered.iter().any(|line| line.contains('─')));
        assert!(!wide_again_rendered.iter().any(|line| line.contains("Name:")));
    }

    #[test]
    fn welcome_height_matches_ground_truth() {
        let spinner = idle_spinner();
        let mut measured_msg = make_welcome_message("Max", "~/project", "session-1");
        let mut truth_msg = make_welcome_message("Max", "~/project", "session-1");

        let h = measure_message_height(&mut measured_msg, &spinner, 52);
        let truth = ground_truth_height(&mut truth_msg, &spinner, 52);
        assert_eq!(h, truth);
    }

    #[test]
    fn system_warning_severity_renders_warning_label() {
        let spinner = idle_spinner();
        let mut msg = make_text_message(
            MessageRole::System(Some(SystemSeverity::Warning)),
            "Rate limit warning",
        );
        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(rendered.iter().any(|line| line.contains("Warning")));
        assert!(rendered.iter().any(|line| line.contains("Rate limit warning")));
    }

    #[test]
    fn assistant_message_suppresses_hidden_subagent_child_tools() {
        let spinner = idle_spinner();

        let mut hidden_tool = make_tool_call_info(
            "hidden-child",
            "Bash",
            crate::agent::model::ToolCallStatus::Completed,
            "child output",
        );
        hidden_tool.hidden = true;
        let mut msg = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(hidden_tool))],
            None,
        );

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(!rendered.iter().any(|line| line.contains("hidden-child")));
        assert!(!rendered.iter().any(|line| line.contains("child output")));
    }

    #[test]
    fn assistant_message_renders_hidden_subagent_child_permission_prompt() {
        let spinner = idle_spinner();
        let mut hidden_tool = make_tool_call_info(
            "hidden-permission",
            "Bash",
            crate::agent::model::ToolCallStatus::InProgress,
            "",
        );
        hidden_tool.hidden = true;
        hidden_tool.pending_permission = Some(pending_permission(true));
        let mut msg = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(hidden_tool))],
            None,
        );

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(rendered.iter().any(|line| line.contains("hidden-permission")));
        assert!(rendered.iter().any(|line| line.contains("Allow")));
        assert!(rendered.iter().any(|line| line.contains("Deny")));
    }

    #[test]
    fn assistant_message_renders_only_focused_hidden_subagent_child_permission_prompt() {
        let spinner = idle_spinner();
        let mut focused_tool = make_tool_call_info(
            "focused-permission",
            "Bash",
            crate::agent::model::ToolCallStatus::InProgress,
            "",
        );
        focused_tool.hidden = true;
        focused_tool.pending_permission = Some(pending_permission(true));
        let mut waiting_tool = make_tool_call_info(
            "waiting-permission",
            "Bash",
            crate::agent::model::ToolCallStatus::InProgress,
            "",
        );
        waiting_tool.hidden = true;
        waiting_tool.pending_permission = Some(pending_permission(false));
        let mut msg = ChatMessage::new(
            MessageRole::Assistant,
            vec![
                MessageBlock::ToolCall(Box::new(focused_tool)),
                MessageBlock::ToolCall(Box::new(waiting_tool)),
            ],
            None,
        );

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(rendered.iter().any(|line| line.contains("focused-permission")));
        assert!(!rendered.iter().any(|line| line.contains("waiting-permission")));
        assert!(!rendered.iter().any(|line| line.contains("Waiting for input")));
    }

    #[test]
    fn assistant_message_keeps_unfocused_main_agent_permission_prompt_visible() {
        let spinner = idle_spinner();
        let mut main_tool = make_tool_call_info(
            "main-permission",
            "Bash",
            crate::agent::model::ToolCallStatus::InProgress,
            "",
        );
        main_tool.pending_permission = Some(pending_permission(false));
        let mut msg = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(main_tool))],
            None,
        );

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(rendered.iter().any(|line| line.contains("main-permission")));
        assert!(rendered.iter().any(|line| line.contains("Waiting for input")));
    }

    #[test]
    fn assistant_message_defers_focused_hidden_child_permission_after_later_subagent_roots() {
        let spinner = idle_spinner();
        let root_a = make_tool_call_info(
            "root-a",
            "Task",
            crate::agent::model::ToolCallStatus::InProgress,
            "first subagent",
        );
        let mut focused_tool = make_tool_call_info(
            "focused-permission",
            "Bash",
            crate::agent::model::ToolCallStatus::InProgress,
            "",
        );
        focused_tool.hidden = true;
        focused_tool.pending_permission = Some(pending_permission(true));
        let root_b = make_tool_call_info(
            "root-b",
            "Agent",
            crate::agent::model::ToolCallStatus::InProgress,
            "second subagent",
        );
        let mut msg = ChatMessage::new(
            MessageRole::Assistant,
            vec![
                MessageBlock::ToolCall(Box::new(root_a)),
                MessageBlock::ToolCall(Box::new(focused_tool)),
                MessageBlock::ToolCall(Box::new(root_b)),
            ],
            None,
        );

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        let first_root_line = line_index_containing(&rendered, "root-a");
        let second_root_line = line_index_containing(&rendered, "root-b");
        let focused_idx = line_index_containing(&rendered, "focused-permission");

        assert!(first_root_line < second_root_line);
        assert!(second_root_line < focused_idx);
    }

    #[test]
    fn assistant_message_keeps_focused_hidden_child_permission_before_later_main_tool() {
        let spinner = idle_spinner();
        let root = make_tool_call_info(
            "root",
            "Task",
            crate::agent::model::ToolCallStatus::InProgress,
            "subagent",
        );
        let mut focused_tool = make_tool_call_info(
            "focused-permission",
            "Bash",
            crate::agent::model::ToolCallStatus::InProgress,
            "",
        );
        focused_tool.hidden = true;
        focused_tool.pending_permission = Some(pending_permission(true));
        let main_tool = make_tool_call_info(
            "main-tool",
            "Read",
            crate::agent::model::ToolCallStatus::InProgress,
            "main agent tool",
        );
        let mut msg = ChatMessage::new(
            MessageRole::Assistant,
            vec![
                MessageBlock::ToolCall(Box::new(root)),
                MessageBlock::ToolCall(Box::new(focused_tool)),
                MessageBlock::ToolCall(Box::new(main_tool)),
            ],
            None,
        );

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        let root_idx = line_index_containing(&rendered, "root");
        let focused_idx = line_index_containing(&rendered, "focused-permission");
        let main_idx = line_index_containing(&rendered, "main-tool");

        assert!(root_idx < focused_idx);
        assert!(focused_idx < main_idx);
    }

    #[test]
    fn assistant_heading_at_start_does_not_render_blank_line_after_label() {
        let spinner = idle_spinner();
        let mut msg = make_text_message(MessageRole::Assistant, "\n# Heading\nBody");

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 80, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert_eq!(rendered.first().map(String::as_str), Some("Claude"));
        let heading_idx =
            rendered.iter().position(|line| line.contains("Heading")).expect("heading");
        assert_eq!(heading_idx, 1);
        assert!(!rendered[heading_idx].is_empty());
    }

    #[test]
    fn assistant_heading_at_start_height_matches_rendered_output() {
        let spinner = idle_spinner();
        let mut measured = make_text_message(MessageRole::Assistant, "\n# Heading\nBody");
        let mut truth = make_text_message(MessageRole::Assistant, "\n# Heading\nBody");

        let h = measure_message_height(&mut measured, &spinner, 80);
        let truth_h = ground_truth_height(&mut truth, &spinner, 80);

        assert_eq!(h, truth_h);
    }

    #[test]
    fn assistant_message_does_not_show_empty_turn_thinking_after_content_exists() {
        let spinner = SpinnerState {
            is_active_turn_assistant: true,
            show_empty_thinking: true,
            ..idle_spinner()
        };
        let mut msg = make_text_message(MessageRole::Assistant, "done");

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(!rendered.iter().any(|line| line.contains("Thinking...")));
    }

    #[test]
    fn assistant_message_suppresses_thinking_line_while_compacting() {
        let spinner = SpinnerState {
            is_active_turn_assistant: true,
            show_thinking: true,
            show_compacting: true,
            ..idle_spinner()
        };
        let mut msg = make_text_message(MessageRole::Assistant, "done");

        let mut lines = Vec::new();
        render_message(&mut msg, &spinner, 120, &mut lines);
        let rendered = render_lines_to_strings(&lines);

        assert!(rendered.iter().any(|line| line.contains("Compacting context...")));
        assert!(!rendered.iter().any(|line| line.contains("Thinking...")));
    }
}
