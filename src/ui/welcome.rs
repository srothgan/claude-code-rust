// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::WelcomeBlock;
use crate::ui::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

const FERRIS_ART: &[&str] =
    &[r"    _~^~^~_     ", r"\) /  o o  \ (/ ", r"  '_   -   _'   ", r"  / '-----' \   "];

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
            format!("Tips: {}", selected_tip(block)),
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
    use super::{WELCOME_TIPS, overview_lines};
    use crate::app::{ChatMessage, MessageBlock};

    #[test]
    fn overview_lines_render_expected_fields() {
        let message = ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/cwd", "-");
        let MessageBlock::Welcome(block) = &message.blocks[0] else {
            panic!("expected welcome block");
        };
        let lines: Vec<String> = overview_lines(block, None)
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
}
