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
        || app.slash.is_visible()
        || app.subagent.as_ref().is_some_and(|s| !s.candidates.is_empty())
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
    if let Some(s) = app.slash.visible() {
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
                lines.push(slash_placeholder_line(s));
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

fn slash_placeholder_line(slash: &slash::SlashState) -> Line<'static> {
    let message = slash.placeholder.as_deref().unwrap_or("Type a command name after /");
    hint_line(message)
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
        spans.push(Span::styled("  | ", Style::default().fg(theme::DIM)));
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
    use super::composer_hint_rows;
    use crate::app::{AUTOCOMPLETE_VISIBLE_ROWS, App, file_index, mention, slash, subagent};

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn completion_file(path: &str) -> file_index::FileCandidate {
        file_index::FileCandidate {
            rel_path: path.into(),
            rel_path_lower: path.to_lowercase(),
            basename_lower: path.rsplit('/').next().unwrap().to_lowercase(),
            depth: path.matches('/').count(),
        }
    }
    fn paint_hints(
        app: &App,
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    ) -> Vec<String> {
        let area = ratatui::layout::Rect::new(3, 2, 110, 10);
        terminal
            .draw(|frame| {
                frame
                    .render_widget(ratatui::widgets::Paragraph::new(composer_hint_rows(app)), area);
            })
            .unwrap();
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .filter(|r| !r.is_empty())
            .collect()
    }
    fn hint_terminal() -> ratatui::Terminal<ratatui::backend::TestBackend> {
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(116, 14)).unwrap()
    }
    fn settle_completion(app: &mut App, ready: impl Fn(&App) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            file_index::drain_events(app);
            if ready(app) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "completion did not settle");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn completion_guidance_tracks_real_search_agent_and_rewind_outcomes() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("visible.rs"), "").unwrap();
        let mut app = App::test_default();
        app.cwd_raw = directory.path().to_string_lossy().into_owned();
        file_index::restart(&mut app);
        settle_completion(&mut app, |a| {
            matches!(a.file_index.status, file_index::FileIndexStatus::Complete)
        });
        let mut terminal = hint_terminal();
        assert!(paint_hints(&app, &mut terminal).is_empty());
        app.input.set_text("@");
        mention::activate(&mut app);
        let hint = paint_hints(&app, &mut terminal);
        assert!(hint.iter().any(|r| r.contains("file") && r.contains('@')));
        assert!(hint.iter().all(|r| !r.contains("> ") && !r.contains("visible.rs")));
        app.input.insert_str("zzzz_nonmatching");
        mention::update_query(&mut app);
        settle_completion(&mut app, |a| {
            a.mention
                .as_ref()
                .and_then(mention::MentionState::placeholder_message)
                .is_some_and(|text| text.starts_with("No matching"))
        });
        let no_match = paint_hints(&app, &mut terminal);
        assert!(no_match.iter().any(|r| r.to_lowercase().contains("no match")));
        assert!(no_match.iter().all(|r| !r.contains("> ") && !r.contains("visible.rs")));
        mention::deactivate(&mut app);
        app.input.clear();
        assert!(paint_hints(&app, &mut terminal).is_empty());
        app.sdk_inventory.available_agents =
            vec![crate::agent::model::AvailableAgent::new("reviewer", "INSPECT")];
        app.input.set_text("&");
        subagent::sync_with_cursor(&mut app);
        let hint = paint_hints(&app, &mut terminal);
        assert!(hint.iter().any(|r| r.contains("subagent") && r.contains('&')));
        assert!(hint.iter().all(|r| !r.contains("> ")));
        app.input.insert_str("rev");
        subagent::sync_with_cursor(&mut app);
        assert!(
            paint_hints(&app, &mut terminal)
                .iter()
                .any(|r| r.contains("> &reviewer") && r.contains("INSPECT"))
        );
        app.input.insert_str(" tail");
        subagent::sync_with_cursor(&mut app);
        assert!(paint_hints(&app, &mut terminal).is_empty());
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        app.input.set_text("/rewind ");
        slash::sync_with_cursor(&mut app);
        assert!(
            matches!(rx.try_recv().unwrap().command,crate::agent::wire::BridgeCommand::GetRewindTargets {session_id} if session_id=="session-1")
        );
        let pending = paint_hints(&app, &mut terminal);
        assert!(pending.iter().any(|r| r.contains("Loading") && r.contains("messages")));
        assert!(pending.iter().all(|r| !r.contains("> ")));
        crate::app::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::RewindTargetsReceived {
                session_id: "session-1".into(),
                targets: vec![crate::agent::model::RewindTarget {
                    uuid: "target-current".into(),
                    first_text: "CURRENT_PROMPT".into(),
                    input_text: "CURRENT_PROMPT".into(),
                    index: 0,
                    previous_assistant_uuid: None,
                    resume_anchor_uuid: None,
                }],
                error: None,
            },
        );
        let ready = paint_hints(&app, &mut terminal);
        assert!(ready.iter().any(|r| r.contains("CURRENT_PROMPT") && r.contains("> ")));
        assert!(ready.iter().all(|r| !r.contains("Loading")));
        slash::confirm_selection(&mut app);
        assert_eq!(app.input.text(), "/rewind target-current ");
    }
    #[test]
    fn short_completion_lists_keep_row_identity_selection_and_clear_stale_cells() {
        let mut app = App::test_default();
        let mut terminal = hint_terminal();
        app.mention = Some(mention::MentionState::new(
            0,
            0,
            "rs".into(),
            vec![completion_file("src/alpha.rs"), completion_file("tests/beta.rs")],
        ));
        for selected in ["src/alpha.rs", "tests/beta.rs"] {
            let rows = paint_hints(&app, &mut terminal);
            assert_eq!(rows.len(), 2);
            for path in ["src/alpha.rs", "tests/beta.rs"] {
                let own = rows.iter().filter(|r| r.contains(path)).collect::<Vec<_>>();
                assert_eq!(own.len(), 1);
                assert_eq!(own[0].contains("> "), path == selected);
                assert!(!own[0].contains(if path == "src/alpha.rs" {
                    "tests/beta.rs"
                } else {
                    "src/alpha.rs"
                }));
            }
            mention::move_down(&mut app);
        }
        app.mention = Some(mention::MentionState::new(
            0,
            0,
            "rs".into(),
            vec![completion_file("only/current.rs")],
        ));
        let rows = paint_hints(&app, &mut terminal);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("> only/current.rs"));
        assert!(!rows[0].contains("alpha") && !rows[0].contains("beta"));
        mention::deactivate(&mut app);
        assert!(paint_hints(&app, &mut terminal).is_empty());
    }
    #[test]
    fn completion_window_follows_navigation_and_confirms_the_displayed_target() {
        let mut app = App::test_default();
        let mut terminal = hint_terminal();
        app.input.set_text("/");
        slash::sync_with_cursor(&mut app);
        let catalog = app
            .slash
            .visible()
            .unwrap()
            .candidates
            .iter()
            .map(|c| c.primary.clone())
            .collect::<Vec<_>>();
        for name in ["/help", "/rewind"] {
            assert!(catalog.iter().any(|c| c == name));
        }
        let control = paint_hints(&app, &mut terminal);
        assert_eq!(control.iter().filter(|r| r.contains("> ")).count(), 1);
        for row in &control {
            assert!(catalog.iter().any(|c| row.contains(c)));
        }
        let names = (0..AUTOCOMPLETE_VISIBLE_ROWS + 3)
            .map(|n| format!("/target-{n:02}"))
            .collect::<Vec<_>>();
        app.slash.visible_mut().unwrap().candidates = names
            .iter()
            .map(|n| slash::SlashCandidate {
                insert_value: n.clone(),
                primary: n.clone(),
                secondary: None,
            })
            .collect();
        let last = names.len() - 2;
        for _ in 0..last {
            slash::move_down(&mut app);
        }
        let rows = paint_hints(&app, &mut terminal);
        let expected = &names[last + 1 - AUTOCOMPLETE_VISIBLE_ROWS..=last];
        assert_eq!(rows.len(), expected.len());
        for (row, name) in rows.iter().zip(expected) {
            assert!(row.contains(name));
            assert_eq!(row.contains("> "), name == &names[last]);
        }
        for hidden in names.iter().filter(|n| !expected.contains(n)) {
            assert!(rows.iter().all(|r| !r.contains(hidden)));
        }
        for _ in 0..last {
            slash::move_up(&mut app);
        }
        let rows = paint_hints(&app, &mut terminal);
        for (row, name) in rows.iter().zip(&names[..AUTOCOMPLETE_VISIBLE_ROWS]) {
            assert!(row.contains(name));
        }
        assert!(rows[0].contains("> "));
        for _ in 0..last {
            slash::move_down(&mut app);
        }
        assert!(
            paint_hints(&app, &mut terminal)
                .iter()
                .any(|r| r.contains(&format!("> {}", names[last])))
        );
        slash::confirm_selection(&mut app);
        assert_eq!(app.input.text(), format!("{} ", names[last]));
        assert!(app.pending_submit.is_none());
    }
    fn assert_candidate_emphasis(
        line: &ratatui::text::Line<'_>,
        text: &str,
        matched: &str,
        selected: bool,
    ) {
        use ratatui::style::Modifier;
        let actual = line_text(line);
        let start = actual.find(text).unwrap();
        let chars = line
            .spans
            .iter()
            .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
            .collect::<Vec<_>>();
        let start = actual[..start].chars().count();
        let expected_match =
            if matched.is_empty() { None } else { Some(text.find(matched).unwrap()) };
        let highlighted = chars[start..start + text.chars().count()]
            .iter()
            .filter(|(_, style)| style.add_modifier.contains(Modifier::BOLD))
            .map(|(c, _)| *c)
            .collect::<String>();
        assert_eq!(highlighted, matched, "{actual}");
        if let Some(byte) = expected_match {
            let mstart = start + text[..byte].chars().count();
            assert!(
                chars[mstart..mstart + matched.chars().count()]
                    .iter()
                    .all(|(_, s)| s.add_modifier.contains(Modifier::BOLD))
            );
        }
        assert_eq!(actual[..actual.find(text).unwrap()].contains("> "), selected);
    }
    #[test]
    fn candidate_rows_preserve_original_text_match_emphasis_and_metadata_association() {
        let mut app = App::test_default();
        app.mention = Some(mention::MentionState::new(
            0,
            0,
            "i".into(),
            vec![completion_file("界/İstanbul.rs"), completion_file("other.rs")],
        ));
        let rows = composer_hint_rows(&app);
        assert_candidate_emphasis(&rows[0], "界/İstanbul.rs", "İ", true);
        assert_candidate_emphasis(&rows[1], "other.rs", "", false);
        app.mention = None;
        for (context, primary, query, matched) in [
            (slash::SlashContext::CommandName, "/İnfo", "i", "İ"),
            (
                slash::SlashContext::Argument {
                    command: "/model".into(),
                    arg_index: 0,
                    token_range: (7, 8),
                },
                "界/İstanbul",
                "i",
                "İ",
            ),
        ] {
            app.input.set_text("/");
            slash::sync_with_cursor(&mut app);
            let state = app.slash.visible_mut().unwrap();
            state.query = query.into();
            state.context = context;
            state.candidates = vec![
                slash::SlashCandidate {
                    insert_value: primary.into(),
                    primary: primary.into(),
                    secondary: Some("OWN_DETAIL".into()),
                },
                slash::SlashCandidate {
                    insert_value: "other".into(),
                    primary: "other".into(),
                    secondary: None,
                },
            ];
            let rows = composer_hint_rows(&app);
            assert_candidate_emphasis(&rows[0], primary, matched, true);
            assert!(line_text(&rows[0]).contains("OWN_DETAIL"));
            assert!(!line_text(&rows[1]).contains("OWN_DETAIL"));
        }
        app.slash.clear();
        let mut agent = crate::agent::model::AvailableAgent::new("inspector", "OWN_DESCRIPTION");
        agent.model = Some("MODEL_MATCH".into());
        app.sdk_inventory.available_agents =
            vec![agent, crate::agent::model::AvailableAgent::new("helper", "OTHER_DESCRIPTION")];
        for (query, matched) in [("in", "in"), ("MODEL_MATCH", "")] {
            app.input.set_text(&format!("&{query}"));
            subagent::sync_with_cursor(&mut app);
            let rows = composer_hint_rows(&app);
            assert_candidate_emphasis(&rows[0], "&inspector", matched, true);
            let own = line_text(&rows[0]);
            assert!(
                own.contains("OWN_DESCRIPTION")
                    && own.contains("MODEL_MATCH")
                    && !own.contains("OTHER_DESCRIPTION")
            );
        }
        app.input.set_text("&helper");
        subagent::sync_with_cursor(&mut app);
        let rows = composer_hint_rows(&app);
        assert_candidate_emphasis(&rows[0], "&helper", "helper", true);
        assert!(line_text(&rows[0]).contains("OTHER_DESCRIPTION"));
        assert!(!line_text(&rows[0]).contains("model:"));
    }
}
