// SPDX-License-Identifier: Apache-2.0

use super::{AUTOCOMPLETE_VISIBLE_ROWS, App, FocusTarget, dialog::DialogState};

const MAX_CANDIDATES: usize = 50;

#[derive(Debug, Clone)]
pub struct SubagentCandidate {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubagentState {
    /// Character position where the `&` token starts.
    pub trigger_row: usize,
    pub trigger_col: usize,
    /// Current query text after `&`.
    pub query: String,
    /// Filtered subagent candidates.
    pub candidates: Vec<SubagentCandidate>,
    /// Shared autocomplete dialog navigation state.
    pub dialog: DialogState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubagentDetection {
    trigger_row: usize,
    trigger_col: usize,
    query: String,
}

fn is_subagent_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn detect_subagent_at_cursor(
    lines: &[String],
    cursor_row: usize,
    cursor_col: usize,
) -> Option<SubagentDetection> {
    let line = lines.get(cursor_row)?;
    let chars: Vec<char> = line.chars().collect();
    if cursor_col > chars.len() {
        return None;
    }

    let mut token_start = cursor_col;
    while token_start > 0 && !chars[token_start - 1].is_whitespace() {
        token_start -= 1;
    }
    if token_start >= chars.len() || chars[token_start] != '&' {
        return None;
    }

    // Reject shell-style logical and operators (`&&`).
    if chars.get(token_start + 1).copied() == Some('&') {
        return None;
    }

    let token_end =
        (token_start + 1..chars.len()).find(|&i| chars[i].is_whitespace()).unwrap_or(chars.len());
    if cursor_col <= token_start || cursor_col > token_end {
        return None;
    }

    // Eager activation: allow a bare `&` only when it is at token end (`... &` with no
    // trailing chars). This avoids triggering on spacing/operator patterns like `& `.
    if token_end == token_start + 1 {
        if token_end == chars.len() {
            return Some(SubagentDetection {
                trigger_row: cursor_row,
                trigger_col: token_start,
                query: String::new(),
            });
        }
        return None;
    }

    // First character after '&' must be alphabetic to start a valid subagent token.
    if !chars[token_start + 1].is_ascii_alphabetic() {
        return None;
    }

    if chars[token_start + 1..token_end].iter().any(|ch| !is_subagent_char(*ch)) {
        return None;
    }
    let query: String = chars[token_start + 1..cursor_col].iter().collect();

    Some(SubagentDetection { trigger_row: cursor_row, trigger_col: token_start, query })
}

fn candidate_matches(candidate: &SubagentCandidate, query_lower: &str) -> bool {
    candidate.name.to_lowercase().contains(query_lower)
        || candidate.description.to_lowercase().contains(query_lower)
        || candidate.model.as_ref().is_some_and(|model| model.to_lowercase().contains(query_lower))
}

fn filter_candidates(
    candidates: &[crate::agent::model::AvailableAgent],
    query: &str,
) -> Vec<SubagentCandidate> {
    let query_lower = query.to_lowercase();
    candidates
        .iter()
        .filter(|agent| !agent.name.trim().is_empty())
        .map(|agent| SubagentCandidate {
            name: agent.name.clone(),
            description: agent.description.clone(),
            model: agent.model.clone(),
        })
        .filter(|candidate| candidate_matches(candidate, &query_lower))
        .take(MAX_CANDIDATES)
        .collect()
}

fn build_subagent_state(app: &App) -> Option<SubagentState> {
    let detection = detect_subagent_at_cursor(
        app.input.lines(),
        app.input.cursor_row(),
        app.input.cursor_col(),
    )?;
    let candidates = filter_candidates(&app.sdk_inventory.available_agents, &detection.query);
    if candidates.is_empty() {
        return None;
    }
    Some(SubagentState {
        trigger_row: detection.trigger_row,
        trigger_col: detection.trigger_col,
        query: detection.query,
        candidates,
        dialog: DialogState::default(),
    })
}

pub fn activate(app: &mut App) {
    let Some(state) = build_subagent_state(app) else {
        return;
    };
    app.subagent = Some(state);
    app.mention = None;
    app.slash.clear();
    app.claim_focus_target(FocusTarget::Mention);
}

pub fn update_query(app: &mut App) {
    let Some(next_state) = build_subagent_state(app) else {
        deactivate(app);
        return;
    };

    if let Some(ref mut subagent) = app.subagent {
        subagent.trigger_row = next_state.trigger_row;
        subagent.trigger_col = next_state.trigger_col;
        subagent.query = next_state.query;
        subagent.candidates = next_state.candidates;
        subagent.dialog.clamp(subagent.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    } else {
        app.subagent = Some(next_state);
        app.claim_focus_target(FocusTarget::Mention);
    }
}

pub fn sync_with_cursor(app: &mut App) {
    match (build_subagent_state(app), app.subagent.is_some()) {
        (Some(_), true) => update_query(app),
        (Some(_), false) => activate(app),
        (None, true) => deactivate(app),
        (None, false) => {}
    }
}

pub fn deactivate(app: &mut App) {
    app.subagent = None;
    if app.mention.is_none() && app.slash.visible().is_none() {
        app.release_focus_target(FocusTarget::Mention);
    }
}

pub fn move_up(app: &mut App) {
    if let Some(ref mut subagent) = app.subagent {
        subagent.dialog.move_up(subagent.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    }
}

pub fn move_down(app: &mut App) {
    if let Some(ref mut subagent) = app.subagent {
        subagent.dialog.move_down(subagent.candidates.len(), AUTOCOMPLETE_VISIBLE_ROWS);
    }
}

pub fn confirm_selection(app: &mut App) {
    let Some(subagent) = app.subagent.take() else {
        return;
    };

    let Some(candidate) = subagent.candidates.get(subagent.dialog.selected) else {
        if app.mention.is_none() && app.slash.visible().is_none() {
            app.release_focus_target(FocusTarget::Mention);
        }
        return;
    };

    let mut lines = app.input.lines().to_vec();
    let Some(line) = lines.get(subagent.trigger_row) else {
        if app.mention.is_none() && app.slash.visible().is_none() {
            app.release_focus_target(FocusTarget::Mention);
        }
        return;
    };

    let chars: Vec<char> = line.chars().collect();
    if subagent.trigger_col >= chars.len() || chars[subagent.trigger_col] != '&' {
        if app.mention.is_none() && app.slash.visible().is_none() {
            app.release_focus_target(FocusTarget::Mention);
        }
        return;
    }

    let token_end = (subagent.trigger_col + 1..chars.len())
        .find(|&i| chars[i].is_whitespace())
        .unwrap_or(chars.len());
    let before: String = chars[..subagent.trigger_col].iter().collect();
    let after: String = chars[token_end..].iter().collect();
    let replacement = if after.is_empty() {
        format!("&{} ", candidate.name)
    } else {
        format!("&{}", candidate.name)
    };
    let new_line = format!("{before}{replacement}{after}");
    let new_cursor_col = subagent.trigger_col + replacement.chars().count();
    let new_line_len = new_line.chars().count();
    lines[subagent.trigger_row] = new_line;
    app.input.replace_lines_and_cursor(
        lines,
        subagent.trigger_row,
        new_cursor_col.min(new_line_len),
    );

    sync_with_cursor(app);
    if app.mention.is_none() && app.slash.visible().is_none() && app.subagent.is_none() {
        app.release_focus_target(FocusTarget::Mention);
    }
}

pub fn find_subagent_spans(text: &str) -> Vec<(usize, usize, String)> {
    let mut spans = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] != '&' {
            i += 1;
            continue;
        }
        if i > 0 && !chars[i - 1].is_whitespace() {
            i += 1;
            continue;
        }
        if i + 1 >= chars.len() || !chars[i + 1].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        if chars[i + 1] == '&' {
            i += 1;
            continue;
        }

        let mut end = i + 1;
        while end < chars.len() && !chars[end].is_whitespace() {
            if !is_subagent_char(chars[end]) {
                break;
            }
            end += 1;
        }
        if end <= i + 1 {
            i += 1;
            continue;
        }
        if end < chars.len() && !chars[end].is_whitespace() {
            i = end + 1;
            continue;
        }

        let byte_start: usize = chars[..i].iter().map(|c| c.len_utf8()).sum();
        let byte_end: usize = chars[..end].iter().map(|c| c.len_utf8()).sum();
        let name: String = chars[i + 1..end].iter().collect();
        spans.push((byte_start, byte_end, name));
        i = end;
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    #[test]
    fn subagent_detection_respects_token_grammar_and_character_coordinates() {
        for (line, cursor, expected) in [
            ("界 &review_2-x tail", 8, Some((2, "revie"))),
            ("界 &review_2-x", 13, Some((2, "review_2-x"))),
            ("界 &", 3, Some((2, ""))),
            ("&review", 1, Some((0, ""))),
            ("&review tail", 4, Some((0, "rev"))),
            ("&9bad", 1, None),
            ("&9bad", 5, None),
            ("&-bad", 1, None),
            ("&bad!", 1, None),
            ("&bad!", 5, None),
            ("&éclair", 1, None),
            ("& review", 1, None),
            ("cmd && build", 5, None),
            ("cmd && build", 6, None),
            ("x&review", 8, None),
            ("&review", 0, None),
            ("&review", 8, None),
        ] {
            let lines = vec!["untouched".into(), line.into()];
            let result = detect_subagent_at_cursor(&lines, 1, cursor);
            assert_eq!(
                result,
                expected.map(|(trigger_col, query)| SubagentDetection {
                    trigger_row: 1,
                    trigger_col,
                    query: query.into(),
                }),
                "{line:?}, cursor {cursor}"
            );
        }
    }

    fn advertised_agents() -> Vec<crate::agent::model::AvailableAgent> {
        let mut model_agent = crate::agent::model::AvailableAgent::new("builder", "Compile");
        model_agent.model = Some("SpecialModel".into());
        vec![
            crate::agent::model::AvailableAgent::new("reviewer", "Inspect changes"),
            crate::agent::model::AvailableAgent::new("explorer", "FindNeedle"),
            model_agent,
            crate::agent::model::AvailableAgent::new("", "FindNeedle"),
        ]
    }

    #[test]
    fn subagent_matching_selects_advertised_identity_and_preserves_surrounding_draft() {
        for (query, name, description, model) in [
            ("REVIEW", "reviewer", "Inspect changes", None),
            ("findneedle", "explorer", "FindNeedle", None),
            ("SPECIALMODEL", "builder", "Compile", Some("SpecialModel")),
        ] {
            let mut app = App::test_default();
            app.sdk_inventory.available_agents = advertised_agents();
            app.input.set_text(&format!("FIRST\n界 &{query} preserve suffix\nLAST"));
            app.input.set_cursor(1, 3 + query.len());
            sync_with_cursor(&mut app);
            let state = app.subagent.as_ref().unwrap();
            assert_eq!(
                state
                    .candidates
                    .iter()
                    .map(|c| (c.name.as_str(), c.description.as_str(), c.model.as_deref()))
                    .collect::<Vec<_>>(),
                [(name, description, model)]
            );
            confirm_selection(&mut app);
            assert_eq!(app.input.text(), format!("FIRST\n界 &{name} preserve suffix\nLAST"));
            app.input.textarea_insert_char('!');
            assert_eq!(app.input.text(), format!("FIRST\n界 &{name}! preserve suffix\nLAST"));
        }
        let mut app = App::test_default();
        app.sdk_inventory.available_agents = advertised_agents();
        app.input.set_text("&");
        sync_with_cursor(&mut app);
        assert_eq!(
            app.subagent
                .as_ref()
                .unwrap()
                .candidates
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["reviewer", "explorer", "builder"]
        );
        app.input.insert_str("unmatched");
        sync_with_cursor(&mut app);
        assert!(app.subagent.is_none());
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        app.input.set_text("&review");
        sync_with_cursor(&mut app);
        confirm_selection(&mut app);
        assert_eq!(app.input.text(), "&reviewer ");
        for text in ["& ", "&review"] {
            app.sdk_inventory.available_agents.clear();
            app.input.set_text(text);
            sync_with_cursor(&mut app);
            assert!(app.subagent.is_none());
        }
    }

    #[test]
    fn subagent_highlight_spans_preserve_byte_ranges_and_reject_malformed_neighbors() {
        let text = "界 &alpha_2-x && &beta x&embed &9bad &bad! & &éclair";
        assert_eq!(
            find_subagent_spans(text),
            [(4, 14, "alpha_2-x".into()), (18, 23, "beta".into())]
        );
        assert_eq!(&text[4..14], "&alpha_2-x");
        assert_eq!(&text[18..23], "&beta");
    }

    fn agent_key(app: &mut App, code: crossterm::event::KeyCode) {
        crate::app::events::handle_terminal_event(
            app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                code,
                crossterm::event::KeyModifiers::NONE,
            )),
        );
    }
    fn publish_agents(app: &mut App, agents: Vec<crate::agent::model::AvailableAgent>) {
        crate::app::events::handle_client_event(
            app,
            crate::agent::events::ClientEvent::SessionUpdate {
                session_id: "completion-session".into(),
                update: crate::agent::model::SessionUpdate::AvailableAgentsUpdate(
                    crate::agent::model::AvailableAgentsUpdate::new(agents),
                ),
            },
        );
    }
    fn agent_menu(app: &App) -> Vec<String> {
        use ratatui::{
            buffer::Buffer,
            layout::Rect,
            widgets::{Paragraph, Widget},
        };
        let area = Rect::new(3, 2, 100, 10);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 106, 14));
        Paragraph::new(crate::ui::input_rows::build_composer_hint_rows(app))
            .render(area, &mut buffer);
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn advertised_agent_refresh_updates_visible_selection_and_keyboard_completion() {
        use crate::agent::model::AvailableAgent;
        use crossterm::event::KeyCode;
        let mut app = App::test_default();
        app.status = crate::app::AppStatus::Ready;
        app.session_runtime.session_id =
            Some(crate::agent::model::SessionId::new("completion-session"));
        publish_agents(
            &mut app,
            vec![
                AvailableAgent::new("review-a", "OLD_A"),
                AvailableAgent::new("review-b", "OLD_B"),
                AvailableAgent::new("review-c", "OLD_C"),
            ],
        );
        app.input.set_text("FIRST\n界 &re suffix\nLAST");
        app.input.set_cursor(1, 2);
        for _ in 0..3 {
            agent_key(&mut app, KeyCode::Right);
        }
        agent_key(&mut app, KeyCode::Down);
        agent_key(&mut app, KeyCode::Down);
        assert!(agent_menu(&app).iter().any(|r| r.contains("> &review-c")));
        let mut updated = AvailableAgent::new("review-b", "NEW_B");
        updated.model = Some("MODEL_B".into());
        publish_agents(&mut app, vec![updated, AvailableAgent::new("review-d", "NEW_D")]);
        let rows = agent_menu(&app);
        assert_eq!(rows.len(), 2);
        let b = rows.iter().find(|r| r.contains("&review-b")).unwrap();
        assert!(b.contains("NEW_B") && b.contains("MODEL_B") && !b.contains("NEW_D"));
        let d = rows.iter().find(|r| r.contains("&review-d")).unwrap();
        assert!(d.contains("NEW_D") && !d.contains("MODEL_B"));
        assert!(
            rows.iter().all(|r| !r.contains("OLD_")
                && !r.contains("&review-a")
                && !r.contains("&review-c"))
        );
        let selected = rows.iter().filter(|r| r.contains("> ")).collect::<Vec<_>>();
        assert_eq!(selected.len(), 1);
        let expected = if selected[0].contains("&review-b") {
            "review-b"
        } else {
            assert!(selected[0].contains("&review-d"));
            "review-d"
        };
        agent_key(&mut app, KeyCode::Enter);
        assert_eq!(app.input.text(), format!("FIRST\n界 &{expected} suffix\nLAST"));
        assert!(app.pending_submit.is_none());

        let mut app = App::test_default();
        app.status = crate::app::AppStatus::Ready;
        app.session_runtime.session_id =
            Some(crate::agent::model::SessionId::new("completion-session"));
        publish_agents(&mut app, vec![AvailableAgent::new("review-old", "OLD")]);
        app.input.set_text("&re");
        sync_with_cursor(&mut app);
        assert!(!agent_menu(&app).is_empty());
        publish_agents(&mut app, vec![AvailableAgent::new("builder", "BUILD_ONLY")]);
        assert!(agent_menu(&app).is_empty());
        assert!(app.subagent.is_none());
        assert_eq!(app.focus_owner(), crate::app::FocusOwner::Input);
        agent_key(&mut app, KeyCode::Tab);
        agent_key(&mut app, KeyCode::Char('x'));
        assert!(app.input.text().ends_with('x'));
        assert!(!app.input.text().contains("review-old"));
        app.input.set_text("ordinary draft");
        publish_agents(&mut app, vec![AvailableAgent::new("builder", "LATEST")]);
        assert!(agent_menu(&app).is_empty());
        assert_eq!(app.input.text(), "ordinary draft");
        app.input.set_text("&bui");
        sync_with_cursor(&mut app);
        assert!(agent_menu(&app).iter().any(|r| r.contains("&builder") && r.contains("LATEST")));
        agent_key(&mut app, KeyCode::Enter);
        assert_eq!(app.input.text(), "&builder ");
    }
}
