// SPDX-License-Identifier: Apache-2.0
//! Inline interaction rendering: permissions, plan approvals, and `AskUserQuestion`.

use crate::agent::model::PermissionOptionKind;
use crate::app::{InlinePermission, InlineQuestion, ToolCallInfo};
use crate::ui::theme;
use crate::ui::tool_display;
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

    let mut lines = vec![Line::default()];
    // Unfocused permissions: show a dimmed "waiting for focus" line.
    if !perm.focused {
        lines.push(Line::from(Span::styled(
            "  \u{25cb} Waiting for input... (Tab to focus)",
            Style::default().fg(theme::DIM),
        )));
        return lines;
    }

    if perm.subagent_context.is_none()
        && let Some(display) = permission_display_lines(tc, perm)
    {
        lines.extend(display);
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
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
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
    }

    lines.push(Line::from(spans));
    lines.push(Line::from(Span::styled(
        "\u{2190}\u{2192} select  \u{2191}\u{2193} next  enter confirm  esc reject",
        Style::default().fg(theme::DIM),
    )));
    lines
}

fn permission_display_lines(
    tc: &ToolCallInfo,
    perm: &InlinePermission,
) -> Option<Vec<Line<'static>>> {
    let display = perm.display.as_ref()?;
    let title = display.title.as_ref().map(|value| value.trim()).filter(|value| !value.is_empty());
    let display_name =
        display.display_name.as_ref().map(|value| value.trim()).filter(|value| !value.is_empty());
    let description =
        display.description.as_ref().map(|value| value.trim()).filter(|value| !value.is_empty());
    if title.is_none() && display_name.is_none() && description.is_none() {
        return None;
    }

    let header = title
        .into_iter()
        .chain(display_name)
        .find(|value| !is_redundant_permission_header(tc, value));

    let mut lines = Vec::new();
    if let Some(title) = header {
        lines.push(Line::from(Span::styled(
            format!("  {title}"),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(description) = description {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(description.to_owned(), Style::default().fg(theme::DIM)),
        ]));
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    Some(lines)
}

fn is_redundant_permission_header(tc: &ToolCallInfo, value: &str) -> bool {
    let normalized = normalize_permission_header(value);
    let tool_label = tool_display::tool_name_label(&tc.sdk_tool_name);
    let tool_label = normalize_permission_header(tool_label.label.as_ref());
    let display_title = tool_display::tool_title(&tc.sdk_tool_name, &tc.title);
    let display_title = normalize_permission_header(display_title.as_ref());
    let sdk_name = normalize_permission_header(&tc.sdk_tool_name);
    let title = normalize_permission_header(&tc.title);
    normalized == tool_label
        || normalized == display_title
        || normalized == sdk_name
        || normalized == title
}

fn normalize_permission_header(value: &str) -> String {
    value.trim().to_ascii_lowercase()
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
                "  \u{25cb} Waiting for input... (Tab to focus)",
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
        let (icon, icon_color) = match opt.kind {
            PermissionOptionKind::PlanApprove => ("\u{2713}", Color::Green),
            PermissionOptionKind::PlanReject => ("\u{2717}", Color::Red),
            _ => ("\u{00b7}", Color::Gray),
        };

        let name_style = if is_selected {
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
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
        lines.push(Line::from(line_spans));
    }

    lines.push(Line::from(Span::styled(
        "  \u{2191}\u{2193} select  enter confirm  esc reject",
        Style::default().fg(theme::DIM),
    )));

    lines
}

#[allow(clippy::too_many_lines)]
pub(super) fn render_question_lines(question: &InlineQuestion) -> Vec<Line<'static>> {
    let progress = match question.total_questions {
        total if total > 0 => format!(" ({}/{total})", question.question_index + 1),
        _ => String::new(),
    };

    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled("  ? ", Style::default().fg(theme::RUST_ORANGE)),
            Span::styled(
                format!("{}{}", question.prompt.header, progress),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    for row in question.prompt.question.lines() {
        lines.push(Line::from(vec![Span::styled(
            format!("    {row}"),
            if question.focused {
                Style::default().fg(theme::RUST_ORANGE)
            } else {
                Style::default().fg(Color::Gray)
            },
        )]));
    }

    if !question.focused {
        lines.push(Line::from(Span::styled(
            "  waiting for input... (Tab to focus)",
            Style::default().fg(theme::DIM),
        )));
        return lines;
    }

    let horizontal = question.prompt.options.len() <= 3
        && question.prompt.options.iter().all(|opt| {
            opt.description.as_deref().is_none_or(str::is_empty) && opt.label.chars().count() <= 20
        });

    if let Some(timeout) = question.idle_timeout {
        lines.push(Line::styled(
            format!(
                "  Auto-continue after {}s idle; unselected questions are skipped.",
                timeout.as_secs()
            ),
            Style::default().fg(theme::DIM),
        ));
    }

    if horizontal {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, opt) in question.prompt.options.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  |  ", Style::default().fg(theme::DIM)));
            }
            let selected = i == question.focused_option_index;
            let checked = question.selected_option_indices.contains(&i);
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
            let marker = if checked { "[x] " } else { "[ ] " };
            if question.prompt.multi_select {
                spans.push(Span::styled(marker, Style::default().fg(theme::DIM)));
            }
            spans.push(Span::styled(opt.label.clone(), style));
        }
        lines.push(Line::from(spans));
    } else {
        for (i, opt) in question.prompt.options.iter().enumerate() {
            let selected = i == question.focused_option_index;
            let checked = question.selected_option_indices.contains(&i);
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
                Span::styled(
                    if question.prompt.multi_select {
                        if checked { "[x] " } else { "[ ] " }
                    } else {
                        ""
                    },
                    Style::default().fg(theme::DIM),
                ),
                Span::styled(opt.label.clone(), name_style),
                option_description_suffix(opt.description.as_deref()),
            ]));
            if i + 1 < question.prompt.options.len() {
                lines.push(Line::default());
            }
        }
    }

    let answer_text_indent = question_answer_text_indent(question);
    if let Some(preview) = question
        .prompt
        .options
        .get(question.focused_option_index)
        .and_then(|option| option.preview.as_deref())
        .map(str::trim)
        .filter(|preview| !preview.is_empty())
    {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("{answer_text_indent}Preview"),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));
        for row in preview.lines() {
            lines.push(Line::from(Span::styled(
                format!("{answer_text_indent}  {row}"),
                Style::default().fg(theme::DIM),
            )));
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled(
            format!(
                "{answer_text_indent}Notes{}: ",
                if question.editing_notes { " [editing]" } else { "" }
            ),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if question.notes.is_empty() { "<empty>".to_owned() } else { question.notes.clone() },
            if question.editing_notes {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(theme::DIM)
            },
        ),
    ]));

    lines.push(Line::from(Span::styled(
        if question.prompt.multi_select {
            "  Left/Right move  Space toggle  Tab notes  Enter confirm  Esc cancel"
        } else {
            "  Left/Right select  Tab notes  Enter confirm  Esc cancel"
        },
        Style::default().fg(theme::DIM),
    )));
    lines
}

fn question_answer_text_indent(question: &InlineQuestion) -> &'static str {
    if question.prompt.multi_select { "        " } else { "    " }
}

fn option_description_suffix(description: Option<&str>) -> Span<'static> {
    description.map(str::trim).filter(|desc| !desc.is_empty()).map_or_else(
        || Span::raw(""),
        |desc| {
            Span::styled(format!(" - {}", single_line_text(desc)), Style::default().fg(theme::DIM))
        },
    )
}

fn single_line_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod card_tests {
    use crate::agent::model::{
        PermissionDisplay, PermissionOption, PermissionOptionKind, QuestionOption, QuestionPrompt,
        ToolCallStatus,
    };
    use crate::app::{App, FocusTarget, ToolCallInfo};
    use crate::ui::tool_call::test_support::{
        assert_focused_option, attach_permission, attach_question, interaction_app,
        interaction_card, press,
    };
    use crossterm::event::KeyCode;
    use ratatui::text::Line;

    const LABELS: [&str; 3] = ["Cedar", "Maple", "Birch"];
    const PERMISSIONS: [&str; 3] = ["Allow once", "Allow session", "Reject request"];

    fn tool(name: &str) -> ToolCallInfo {
        ToolCallInfo {
            title: "Target operation".into(),
            ..crate::app::test_support::named_tool_call_info(
                "target",
                name,
                ToolCallStatus::InProgress,
            )
        }
    }

    fn prompt(multi: bool, described: bool) -> QuestionPrompt {
        let options = ["cedar-id", "maple-id", "birch-id"]
            .into_iter()
            .zip(LABELS)
            .enumerate()
            .map(|(index, (id, label))| {
                QuestionOption::new(id, label).description(described.then(|| {
                    ["  Evergreen\n  canopy ", "Broad\tleaf shade", "Pale bark"][index].to_owned()
                }))
            })
            .collect();
        QuestionPrompt::new("Which tree should we plant?", "Garden choice", multi, options)
    }

    fn permission_options(plan: bool) -> Vec<PermissionOption> {
        let kinds = if plan {
            [
                PermissionOptionKind::PlanApprove,
                PermissionOptionKind::PlanApprove,
                PermissionOptionKind::PlanReject,
            ]
        } else {
            [
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowSession,
                PermissionOptionKind::RejectOnce,
            ]
        };
        ["once-id", "session-id", "reject-id"]
            .into_iter()
            .zip(PERMISSIONS)
            .zip(kinds)
            .map(|((id, name), kind)| PermissionOption::new(id, name, kind))
            .collect()
    }

    fn text(rows: &[Line<'_>]) -> Vec<String> {
        rows.iter().map(Line::to_string).collect()
    }

    fn write_notes(app: &mut App, notes: &str) {
        press(app, KeyCode::Tab);
        for ch in notes.chars() {
            press(app, KeyCode::Char(ch));
        }
    }

    #[test]
    fn question_focus_preserves_its_prompt_and_restores_its_controls() {
        let mut app = interaction_app(vec![tool("AskUserQuestion")]);
        let _response = attach_question(&mut app, "target", prompt(false, false));
        write_notes(&mut app, "Keep garden notes");
        press(&mut app, KeyCode::Tab);
        for focused in [true, false, true] {
            app.input.set_text("neighbor draft");
            if focused {
                app.claim_focus_target(FocusTarget::Permission);
            } else {
                app.release_focus_target(FocusTarget::Permission);
            }
            app.rebuild_chat_focus_from_state();
            let rows = interaction_card(&mut app, "target", 100);
            let content = text(&rows);
            assert!(content.iter().any(|row| row.contains("Garden choice")));
            assert!(content.iter().any(|row| row.contains("Which tree should we plant?")));
            for label in LABELS {
                assert_eq!(
                    rows.iter().flat_map(|row| &row.spans).any(|span| span.content == label),
                    focused
                );
            }
            assert_eq!(content.iter().any(|row| row.contains("Keep garden notes")), focused);
            assert!(!content.iter().any(|row| row.contains("neighbor draft")));
            if focused {
                assert_focused_option(&rows, LABELS, 0);
            }
        }
    }

    #[test]
    fn question_descriptions_stay_with_their_own_options() {
        let mut app = interaction_app(vec![tool("AskUserQuestion")]);
        let _response = attach_question(&mut app, "target", prompt(false, true));
        let rows = interaction_card(&mut app, "target", 100);
        let content = text(&rows);
        let descriptions = ["Evergreen canopy", "Broad leaf shade", "Pale bark"];
        let mut previous = None;
        for (index, label) in LABELS.iter().enumerate() {
            let position = content.iter().position(|row| row.contains(label)).expect("option row");
            assert!(previous.is_none_or(|previous| previous < position), "option order");
            previous = Some(position);
            assert!(content[position].contains(descriptions[index]));
            for (other, description) in descriptions.iter().enumerate() {
                assert_eq!(content[position].contains(description), index == other);
            }
        }
    }

    #[test]
    fn question_preview_follows_focus_and_notes_survive_cached_transitions() {
        let mut app = interaction_app(vec![tool("AskUserQuestion")]);
        let mut question = prompt(true, true);
        question.options[0].preview = Some("Cedar preview".into());
        question.options[1].preview = Some("Maple preview".into());
        let _response = attach_question(&mut app, "target", question);
        write_notes(&mut app, "garden-only note");
        let editing = text(&interaction_card(&mut app, "target", 100));
        let notes = editing.iter().find(|row| row.contains("garden-only note")).expect("notes row");
        assert!(notes.contains("editing"));
        press(&mut app, KeyCode::Tab);
        for (focused, expected) in
            [(0, Some("Cedar preview")), (1, Some("Maple preview")), (2, None)]
        {
            if focused > 0 {
                press(&mut app, KeyCode::Right);
            }
            let rows = interaction_card(&mut app, "target", 100);
            let option_end = rows
                .iter()
                .position(|row| row.spans.iter().any(|span| span.content.trim() == "Preview"))
                .unwrap_or(rows.len());
            assert_focused_option(&rows[..option_end], LABELS, focused);
            assert!(
                rows.iter()
                    .flat_map(|row| &row.spans)
                    .any(|span| span.content == "garden-only note")
            );
            let content = text(&rows);
            let notes = content
                .iter()
                .find(|row| row.contains("garden-only note"))
                .expect("retained notes");
            assert!(!notes.contains("editing"));
            for preview in ["Cedar preview", "Maple preview"] {
                assert_eq!(
                    content.iter().any(|row| row.contains(preview)),
                    expected == Some(preview)
                );
            }
            if let Some(expected) = expected {
                let preview = content.iter().position(|row| row.contains(expected)).unwrap();
                let notes =
                    content.iter().position(|row| row.contains("garden-only note")).unwrap();
                assert!(preview < notes, "preview and notes retain separate regions");
            }
        }
    }

    #[test]
    fn permission_selection_follows_the_option_in_ordinary_and_plan_cards() {
        for (sdk_name, plan_kinds) in [("Bash", false), ("ExitPlanMode", false), ("Write", true)] {
            let mut app = interaction_app(vec![tool(sdk_name)]);
            let _response =
                attach_permission(&mut app, "target", permission_options(plan_kinds), None);
            let rows = interaction_card(&mut app, "target", 120);
            assert_focused_option(&rows, PERMISSIONS, 0);
            for selected in [1, 2] {
                press(&mut app, if plan_kinds { KeyCode::Down } else { KeyCode::Right });
                let rows = interaction_card(&mut app, "target", 120);
                assert_focused_option(&rows, PERMISSIONS, selected);
                let option_rows: Vec<_> = rows
                    .iter()
                    .filter(|row| {
                        row.spans.iter().any(|span| PERMISSIONS.contains(&span.content.as_ref()))
                    })
                    .collect();
                if sdk_name == "ExitPlanMode" || plan_kinds {
                    assert!(
                        option_rows.iter().all(|row| row
                            .spans
                            .iter()
                            .filter(|span| PERMISSIONS.contains(&span.content.as_ref()))
                            .count()
                            == 1),
                        "plan options must retain their own rows"
                    );
                }
            }
        }
    }

    #[test]
    fn permission_metadata_keeps_useful_headers_without_repeating_tool_identity() {
        for (sdk, title, name, expected) in [
            ("Bash", Some("Verify project"), Some("Fallback header"), Some("Verify project")),
            ("Bash", Some("  bAsH  "), Some("Fallback header"), Some("Fallback header")),
            ("Bash", None, Some("Fallback header"), Some("Fallback header")),
            ("Bash", Some("Target operation"), None, None),
            (
                "mcp__claude_ai_Strava__list_activities",
                Some(" Strava: List activities "),
                None,
                None,
            ),
            (
                "mcp__claude_ai_Strava__list_activities",
                Some("Strava: List activities"),
                Some("Distinct activity request"),
                Some("Distinct activity request"),
            ),
            ("Bash", Some("  "), Some(" "), None),
        ] {
            let mut target = tool(sdk);
            let identity = if sdk.starts_with("mcp__") {
                target.title = sdk.into();
                "Strava: List activities"
            } else {
                "Target operation"
            };
            let mut app = interaction_app(vec![target]);
            let display = PermissionDisplay::new()
                .title(title.map(str::to_owned))
                .display_name(name.map(str::to_owned))
                .description(Some("Reads only the target project".into()));
            let _response =
                attach_permission(&mut app, "target", permission_options(false), Some(display));
            let rows = interaction_card(&mut app, "target", 150);
            assert!(rows[0].to_string().contains(identity), "outer identity retained");
            let body: Vec<_> = rows[1..]
                .iter()
                .flat_map(|row| &row.spans)
                .map(|span| span.content.trim())
                .collect();
            assert!(body.contains(&"Reads only the target project"));
            for header in ["Verify project", "Fallback header", "Distinct activity request"] {
                assert_eq!(
                    body.iter().filter(|value| **value == header).count(),
                    usize::from(expected == Some(header))
                );
            }
            assert!(
                !body.iter().any(|value| value.eq_ignore_ascii_case(identity)
                    || value.eq_ignore_ascii_case("Bash"))
            );
            assert_focused_option(&rows, PERMISSIONS, 0);
        }
        let mut app = interaction_app(vec![tool("Bash")]);
        let _response = attach_permission(
            &mut app,
            "target",
            permission_options(false),
            Some(PermissionDisplay::new()),
        );
        assert_focused_option(&interaction_card(&mut app, "target", 100), PERMISSIONS, 0);
    }

    fn assert_checks(rows: &[Line<'_>], expected: [bool; 3]) {
        for (label, checked) in LABELS.into_iter().zip(expected) {
            let (row, index) = rows
                .iter()
                .find_map(|row| {
                    row.spans
                        .iter()
                        .position(|span| span.content == label)
                        .map(|index| (row, index))
                })
                .expect("option label");
            let marker = row.spans[..index]
                .iter()
                .rev()
                .take_while(|span| !LABELS.contains(&span.content.as_ref()))
                .find(|span| ["[x]", "[ ]"].contains(&span.content.trim()))
                .expect("checkbox beside its own label");
            assert_eq!(
                marker.content.trim(),
                if checked { "[x]" } else { "[ ]" },
                "checked state for {label}"
            );
        }
    }

    #[test]
    fn checked_answers_stay_with_option_ids_while_focus_moves_and_toggles() {
        for described in [false, true] {
            let mut app = interaction_app(vec![tool("AskUserQuestion")]);
            let _response = attach_question(&mut app, "target", prompt(true, described));
            let _warm = interaction_card(&mut app, "target", 100);
            for key in [
                KeyCode::Char(' '),
                KeyCode::Right,
                KeyCode::Right,
                KeyCode::Char(' '),
                KeyCode::Left,
            ] {
                press(&mut app, key);
            }
            let rows = interaction_card(&mut app, "target", 100);
            assert_focused_option(&rows, LABELS, 1);
            assert_checks(&rows, [true, false, true]);
            press(&mut app, KeyCode::Left);
            let rows = interaction_card(&mut app, "target", 100);
            assert_focused_option(&rows, LABELS, 0);
            assert_checks(&rows, [true, false, true]);
            press(&mut app, KeyCode::Right);
            let _warm = interaction_card(&mut app, "target", 100);
            press(&mut app, KeyCode::Char(' '));
            let rows = interaction_card(&mut app, "target", 100);
            assert_focused_option(&rows, LABELS, 1);
            assert_checks(&rows, [true, true, true]);
        }
    }
}
