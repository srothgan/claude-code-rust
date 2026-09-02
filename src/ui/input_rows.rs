// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::app::{App, ComposerBlockReason, FocusOwner};
use crate::ui::{autocomplete, theme};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

const SPINNER_FRAMES: &[char] = &[
    '\u{280B}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}', '\u{2827}',
    '\u{2807}', '\u{280F}',
];
const MAX_PENDING_MESSAGE_PREVIEW_ROWS: usize = 3;

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

    if app.turn.cancel_requested {
        let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
        rows.push(Line::from(vec![
            Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
            Span::styled("Cancelling current turn...", Style::default().fg(theme::DIM)),
        ]));
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

pub(crate) fn blocked_input_lines(app: &App, reason: ComposerBlockReason) -> Vec<Line<'static>> {
    match reason {
        ComposerBlockReason::CommandPending => {
            let spinner_ch = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
            let label =
                app.turn.pending_command_label.as_deref().unwrap_or("Processing command...");
            vec![Line::from(vec![
                Span::styled(format!("{spinner_ch} "), Style::default().fg(theme::DIM)),
                Span::styled(label.to_owned(), Style::default().fg(theme::DIM)),
            ])]
        }
        ComposerBlockReason::Error => vec![
            Line::from(Span::styled(
                "Input disabled due to error",
                Style::default().fg(theme::STATUS_ERROR),
            )),
            Line::from(Span::styled(
                "Press Ctrl+Q to quit and try again.",
                Style::default().fg(theme::DIM),
            )),
        ],
        ComposerBlockReason::Shutdown => vec![Line::from(Span::styled(
            "Shutting down... Press Ctrl+C again to force exit.",
            Style::default().fg(theme::DIM),
        ))],
    }
}

#[cfg(test)]
mod tests {
    use super::{blocked_input_lines, build_composer_hint_rows};
    use crate::app::{
        App, AppStatus, ComposerBlockReason, FocusTarget, LoginHint, PendingUserMessage,
    };

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn build_composer_hint_rows_preserves_login_hint_content() {
        let mut app = App::test_default();
        app.session_runtime.login_hint = Some(LoginHint {
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
        app.turn.cancel_requested = true;
        app.session_runtime.prompt_suggestion = Some("Write tests".to_owned());

        let rows = build_composer_hint_rows(&app);
        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Cancelling current turn"));
        assert!(line_text(&rows[1]).contains("Suggestion: Write tests"));
    }

    #[test]
    fn pending_message_rows_show_dimmed_summary_and_preview() {
        let mut app = App::test_default();
        assert!(
            app.pending_user_messages
                .try_push_sending(PendingUserMessage::sending(
                    "one".to_owned(),
                    "first line\nsecond line".to_owned(),
                    Vec::new(),
                ))
                .is_ok()
        );

        let rows = build_composer_hint_rows(&app);

        assert_eq!(rows.len(), 2);
        assert_eq!(line_text(&rows[0]), "Queued Messages (1) · Esc interrupt & continue");
        assert_eq!(line_text(&rows[1]), "  1. first line");
        assert!(
            rows.iter()
                .flat_map(|row| row.spans.iter())
                .all(|span| span.style.fg == Some(crate::ui::theme::DIM))
        );
    }

    #[test]
    fn pending_message_rows_show_only_latest_three_previews_with_stable_numbers() {
        let mut app = App::test_default();
        for index in 1..=5 {
            assert!(
                app.pending_user_messages
                    .try_push_sending(PendingUserMessage::sending(
                        format!("message-{index}"),
                        format!("preview {index}"),
                        Vec::new(),
                    ))
                    .is_ok()
            );
        }

        let rows = build_composer_hint_rows(&app);

        assert_eq!(
            rows.iter().map(line_text).collect::<Vec<_>>(),
            [
                "Queued Messages (5) · Esc interrupt & continue",
                "  3. preview 3",
                "  4. preview 4",
                "  5. preview 5",
            ]
        );
    }

    #[test]
    fn build_composer_hint_rows_omits_compaction_status() {
        let mut app = App::test_default();
        app.turn.compaction.begin();

        let rows = build_composer_hint_rows(&app);

        assert!(rows.is_empty());
    }

    #[test]
    fn build_composer_hint_rows_prefers_autocomplete_over_prompt_suggestion() {
        let mut app = App::test_default();
        app.input.set_text("@");
        let _ = app.input.set_cursor(0, 1);
        app.session_runtime.prompt_suggestion = Some("Write tests".to_owned());
        crate::app::mention::activate(&mut app);

        let rows = build_composer_hint_rows(&app);

        assert_eq!(rows.len(), 1);
        assert!(line_text(&rows[0]).contains("Type a file or folder name after @"));
        assert!(!rows.iter().any(|row| line_text(row).contains("Suggestion:")));
    }

    #[test]
    fn prompt_suggestion_hint_requires_input_focus() {
        let mut app = App::test_default();
        app.session_runtime.prompt_suggestion = Some("Write tests".to_owned());
        app.turn.pending_interaction_ids.push("perm-1".to_owned());
        app.claim_focus_target(FocusTarget::Permission);

        let rows = build_composer_hint_rows(&app);
        assert!(rows.is_empty());
    }

    #[test]
    fn blocked_input_lines_shows_pending_command_label() {
        let mut app = App::test_default();
        app.status = AppStatus::CommandPending;
        app.turn.pending_command_label = Some("Switching model...".to_owned());

        let rows = blocked_input_lines(&app, ComposerBlockReason::CommandPending);

        assert_eq!(rows.len(), 1);
        assert!(line_text(&rows[0]).contains("Switching model..."));
    }

    #[test]
    fn blocked_input_lines_shows_error_rows() {
        let mut app = App::test_default();
        app.status = AppStatus::Error;

        let rows = blocked_input_lines(&app, ComposerBlockReason::Error);

        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Input disabled due to error"));
        assert!(line_text(&rows[1]).contains("Press Ctrl+Q to quit and try again."));
    }

    #[test]
    fn blocked_input_lines_prioritizes_shutdown_status() {
        let mut app = App::test_default();
        app.status = AppStatus::Running;
        app.request_shutdown();

        let rows = blocked_input_lines(&app, ComposerBlockReason::Shutdown);

        assert_eq!(rows.len(), 1);
        assert_eq!(line_text(&rows[0]), "Shutting down... Press Ctrl+C again to force exit.");
    }
}
