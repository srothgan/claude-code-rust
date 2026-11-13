// SPDX-License-Identifier: Apache-2.0

use crate::app::BtwRequestState;
use crate::app::{App, ComposerBlockReason, FocusOwner};
use crate::ui::message_layout::MessageLayout;
use crate::ui::{autocomplete, theme};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const MAX_PENDING_MESSAGE_PREVIEW_ROWS: usize = 3;

pub(crate) fn reading_hint_rows(app: &App, width: u16) -> Vec<Line<'static>> {
    use crate::app::keymap::{AppAction, KeyAction, KeyContext};
    let bindings = app.keymap.help_bindings_for_context(KeyContext::ChatInput);
    let mut parts = vec!["Reading output".to_owned()];
    for (action, label) in [
        (AppAction::ScrollChatUp, "earlier"),
        (AppAction::ScrollChatDown, "later"),
        (AppAction::FollowChat, "live"),
    ] {
        let keys = bindings
            .iter()
            .filter(|binding| binding.action == KeyAction::App(action))
            .map(|binding| crate::ui::help::format_help_key_spec(&binding.spec))
            .collect::<Vec<_>>()
            .join("/");
        if !keys.is_empty() {
            parts.push(format!("{keys} {label}"));
        }
    }
    crate::ui::wrap::wrap_lines_to_physical_rows(
        &[Line::from(Span::styled(parts.join(" · "), Style::default().fg(theme::DIM)))],
        width,
    )
}

pub(crate) fn build_composer_hint_rows(app: &App) -> Vec<Line<'static>> {
    let mut rows = Vec::new();

    if let Some(hint) = &app.session_runtime.login_hint {
        rows.push(Line::from(Span::styled(
            format!("Authentication required: {} -- {}", hint.method_name, hint.method_description),
            Style::default().fg(ratatui::style::Color::Yellow),
        )));
        rows.push(Line::from(Span::styled(
            "Type /login to authenticate, or run `claude auth login` in another terminal",
            Style::default().fg(theme::DIM),
        )));
    }

    if !app.pending_user_messages.is_empty() {
        let count = app.pending_user_messages.len();
        rows.push(Line::from(Span::styled(
            format!("Queued Messages ({count}) · Esc interrupt & continue"),
            Style::default().fg(theme::DIM),
        )));
        let hidden = count.saturating_sub(MAX_PENDING_MESSAGE_PREVIEW_ROWS);
        for (index, pending) in app
            .pending_user_messages
            .iter()
            .skip(hidden)
            .take(MAX_PENDING_MESSAGE_PREVIEW_ROWS)
            .enumerate()
        {
            rows.push(Line::from(Span::styled(
                format!("  {}. {}", hidden + index + 1, pending.first_line()),
                Style::default().fg(theme::DIM),
            )));
        }
    }

    if autocomplete::is_active(app) {
        rows.extend(autocomplete::composer_hint_rows(app));
    } else if app.input.is_empty()
        && app.focus_owner() == FocusOwner::Input
        && let Some(suggestion) = app.session_runtime.prompt_suggestion.as_deref()
        && !suggestion.trim().is_empty()
    {
        rows.push(Line::from(vec![
            Span::styled("Suggestion: ", Style::default().fg(theme::DIM)),
            Span::styled(
                suggestion.trim().to_owned(),
                Style::default().fg(ratatui::style::Color::White),
            ),
            Span::styled("    Tab to accept", Style::default().fg(theme::DIM)),
        ]));
    }

    rows
}

pub(crate) fn wrapped_composer_hint_rows(app: &App, width: u16) -> Vec<Line<'static>> {
    crate::ui::wrap::wrap_lines_to_physical_rows(&build_composer_hint_rows(app), width)
}

pub(crate) fn build_btw_status_rows(app: &App, width: u16) -> Vec<Line<'static>> {
    let ordered = app.btw.status_items();
    if ordered.is_empty() {
        return Vec::new();
    }

    let detail_count = ordered.len().min(crate::app::BtwRequests::MAX_DETAIL_ROWS);
    let mut rows = Vec::with_capacity(detail_count + usize::from(ordered.len() > detail_count));
    for item in ordered.into_iter().take(detail_count) {
        let (icon, icon_style, body, body_style) = match &item.state {
            BtwRequestState::Active => (
                crate::ui::SpinnerState::for_app(app).icon().to_owned(),
                Style::default().fg(theme::BTW_ACCENT),
                item.question.replace(['\r', '\n'], " "),
                Style::default().fg(theme::DIM),
            ),
            BtwRequestState::Waiting => (
                "·".to_owned(),
                Style::default().fg(theme::DIM),
                item.question.replace(['\r', '\n'], " "),
                Style::default().fg(theme::DIM),
            ),
            BtwRequestState::Failed { reason, .. } => (
                theme::ICON_FAILED.to_owned(),
                Style::default().fg(theme::STATUS_ERROR),
                format!(
                    "{} — {}",
                    item.question.replace(['\r', '\n'], " "),
                    reason.replace(['\r', '\n'], " ")
                ),
                Style::default().fg(theme::STATUS_ERROR),
            ),
        };
        let prefix = format!("{icon} BTW  ");
        let available = usize::from(width).saturating_sub(UnicodeWidthStr::width(prefix.as_str()));
        let preview = fit_status_text(&body, available);
        rows.push(Line::from(vec![
            Span::styled(format!("{icon} "), icon_style),
            Span::styled("BTW  ", Style::default().fg(theme::BTW_ACCENT)),
            Span::styled(preview, body_style),
        ]));
    }

    let hidden = app.btw.len().saturating_sub(detail_count);
    if hidden > 0 {
        let prefix = "· BTW ";
        let available = usize::from(width).saturating_sub(UnicodeWidthStr::width(prefix));
        rows.push(Line::from(vec![
            Span::styled("· ", Style::default().fg(theme::DIM)),
            Span::styled("BTW ", Style::default().fg(theme::BTW_ACCENT)),
            Span::styled(
                fit_status_text(&format!("+{hidden} more pending"), available),
                Style::default().fg(theme::DIM),
            ),
        ]));
    }
    rows.into_iter()
        .map(|row| {
            if row.width() > usize::from(width) {
                // Clip even the fixed prefix when the terminal is narrower than it.
                crate::ui::wrap::wrap_lines_to_physical_rows(&[row], width)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            } else {
                row
            }
        })
        .collect()
}

fn fit_status_text(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let ellipsis = '…';
    let content_width = max_width.saturating_sub(1);
    let mut result = String::new();
    let mut width = 0usize;
    for grapheme in text.graphemes(true) {
        let ch_width = UnicodeWidthStr::width(grapheme);
        if width.saturating_add(ch_width) > content_width {
            break;
        }
        result.push_str(grapheme);
        width = width.saturating_add(ch_width);
    }
    result.push(ellipsis);
    result
}

/// Read-only status rows shown in the composer field in place of the draft.
///
/// Returns `None` for a silent pending command, which keeps the draft visible.
pub(crate) fn blocked_input_lines(
    app: &App,
    reason: ComposerBlockReason,
    width: u16,
) -> Option<Vec<Line<'static>>> {
    let rows = match reason {
        ComposerBlockReason::CommandPending => {
            let label = app.displayed_pending_command_label()?;
            let spinner_ch = crate::ui::SpinnerState::for_app(app).icon();
            vec![Line::from(vec![
                Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
                Span::styled(label.to_owned(), Style::default().fg(theme::DIM)),
            ])]
        }
        ComposerBlockReason::Error => {
            let mut rows = vec![
                Line::from(Span::styled(
                    "Input disabled due to error",
                    Style::default().fg(theme::STATUS_ERROR),
                )),
                Line::from(Span::styled(
                    "Press Ctrl+Q to quit and try again.",
                    Style::default().fg(theme::DIM),
                )),
            ];
            if !app.input.is_empty() {
                rows.push(Line::from(Span::styled(
                    "Unsent draft:",
                    Style::default().fg(theme::DIM),
                )));
                rows.extend(app.input.lines().iter().cloned().map(Line::from));
            }
            crate::ui::wrap::wrap_lines_to_physical_rows(
                &rows,
                MessageLayout::new(width).content_width,
            )
        }
        ComposerBlockReason::Shutdown => vec![Line::from(Span::styled(
            "Shutting down... Press Ctrl+C again to force exit.",
            Style::default().fg(theme::DIM),
        ))],
    };
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::build_btw_status_rows;
    use crate::app::App;

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn composer_btw_priority_associates_each_failure_and_overflow() {
        let mut app = App::test_default();
        for (id, question) in [
            ("old", "OLD_QUESTION"),
            ("new", "NEW_QUESTION"),
            ("active", "ACTIVE_QUESTION"),
            ("waiting", "WAITING_QUESTION"),
        ] {
            app.btw.try_push(id.into(), question.into()).unwrap();
        }
        let now = std::time::Instant::now();
        app.btw.take_next().unwrap();
        assert!(app.btw.fail("old", "OLD_CAUSE".into(), now));
        app.btw.take_next().unwrap();
        assert!(app.btw.fail("new", "NEW_CAUSE".into(), now));
        app.btw.take_next().unwrap();
        let rows = build_btw_status_rows(&app, 90);
        let text = rows.iter().map(line_text).collect::<Vec<_>>();
        let active = text.iter().position(|r| r.contains("ACTIVE_QUESTION")).unwrap();
        let recent = text.iter().position(|r| r.contains("NEW_QUESTION")).unwrap();
        let old = text.iter().position(|r| r.contains("OLD_QUESTION")).unwrap();
        assert!(active < recent && recent < old);
        assert!(text[recent].contains("NEW_CAUSE") && !text[recent].contains("OLD_CAUSE"));
        assert!(text[old].contains("OLD_CAUSE") && !text[old].contains("NEW_CAUSE"));
        assert_ne!(text[active].split("BTW").next(), text[recent].split("BTW").next());
        assert!(text.iter().any(|r| r.contains("+1") && r.contains("pending")));
        assert!(!text.iter().any(|r| r.contains("WAITING_QUESTION")));
        let narrow = build_btw_status_rows(&app, 24);
        assert!(narrow.iter().all(|r| r.width() <= 24));
        assert!(line_text(&narrow[0]).contains("ACTIVE_QUESTION"));
        assert!(narrow.iter().any(|r| line_text(r).contains("+1")));
    }

    #[test]
    fn btw_rows_collapse_multiline_question_into_one_status_row() {
        let mut app = App::test_default();
        app.btw
            .try_push("btw-1".to_owned(), "first line\nsecond  line".to_owned())
            .expect("queue slot");

        let rows = build_btw_status_rows(&app, 80);

        assert_eq!(rows.len(), 1);
        assert!(line_text(&rows[0]).contains("first line second  line"));
    }

    #[test]
    fn composer_btw_clipping_keeps_graphemes_and_failure_identity() {
        for (question, width, expected) in [
            ("界XY", 10, "界…"),
            ("e\u{301}XY", 9, "e\u{301}…"),
            ("👩\u{200d}💻XY", 10, "👩\u{200d}💻…"),
        ] {
            let mut app = App::test_default();
            app.btw.try_push("unicode".into(), question.into()).unwrap();
            let rows = build_btw_status_rows(&app, width);
            let rendered = line_text(&rows[0]);
            assert_eq!(rendered.split_once("BTW  ").unwrap().1, expected);
            assert!(rows[0].width() <= usize::from(width));
            assert_eq!(app.btw.status_items()[0].question, question);
            assert!(line_text(&build_btw_status_rows(&app, 80)[0]).ends_with(question));
        }
        let mut app = App::test_default();
        for (id, body) in [
            ("failed", "FAILED_QUESTION"),
            ("active", "ACTIVE_BODY"),
            ("a", "WAIT_A"),
            ("b", "WAIT_B"),
        ] {
            app.btw.try_push(id.into(), body.into()).unwrap();
        }
        app.btw.take_next().unwrap();
        assert!(app.btw.fail(
            "failed",
            "FIRST_CAUSE\nSECOND_CAUSE".into(),
            std::time::Instant::now()
        ));
        app.btw.take_next().unwrap();
        for width in 0..10 {
            let rows = build_btw_status_rows(&app, width);
            assert!(rows.iter().all(|r| r.width() <= usize::from(width)));
        }
        let rows = build_btw_status_rows(&app, 100);
        let text = rows.iter().map(line_text).collect::<Vec<_>>();
        let failure = text.iter().find(|r| r.contains("FAILED_QUESTION")).unwrap();
        assert!(failure.contains("FIRST_CAUSE SECOND_CAUSE"));
        assert!(!failure.contains('\n'));
        assert!(text.iter().any(|r| r.contains("ACTIVE_BODY")));
        assert!(text.iter().any(|r| r.contains("+1")));
    }
}
