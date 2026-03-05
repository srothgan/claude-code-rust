// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Inline permission rendering: standard tool permissions, plan approval, and
//! `AskUserQuestion` question-choice UI.

use crate::agent::model::PermissionOptionKind;
use crate::app::{InlinePermission, ToolCallInfo};
use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::markdown_inline_spans;

/// Render inline permission options on a single compact line.
/// Options are dynamic and include shortcuts only when applicable.
/// Unfocused permissions are dimmed to indicate they don't have keyboard input.
pub(super) fn render_permission_lines(
    tc: &ToolCallInfo,
    perm: &InlinePermission,
) -> Vec<Line<'static>> {
    if tc.is_exit_plan_mode_tool() || is_plan_approval_permission(perm) {
        return render_plan_approval_lines(tc, perm);
    }
    if is_question_permission(perm, tc) {
        return render_question_permission_lines(tc, perm);
    }

    // Unfocused permissions: show a dimmed "waiting for focus" line
    if !perm.focused {
        return vec![
            Line::default(),
            Line::from(Span::styled(
                "  \u{25cb} Waiting for input\u{2026} (\u{2191}\u{2193} to focus)",
                Style::default().fg(theme::DIM),
            )),
        ];
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let dot = Span::styled("  \u{00b7}  ", Style::default().fg(theme::DIM));

    for (i, opt) in perm.options.iter().enumerate() {
        let is_selected = i == perm.selected_index;
        let is_allow = matches!(
            opt.kind,
            PermissionOptionKind::AllowOnce
                | PermissionOptionKind::AllowSession
                | PermissionOptionKind::AllowAlways
        );

        let (icon, icon_color) = if is_allow {
            ("\u{2713}", Color::Green) // check
        } else {
            ("\u{2717}", Color::Red) // cross
        };

        // Separator between options
        if i > 0 {
            spans.push(dot.clone());
        }

        // Selection indicator
        if is_selected {
            spans.push(Span::styled(
                "\u{25b8} ",
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
            ));
        }

        spans.push(Span::styled(format!("{icon} "), Style::default().fg(icon_color)));

        let name_style = if is_selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let mut name_spans = markdown_inline_spans(&opt.name);
        if name_spans.is_empty() {
            spans.push(Span::styled(opt.name.clone(), name_style));
        } else {
            for span in &mut name_spans {
                span.style = span.style.patch(name_style);
            }
            spans.extend(name_spans);
        }

        let shortcut = match opt.kind {
            PermissionOptionKind::AllowOnce => " (Ctrl+y)",
            PermissionOptionKind::AllowSession | PermissionOptionKind::AllowAlways => " (Ctrl+a)",
            PermissionOptionKind::RejectOnce => " (Ctrl+n)",
            PermissionOptionKind::RejectAlways
            | PermissionOptionKind::QuestionChoice
            | PermissionOptionKind::PlanApprove
            | PermissionOptionKind::PlanReject => "",
        };
        spans.push(Span::styled(shortcut, Style::default().fg(theme::DIM)));
    }

    vec![
        Line::default(),
        Line::from(spans),
        Line::from(Span::styled(
            "\u{2190}\u{2192} select  \u{2191}\u{2193} next  enter confirm  esc reject",
            Style::default().fg(theme::DIM),
        )),
    ]
}

fn is_question_permission(perm: &InlinePermission, tc: &ToolCallInfo) -> bool {
    tc.is_ask_question_tool()
        || perm.options.iter().all(|opt| matches!(opt.kind, PermissionOptionKind::QuestionChoice))
}

fn is_plan_approval_permission(perm: &InlinePermission) -> bool {
    perm.options.iter().any(|opt| {
        matches!(opt.kind, PermissionOptionKind::PlanApprove | PermissionOptionKind::PlanReject)
    })
}

fn parse_exit_plan_mode_allowed_prompts(raw_input: Option<&serde_json::Value>) -> Vec<String> {
    let Some(raw) = raw_input else {
        return Vec::new();
    };
    let Some(arr) = raw.get("allowedPrompts").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            let prompt = item.get("prompt")?.as_str()?;
            let tool = item.get("tool")?.as_str()?;
            Some(format!("{tool}: {prompt}"))
        })
        .collect()
}

fn render_plan_approval_lines(tc: &ToolCallInfo, perm: &InlinePermission) -> Vec<Line<'static>> {
    if !perm.focused {
        return vec![
            Line::default(),
            Line::from(Span::styled(
                "  \u{25cb} Waiting for input\u{2026} (\u{2191}\u{2193} to focus)",
                Style::default().fg(theme::DIM),
            )),
        ];
    }

    let mut lines = vec![Line::default()];

    // Show pre-approved actions requested by Claude, if any.
    let allowed_prompts = parse_exit_plan_mode_allowed_prompts(tc.raw_input.as_ref());
    if !allowed_prompts.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Pre-approved actions:",
            Style::default().fg(theme::DIM),
        )));
        for prompt_text in allowed_prompts {
            lines.push(Line::from(vec![
                Span::styled("    \u{2022} ", Style::default().fg(theme::DIM)),
                Span::styled(prompt_text, Style::default().fg(Color::White)),
            ]));
        }
        lines.push(Line::default());
    }

    // Stacked approve / reject options.
    for (i, opt) in perm.options.iter().enumerate() {
        let is_selected = i == perm.selected_index;
        let (icon, icon_color, shortcut) = match opt.kind {
            PermissionOptionKind::PlanApprove => ("\u{2713}", Color::Green, " [y]"),
            PermissionOptionKind::PlanReject => ("\u{2717}", Color::Red, " [n]"),
            _ => ("\u{00b7}", Color::Gray, ""),
        };

        let name_style = if is_selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };

        let mut line_spans: Vec<Span<'static>> = Vec::new();
        if is_selected {
            line_spans.push(Span::styled(
                "  \u{25b8} ",
                Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
            ));
        } else {
            line_spans.push(Span::raw("    "));
        }
        line_spans.push(Span::styled(format!("{icon} "), Style::default().fg(icon_color)));
        line_spans.push(Span::styled(opt.name.clone(), name_style));
        line_spans.push(Span::styled(shortcut, Style::default().fg(theme::DIM)));
        lines.push(Line::from(line_spans));
    }

    lines.push(Line::from(Span::styled(
        "  \u{2191}\u{2193} select  enter confirm  y approve  n/esc reject",
        Style::default().fg(theme::DIM),
    )));

    lines
}

#[derive(Default)]
struct AskQuestionMeta {
    header: Option<String>,
    question: Option<String>,
    question_index: Option<usize>,
    total_questions: Option<usize>,
}

fn parse_ask_question_meta(raw_input: Option<&serde_json::Value>) -> AskQuestionMeta {
    let Some(raw) = raw_input else {
        return AskQuestionMeta::default();
    };

    let question = raw
        .get("questions")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| items.first())
        .and_then(serde_json::Value::as_object);

    AskQuestionMeta {
        header: question
            .and_then(|q| q.get("header"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        question: question
            .and_then(|q| q.get("question"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        question_index: raw
            .get("question_index")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok()),
        total_questions: raw
            .get("total_questions")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok()),
    }
}

fn render_question_permission_lines(
    tc: &ToolCallInfo,
    perm: &InlinePermission,
) -> Vec<Line<'static>> {
    let meta = parse_ask_question_meta(tc.raw_input.as_ref());
    let header = meta.header.unwrap_or_else(|| "Question".to_owned());
    let question_text = meta.question.unwrap_or_else(|| tc.title.clone());
    let progress = match (meta.question_index, meta.total_questions) {
        (Some(index), Some(total)) if total > 0 => format!(" ({}/{total})", index + 1),
        _ => String::new(),
    };

    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled("  ? ", Style::default().fg(theme::RUST_ORANGE)),
            Span::styled(
                format!("{header}{progress}"),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    for row in question_text.lines() {
        lines.push(Line::from(vec![Span::styled(
            format!("    {row}"),
            Style::default().fg(Color::Gray),
        )]));
    }

    if !perm.focused {
        lines.push(Line::from(Span::styled(
            "  waiting for input... (Up/Down to focus)",
            Style::default().fg(theme::DIM),
        )));
        return lines;
    }

    let horizontal = perm.options.len() <= 3
        && perm.options.iter().all(|opt| {
            opt.description.as_deref().is_none_or(str::is_empty) && opt.name.chars().count() <= 20
        });

    if horizontal {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, opt) in perm.options.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  |  ", Style::default().fg(theme::DIM)));
            }
            let selected = i == perm.selected_index;
            if selected {
                spans.push(Span::styled(
                    "\u{25b8} ",
                    Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("  ", Style::default().fg(theme::DIM)));
            }
            let style = if selected {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::styled(opt.name.clone(), style));
        }
        lines.push(Line::from(spans));
    } else {
        for (i, opt) in perm.options.iter().enumerate() {
            let selected = i == perm.selected_index;
            let bullet = if selected { "  \u{25b8} " } else { "  \u{25cb} " };
            let name_style = if selected {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            lines.push(Line::from(vec![
                Span::styled(
                    bullet,
                    if selected {
                        Style::default().fg(theme::RUST_ORANGE)
                    } else {
                        Style::default().fg(theme::DIM)
                    },
                ),
                Span::styled(opt.name.clone(), name_style),
            ]));
            if let Some(desc) = opt.description.as_ref().map(|d| d.trim()).filter(|d| !d.is_empty())
            {
                lines.push(Line::from(Span::styled(
                    format!("      {desc}"),
                    Style::default().fg(theme::DIM),
                )));
            }
        }
    }

    lines.push(Line::from(Span::styled(
        "  Left/Right or Up/Down select  Enter confirm  Esc cancel",
        Style::default().fg(theme::DIM),
    )));
    lines
}
