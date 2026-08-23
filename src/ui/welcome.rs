// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::app::WelcomeBlock;
use crate::ui::theme;
use crate::ui::wrap::{
    StyledChunk, display_width, join_column_lines, wrap_styled_chunks,
    wrap_styled_chunks_with_hanging_prefix,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const FERRIS_ART: &[&str] =
    &[r"    _~^~^~_     ", r"\) /  o o  \ (/ ", r"  '_   -   _'   ", r"  / '-----' \   "];
const FERRIS_LEFT_PADDING: &str = "  ";
const FERRIS_TEXT_GAP: usize = 2;
const MIN_INLINE_FIELD_VALUE_WIDTH: usize = 8;
const WELCOME_FIELD_LABELS: &[&str] = &["Version", "Subscription", "Cwd", "Session ID", "Tips"];

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

pub(crate) fn overview_lines(
    block: &WelcomeBlock,
    loading_status: Option<&str>,
    width: u16,
) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if width == 0 {
        return vec![Line::default()];
    }

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

    let ferris_width = ferris_column_width();
    let overview_offset = ferris_width.saturating_add(FERRIS_TEXT_GAP);
    let overview_width = width.saturating_sub(overview_offset);
    let side_by_side = overview_width >= minimum_overview_width();
    let text_width = if side_by_side { overview_width } else { width };
    let text_rows = overview_text_rows(
        block,
        &subscription_value,
        subscription_style,
        &session_value,
        text_width,
    );
    let mut lines = if side_by_side {
        join_column_lines(ferris_rows(), text_rows, ferris_width, FERRIS_TEXT_GAP)
    } else {
        text_rows
    };
    lines.push(Line::default());
    lines
}

fn overview_text_rows(
    block: &WelcomeBlock,
    subscription_value: &str,
    subscription_style: Style,
    session_value: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme::DIM);
    let mut rows = Vec::new();
    rows.extend(welcome_field_lines("Version", &block.version, dim, width));
    rows.extend(welcome_field_lines("Subscription", subscription_value, subscription_style, width));
    rows.extend(welcome_field_lines("Cwd", &block.cwd, dim, width));
    rows.extend(welcome_field_lines("Session ID", session_value, dim, width));
    rows.push(Line::default());
    rows.extend(welcome_field_lines("Tips", selected_tip(block), dim, width));
    rows
}

fn welcome_field_lines(
    label: &str,
    value: &str,
    value_style: Style,
    width: usize,
) -> Vec<Line<'static>> {
    let prefix =
        vec![StyledChunk { text: format!("{label}: "), style: Style::default().fg(theme::DIM) }];
    let body = vec![StyledChunk { text: value.to_owned(), style: value_style }];
    let prefix_width = prefix.iter().map(|chunk| display_width(&chunk.text)).sum::<usize>();
    if width.saturating_sub(prefix_width) < MIN_INLINE_FIELD_VALUE_WIDTH {
        let mut rows = wrap_styled_chunks(&prefix, width);
        rows.extend(wrap_styled_chunks(&body, width));
        return rows;
    }

    wrap_styled_chunks_with_hanging_prefix(&prefix, &body, width, Style::default())
}

fn ferris_column_width() -> usize {
    display_width(FERRIS_LEFT_PADDING)
        .saturating_add(FERRIS_ART.iter().map(|line| display_width(line)).max().unwrap_or(0))
}

fn ferris_rows() -> Vec<Line<'static>> {
    FERRIS_ART
        .iter()
        .map(|art| {
            Line::from(Span::styled(
                format!("{FERRIS_LEFT_PADDING}{art}"),
                Style::default().fg(theme::RUST_ORANGE),
            ))
        })
        .collect()
}

fn minimum_overview_width() -> usize {
    WELCOME_FIELD_LABELS
        .iter()
        .map(|label| display_width(&format!("{label}: ")))
        .max()
        .unwrap_or(0)
        .saturating_add(MIN_INLINE_FIELD_VALUE_WIDTH)
}

fn welcome_value_missing(value: &str) -> bool {
    value.trim().is_empty() || value == "-"
}

pub(crate) fn selected_tip(block: &WelcomeBlock) -> &'static str {
    let Some(first_tip) = WELCOME_TIPS.first().copied() else {
        return "Enter sends, Shift+Enter inserts a newline, and Ctrl+C clears or quits";
    };
    let len_u64 = u64::try_from(WELCOME_TIPS.len()).unwrap_or(1);
    let idx_u64 = block.tip_seed % len_u64;
    let idx = usize::try_from(idx_u64).unwrap_or(0);
    WELCOME_TIPS.get(idx).copied().unwrap_or(first_tip)
}

#[cfg(test)]
mod tests {
    use super::{Line, WELCOME_TIPS, overview_lines};
    use crate::app::{ChatMessage, MessageBlock};
    use crate::ui::wrap::line_display_width;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn overview_lines_render_expected_fields() {
        let mut message = ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-");
        let MessageBlock::Welcome(block) = &mut message.blocks[0] else {
            panic!("expected welcome block");
        };
        block.tip_seed = 16;
        let lines: Vec<String> = overview_lines(block, None, 120)
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

    #[test]
    fn wide_overview_wraps_tip_below_its_value() {
        let mut message = ChatMessage::welcome("1.2.3", "Pro", "/workspace/demo", "session-123");
        let MessageBlock::Welcome(block) = &mut message.blocks[0] else {
            panic!("expected welcome block");
        };
        block.tip_seed = 7;

        let lines = overview_lines(block, None, 110);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        let tip_row = text.iter().position(|line| line.contains("Tips: Start")).expect("tip row");

        assert_eq!(text[tip_row].find("Tips:"), Some(20));
        assert_eq!(text[tip_row + 1].find("noise"), Some(26));
        assert!(lines.iter().all(|line| line_display_width(line) <= 110));
    }

    #[test]
    fn wide_overview_wraps_long_metadata_below_its_value() {
        let mut message = ChatMessage::welcome(
            "1.2.3",
            "Pro",
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda",
            "session-123",
        );
        let MessageBlock::Welcome(block) = &mut message.blocks[0] else {
            panic!("expected welcome block");
        };

        let text = overview_lines(block, None, 70).iter().map(line_text).collect::<Vec<_>>();
        let cwd_row = text.iter().position(|line| line.contains("Cwd: alpha")).expect("cwd row");

        assert_eq!(text[cwd_row].find("Cwd:"), Some(20));
        assert_eq!(text[cwd_row + 1].find("iota"), Some(25));
    }

    #[test]
    fn narrow_overview_hides_ferris_and_keeps_hanging_indent() {
        let mut message = ChatMessage::welcome("1.2.3", "Pro", "/workspace/demo", "session-123");
        let MessageBlock::Welcome(block) = &mut message.blocks[0] else {
            panic!("expected welcome block");
        };
        block.tip_seed = 7;

        let lines = overview_lines(block, None, 36);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        let tip_row =
            text.iter().position(|line| line.starts_with("Tips: Start")).expect("tip row");

        assert!(!text.iter().any(|line| line.contains("_~^~^~_")));
        assert_eq!(text[tip_row + 1].find(|ch: char| !ch.is_whitespace()), Some(6));
        assert!(lines.iter().all(|line| line_display_width(line) <= 36));
    }
}
