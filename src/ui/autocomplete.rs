// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{AUTOCOMPLETE_VISIBLE_ROWS, App};
use crate::app::{file_index, mention, slash, subagent};
use crate::ui::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

enum Dropdown<'a> {
    Mention(&'a mention::MentionState),
    Slash(&'a slash::SlashState),
    Subagent(&'a subagent::SubagentState),
}

struct DropdownMeta {
    visible_count: usize,
    start: usize,
    end: usize,
}

pub fn is_active(app: &App) -> bool {
    app.mention.is_some()
        || app.slash.is_some()
        || app.subagent.as_ref().is_some_and(|s| !s.candidates.is_empty())
}

pub fn composer_hint_height(app: &App) -> u16 {
    u16::try_from(composer_hint_rows(app).len()).unwrap_or(u16::MAX)
}

pub fn composer_hint_rows(app: &App) -> Vec<Line<'static>> {
    let Some(dropdown) = active_dropdown(app) else {
        return Vec::new();
    };

    let meta = dropdown_meta(&dropdown);
    with_left_rail(dropdown_lines(&dropdown, &meta))
}

fn active_dropdown(app: &App) -> Option<Dropdown<'_>> {
    if let Some(m) = &app.mention {
        return Some(Dropdown::Mention(m));
    }
    if let Some(s) = &app.slash {
        return Some(Dropdown::Slash(s));
    }
    if let Some(s) = &app.subagent
        && !s.candidates.is_empty()
    {
        return Some(Dropdown::Subagent(s));
    }
    None
}

fn dropdown_meta(dropdown: &Dropdown<'_>) -> DropdownMeta {
    match dropdown {
        Dropdown::Mention(m) => {
            let visible_count = m.candidates.len().clamp(1, AUTOCOMPLETE_VISIBLE_ROWS);
            let (start, end) = if m.candidates.is_empty() {
                (0, 0)
            } else {
                m.dialog.visible_range(m.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS)
            };
            DropdownMeta { visible_count, start, end }
        }
        Dropdown::Slash(s) => {
            let visible_count = if s.candidates.is_empty() {
                1
            } else {
                s.candidates.len().min(AUTOCOMPLETE_VISIBLE_ROWS)
            };
            let (start, end) = if s.candidates.is_empty() {
                (0, 0)
            } else {
                s.dialog.visible_range(s.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS)
            };
            DropdownMeta { visible_count, start, end }
        }
        Dropdown::Subagent(s) => {
            let visible_count = if s.query.is_empty() {
                1
            } else {
                s.candidates.len().min(AUTOCOMPLETE_VISIBLE_ROWS)
            };
            let (start, end) = if s.query.is_empty() {
                (0, 0)
            } else {
                s.dialog.visible_range(s.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS)
            };
            DropdownMeta { visible_count, start, end }
        }
    }
}

fn dropdown_lines(dropdown: &Dropdown<'_>, meta: &DropdownMeta) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(meta.visible_count);
    match dropdown {
        Dropdown::Mention(m) => {
            if m.candidates.is_empty() {
                lines.push(mention_placeholder_line(m));
            } else {
                for (i, candidate) in m.candidates[meta.start..meta.end].iter().enumerate() {
                    lines.push(mention_candidate_line(m, candidate, meta.start + i));
                }
            }
        }
        Dropdown::Slash(s) => {
            if s.candidates.is_empty() {
                lines.push(hint_line("Type a command name after /"));
            } else {
                for (i, candidate) in s.candidates[meta.start..meta.end].iter().enumerate() {
                    lines.push(slash_candidate_line(s, candidate, meta.start + i));
                }
            }
        }
        Dropdown::Subagent(s) => {
            if s.query.is_empty() {
                lines.push(hint_line("Type a subagent name after &"));
            } else {
                for (i, candidate) in s.candidates[meta.start..meta.end].iter().enumerate() {
                    lines.push(subagent_candidate_line(s, candidate, meta.start + i));
                }
            }
        }
    }
    lines
}

fn mention_placeholder_line(mention: &mention::MentionState) -> Line<'static> {
    let message = mention.placeholder_message().unwrap_or_default();
    hint_line(&message)
}

fn hint_line(message: &str) -> Line<'static> {
    Line::from(Span::styled(message.to_owned(), Style::default().fg(theme::DIM)))
}

fn mention_candidate_line(
    mention: &mention::MentionState,
    candidate: &file_index::FileCandidate,
    global_idx: usize,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    push_selection_prefix(&mut spans, global_idx == mention.dialog.selected);

    let path = &candidate.rel_path;
    let query = &mention.query;
    if query.is_empty() {
        spans.push(autocomplete_text(path.clone()));
    } else if let Some((match_start, match_end)) = find_case_insensitive_range(path, query) {
        push_highlighted_text(&mut spans, path, match_start, match_end);
    } else {
        spans.push(autocomplete_text(path.clone()));
    }

    Line::from(spans)
}

fn slash_candidate_line(
    slash: &slash::SlashState,
    candidate: &slash::SlashCandidate,
    global_idx: usize,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    push_selection_prefix(&mut spans, global_idx == slash.dialog.selected);

    if slash.query.is_empty() {
        spans.push(autocomplete_text(candidate.primary.clone()));
    } else if matches!(slash.context, slash::SlashContext::CommandName) {
        let command_name = &candidate.primary;
        let command_body = command_name.strip_prefix('/').unwrap_or(command_name);
        if let Some((match_start, match_end)) =
            find_case_insensitive_range(command_body, &slash.query)
        {
            let prefix_len = command_name.len().saturating_sub(command_body.len());
            let start_idx = prefix_len + match_start;
            let end_idx = prefix_len + match_end;
            push_highlighted_text(&mut spans, command_name, start_idx, end_idx);
        } else {
            spans.push(autocomplete_text(command_name.clone()));
        }
    } else if let Some((match_start, match_end)) =
        find_case_insensitive_range(&candidate.primary, &slash.query)
    {
        push_highlighted_text(&mut spans, &candidate.primary, match_start, match_end);
    } else {
        spans.push(autocomplete_text(candidate.primary.clone()));
    }

    if let Some(secondary) = &candidate.secondary {
        spans.push(Span::styled("  ", Style::default().fg(theme::DIM)));
        spans.push(Span::styled(secondary.clone(), Style::default().fg(theme::DIM)));
    }

    Line::from(spans)
}

fn subagent_candidate_line(
    subagent: &subagent::SubagentState,
    candidate: &subagent::SubagentCandidate,
    global_idx: usize,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    push_selection_prefix(&mut spans, global_idx == subagent.dialog.selected);

    let primary = format!("&{}", candidate.name);
    if subagent.query.is_empty() {
        spans.push(autocomplete_text(primary));
    } else if let Some((match_start, match_end)) =
        find_case_insensitive_range(&candidate.name, &subagent.query)
    {
        push_highlighted_text(&mut spans, &primary, match_start + 1, match_end + 1);
    } else {
        spans.push(autocomplete_text(primary));
    }

    let secondary = match (&candidate.description, &candidate.model) {
        (desc, Some(model)) if !desc.trim().is_empty() => Some(format!("{desc} | model: {model}")),
        (desc, None) if !desc.trim().is_empty() => Some(desc.clone()),
        (_, Some(model)) => Some(format!("model: {model}")),
        _ => None,
    };
    if let Some(secondary) = secondary {
        spans.push(Span::styled("  ", Style::default().fg(theme::DIM)));
        spans.push(Span::styled(secondary, Style::default().fg(theme::DIM)));
    }

    Line::from(spans)
}

fn push_selection_prefix(spans: &mut Vec<Span<'static>>, is_selected: bool) {
    if is_selected {
        spans.push(Span::styled(
            "> ",
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::raw("  "));
    }
}

fn autocomplete_text(content: String) -> Span<'static> {
    Span::styled(content, Style::default().fg(theme::DIM))
}

fn with_left_rail(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let line_count = lines.len();
    let pipe_style = Style::default().fg(theme::DIM);

    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let prefix = if line_count == 1 {
                "  [ "
            } else if i + 1 == line_count {
                "  \u{2514}\u{2500} "
            } else if i == 0 {
                "  \u{250c}\u{2500} "
            } else {
                "  \u{2502}  "
            };
            let mut spans = vec![Span::styled(prefix.to_owned(), pipe_style)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

#[derive(Clone, Copy)]
struct FoldSegment {
    fold_start: usize,
    fold_end: usize,
    orig_start: usize,
    orig_end: usize,
}

fn find_case_insensitive_range(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() || haystack.is_empty() {
        return None;
    }

    let folded_needle = needle.to_lowercase();
    if folded_needle.is_empty() {
        return None;
    }

    let mut folded_haystack = String::new();
    let mut segments: Vec<FoldSegment> = Vec::with_capacity(haystack.chars().count());
    for (orig_start, ch) in haystack.char_indices() {
        let orig_end = orig_start + ch.len_utf8();
        let fold_start = folded_haystack.len();
        for lower_ch in ch.to_lowercase() {
            folded_haystack.push(lower_ch);
        }
        let fold_end = folded_haystack.len();
        segments.push(FoldSegment { fold_start, fold_end, orig_start, orig_end });
    }

    let folded_match_start = folded_haystack.find(&folded_needle)?;
    let folded_match_end = folded_match_start + folded_needle.len();
    let start_seg = segments
        .iter()
        .find(|seg| seg.fold_start <= folded_match_start && folded_match_start < seg.fold_end)?;
    let end_probe = folded_match_end.saturating_sub(1);
    let end_seg = segments
        .iter()
        .find(|seg| seg.fold_start <= end_probe && end_probe < seg.fold_end)
        .unwrap_or(start_seg);

    Some((start_seg.orig_start, end_seg.orig_end))
}

fn push_highlighted_text(
    spans: &mut Vec<Span<'static>>,
    text: &str,
    match_start: usize,
    match_end: usize,
) {
    let before = &text[..match_start];
    let matched = &text[match_start..match_end];
    let after = &text[match_end..];

    if !before.is_empty() {
        spans.push(autocomplete_text(before.to_owned()));
    }
    spans.push(Span::styled(
        matched.to_owned(),
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD),
    ));
    if !after.is_empty() {
        spans.push(autocomplete_text(after.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::{composer_hint_height, composer_hint_rows, find_case_insensitive_range, is_active};
    use crate::app::{AUTOCOMPLETE_VISIBLE_ROWS, App, file_index, mention, slash, subagent};
    use crate::ui::theme;
    use std::time::SystemTime;

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn case_insensitive_range_respects_utf8_boundaries() {
        let haystack = "\u{0130}stanbul";
        let (start, end) =
            find_case_insensitive_range(haystack, "i").expect("case-insensitive match");
        assert!(haystack.is_char_boundary(start));
        assert!(haystack.is_char_boundary(end));
        assert_eq!(&haystack[start..end], "\u{0130}");
    }

    #[test]
    fn empty_mention_renders_single_placeholder_row() {
        let mut app = App::test_default();
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        mention::activate(&mut app);

        let rows = composer_hint_rows(&app);

        assert!(is_active(&app));
        assert_eq!(composer_hint_height(&app), 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(line_text(&rows[0]), "  [ Type a file or folder name after @");
    }

    #[test]
    fn bare_slash_renders_command_candidates() {
        let mut app = App::test_default();
        app.input.set_text("/");
        let _ = app.input.set_cursor(0, 1);
        slash::sync_with_cursor(&mut app);

        let rows = composer_hint_rows(&app);

        assert_eq!(composer_hint_height(&app), 5);
        assert_eq!(rows.len(), 5);
        assert!(line_text(&rows[0]).contains("/1m-context"));
    }

    #[test]
    fn bare_subagent_trigger_renders_single_placeholder_row() {
        let mut app = App::test_default();
        app.available_agents =
            vec![crate::agent::model::AvailableAgent::new("reviewer", "Review code")];
        app.input.set_text("&");
        let _ = app.input.set_cursor(0, 1);
        subagent::sync_with_cursor(&mut app);

        let rows = composer_hint_rows(&app);

        assert_eq!(composer_hint_height(&app), 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(line_text(&rows[0]), "  [ Type a subagent name after &");
    }

    #[test]
    fn mention_uses_result_count_when_below_max_height() {
        let mut app = App::test_default();
        app.mention = Some(mention::MentionState::new(
            0,
            0,
            "rs".to_owned(),
            vec![
                file_index::FileCandidate {
                    rel_path: "src/main.rs".to_owned(),
                    rel_path_lower: "src/main.rs".to_owned(),
                    basename_lower: "main.rs".to_owned(),
                    depth: 1,
                    modified: SystemTime::UNIX_EPOCH,
                    is_dir: false,
                },
                file_index::FileCandidate {
                    rel_path: "src/lib.rs".to_owned(),
                    rel_path_lower: "src/lib.rs".to_owned(),
                    basename_lower: "lib.rs".to_owned(),
                    depth: 1,
                    modified: SystemTime::UNIX_EPOCH,
                    is_dir: false,
                },
            ],
        ));

        let rows = composer_hint_rows(&app);

        assert_eq!(composer_hint_height(&app), 2);
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).starts_with("  \u{250c}\u{2500} "));
        assert!(line_text(&rows[1]).starts_with("  \u{2514}\u{2500} "));
        assert!(line_text(&rows[0]).contains("src/main.rs"));
        assert!(line_text(&rows[1]).contains("src/lib.rs"));
    }

    #[test]
    fn slash_rows_use_shared_five_row_max_window() {
        let mut app = App::test_default();
        app.input.set_text("/");
        let _ = app.input.set_cursor(0, 1);
        slash::sync_with_cursor(&mut app);
        let state = app.slash.as_mut().expect("slash autocomplete");
        state.query = "cmd".to_owned();
        state.candidates = (0..8)
            .map(|i| slash::SlashCandidate {
                insert_value: format!("/cmd{i}"),
                primary: format!("/cmd{i}"),
                secondary: None,
            })
            .collect();
        state.dialog.selected = 6;
        state.dialog.scroll_offset = 2;

        let rows = composer_hint_rows(&app);

        assert_eq!(rows.len(), AUTOCOMPLETE_VISIBLE_ROWS);
        assert!(line_text(&rows[0]).starts_with("  \u{250c}\u{2500} "));
        assert!(line_text(&rows[1]).starts_with("  \u{2502}  "));
        assert!(line_text(&rows[4]).starts_with("  \u{2514}\u{2500} "));
        assert!(line_text(&rows[0]).contains("/cmd2"));
        assert!(line_text(&rows[4]).contains("/cmd6"));
        assert!(line_text(&rows[4]).contains("> /cmd6"));
    }

    #[test]
    fn autocomplete_candidate_text_uses_dim_style() {
        let mut app = App::test_default();
        app.mention = Some(mention::MentionState::new(
            0,
            0,
            String::new(),
            vec![file_index::FileCandidate {
                rel_path: "src/main.rs".to_owned(),
                rel_path_lower: "src/main.rs".to_owned(),
                basename_lower: "main.rs".to_owned(),
                depth: 1,
                modified: SystemTime::UNIX_EPOCH,
                is_dir: false,
            }],
        ));

        let rows = composer_hint_rows(&app);

        let path_span = rows[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "src/main.rs")
            .expect("candidate path span");
        assert_eq!(path_span.style.fg, Some(theme::DIM));
    }
}
