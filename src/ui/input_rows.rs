// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::{App, AppStatus, FocusOwner};
use crate::ui::{autocomplete, theme};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

const SPINNER_FRAMES: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280F}',
];

pub(crate) fn build_composer_hint_rows(app: &App) -> Vec<Line<'static>> {
    let mut rows = Vec::new();

    if let Some(hint) = &app.login_hint {
        rows.push(Line::from(Span::styled(
            format!("Authentication required: {} -- {}", hint.method_name, hint.method_description),
            Style::default().fg(ratatui::style::Color::Yellow),
        )));
        rows.push(Line::from(Span::styled(
            "Type /login to authenticate, or run `claude auth login` in another terminal",
            Style::default().fg(theme::DIM),
        )));
    }

    if app.pending_cancel_origin.is_some() {
        let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
        rows.push(Line::from(vec![
            Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
            Span::styled(
                "Cancelling current turn... draft will auto-submit when ready.",
                Style::default().fg(theme::DIM),
            ),
        ]));
    }

    if autocomplete::is_active(app) {
        rows.extend(autocomplete::composer_hint_rows(app));
    } else if app.input.is_empty()
        && app.focus_owner() == FocusOwner::Input
        && let Some(suggestion) = app.prompt_suggestion.as_deref()
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

pub(crate) fn blocked_input_lines(app: &App) -> Vec<Line<'static>> {
    match app.status {
        AppStatus::Connecting => {
            let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
            vec![Line::from(vec![
                Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
                Span::styled("Connecting to Claude Code...", Style::default().fg(theme::DIM)),
            ])]
        }
        AppStatus::CommandPending => {
            let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
            let label = app.pending_command_label.as_deref().unwrap_or("Processing command...");
            vec![Line::from(vec![
                Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
                Span::styled(label.to_owned(), Style::default().fg(theme::DIM)),
            ])]
        }
        AppStatus::Error => vec![
            Line::from(Span::styled(
                "Input disabled due to error",
                Style::default().fg(theme::STATUS_ERROR),
            )),
            Line::from(Span::styled(
                "Press Ctrl+Q to quit and try again.",
                Style::default().fg(theme::DIM),
            )),
        ],
        AppStatus::Ready | AppStatus::Thinking | AppStatus::Running => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{blocked_input_lines, build_composer_hint_rows};
    use crate::app::{App, AppStatus, CancelOrigin, FocusTarget, LoginHint};

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn build_composer_hint_rows_preserves_login_hint_content() {
        let mut app = App::test_default();
        app.login_hint = Some(LoginHint {
            method_name: "oauth".to_owned(),
            method_description: "Sign in".to_owned(),
        });

        let rows = build_composer_hint_rows(&app);
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Authentication required: oauth -- Sign in"));
    }

    #[test]
    fn build_composer_hint_rows_preserves_cancel_and_suggestion_rows() {
        let mut app = App::test_default();
        app.pending_cancel_origin = Some(CancelOrigin::AutoQueue);
        app.prompt_suggestion = Some("Write tests".to_owned());

        let rows = build_composer_hint_rows(&app);
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Cancelling current turn"));
        assert!(line_text(&rows[1]).contains("Suggestion: Write tests"));
    }

    #[test]
    fn build_composer_hint_rows_prefers_autocomplete_over_prompt_suggestion() {
        let mut app = App::test_default();
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        app.prompt_suggestion = Some("Write tests".to_owned());
        crate::app::mention::activate(&mut app);

        let rows = build_composer_hint_rows(&app);

        assert_eq!(rows.len(), 1);
        assert!(line_text(&rows[0]).contains("Type a file or folder name after @"));
        assert!(!rows.iter().any(|row| line_text(row).contains("Suggestion:")));
    }

    #[test]
    fn prompt_suggestion_hint_requires_input_focus() {
        let mut app = App::test_default();
        app.prompt_suggestion = Some("Write tests".to_owned());
        app.pending_interaction_ids.push("perm-1".to_owned());
        app.claim_focus_target(FocusTarget::Permission);

        let rows = build_composer_hint_rows(&app);
        assert!(rows.is_empty());
    }

    #[test]
    fn blocked_input_lines_shows_connecting_status() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;
        app.spinner_frame = 3;

        let rows = blocked_input_lines(&app);

        assert_eq!(rows.len(), 1);
        assert!(line_text(&rows[0]).contains("Connecting to Claude Code..."));
    }

    #[test]
    fn blocked_input_lines_shows_pending_command_label() {
        let mut app = App::test_default();
        app.status = AppStatus::CommandPending;
        app.pending_command_label = Some("Switching model...".to_owned());

        let rows = blocked_input_lines(&app);

        assert_eq!(rows.len(), 1);
        assert!(line_text(&rows[0]).contains("Switching model..."));
    }

    #[test]
    fn blocked_input_lines_shows_error_rows() {
        let mut app = App::test_default();
        app.status = AppStatus::Error;

        let rows = blocked_input_lines(&app);

        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Input disabled due to error"));
        assert!(line_text(&rows[1]).contains("Press Ctrl+Q to quit and try again."));
    }
}
