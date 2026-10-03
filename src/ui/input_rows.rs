// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::app::BtwRequestState;
use crate::app::{App, ComposerBlockReason, FocusOwner};
use crate::ui::{autocomplete, theme};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
        let spinner_ch = crate::ui::SpinnerState::for_app(app).icon();
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
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width) > content_width {
            break;
        }
        result.push(ch);
        width = width.saturating_add(ch_width);
    }
    result.push(ellipsis);
    result
}

pub(crate) fn blocked_input_lines(
    app: &App,
    reason: ComposerBlockReason,
    width: u16,
) -> Vec<Line<'static>> {
    match reason {
        ComposerBlockReason::CommandPending => {
            let spinner_ch = crate::ui::SpinnerState::for_app(app).icon();
            let label =
                app.turn.pending_command_label.as_deref().unwrap_or("Processing command...");
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
            crate::ui::wrap::wrap_lines_to_physical_rows(&rows, width)
        }
        ComposerBlockReason::Shutdown => vec![Line::from(Span::styled(
            "Shutting down... Press Ctrl+C again to force exit.",
            Style::default().fg(theme::DIM),
        ))],
    }
}

#[cfg(test)]
mod tests {
    use super::{blocked_input_lines, build_btw_status_rows, build_composer_hint_rows};
    use crate::app::{
        App, AppStatus, ComposerBlockReason, FocusTarget, LoginHint, PendingUserMessage,
    };

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn reduced_motion_keeps_cancellation_and_command_activity_icons_static() {
        let mut app = App::test_default();
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "prefersReducedMotion",
            serde_json::json!(true),
        ));
        app.turn.cancel_requested = true;
        app.turn.pending_command_label = Some("Switching mode...".to_owned());
        for frame in [0, 4, 9] {
            app.spinner_frame = frame;
            assert_eq!(
                line_text(&build_composer_hint_rows(&app)[0]),
                "\u{25C6} Cancelling current turn..."
            );
            assert_eq!(
                line_text(&blocked_input_lines(&app, ComposerBlockReason::CommandPending, 80)[0]),
                "\u{25C6} Switching mode..."
            );
        }
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
    fn btw_rows_apply_priority_limit_overflow_and_terminal_width() {
        let mut app = App::test_default();
        for (id, question) in [
            ("old-failure", "old failure"),
            ("new-failure", "new failure"),
            ("active", "active question with a long tail"),
            ("waiting", "waiting question"),
        ] {
            app.btw.try_push(id.to_owned(), question.to_owned()).expect("queue slot");
        }
        let now = std::time::Instant::now();
        app.btw.take_next().expect("dispatch old failure");
        assert!(app.btw.fail("old-failure", "old error".to_owned(), now));
        app.btw.take_next().expect("dispatch new failure");
        assert!(app.btw.fail("new-failure", "new error".to_owned(), now));
        app.btw.take_next().expect("dispatch active");

        let rows = build_btw_status_rows(&app, 24);
        let text = rows.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rows.len(), 4);
        assert!(text[0].contains("active question"));
        assert!(text[1].contains("new failure"));
        assert!(text[2].contains("old failure"));
        assert!(text[3].contains("+1 more pending"));
        assert!(rows.iter().all(|row| row.width() <= 24));
        assert!(
            rows[1].spans.iter().any(|span| span.style.fg == Some(crate::ui::theme::STATUS_ERROR))
        );
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
    fn btw_rows_clip_fixed_prefixes_at_tiny_widths_and_keep_multiline_errors_on_one_row() {
        let mut app = App::test_default();
        for index in 0..4 {
            app.btw.try_push(index.to_string(), "question".to_owned()).expect("queue slot");
        }
        app.btw.take_next().expect("dispatch");
        assert!(app.btw.fail(
            "0",
            "first error\nsecond error".to_owned(),
            std::time::Instant::now()
        ));
        app.btw.take_next().expect("dispatch next");
        for width in 0..10 {
            let rows = build_btw_status_rows(&app, width);
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|row| row.width() <= usize::from(width)));
        }
        let rows = build_btw_status_rows(&app, 120);
        assert!(line_text(&rows[1]).contains("first error second error"));
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

        let rows = blocked_input_lines(&app, ComposerBlockReason::CommandPending, 80);

        assert_eq!(rows.len(), 1);
        assert!(line_text(&rows[0]).contains("Switching model..."));
    }

    #[test]
    fn blocked_input_lines_shows_error_rows() {
        let mut app = App::test_default();
        app.status = AppStatus::Error;

        let rows = blocked_input_lines(&app, ComposerBlockReason::Error, 80);

        assert_eq!(rows.len(), 2);
        assert!(line_text(&rows[0]).contains("Input disabled due to error"));
        assert!(line_text(&rows[1]).contains("Press Ctrl+Q to quit and try again."));
    }

    #[test]
    fn blocked_input_lines_prioritizes_shutdown_status() {
        let mut app = App::test_default();
        app.status = AppStatus::Running;
        app.request_shutdown();

        let rows = blocked_input_lines(&app, ComposerBlockReason::Shutdown, 80);

        assert_eq!(rows.len(), 1);
        assert_eq!(line_text(&rows[0]), "Shutting down... Press Ctrl+C again to force exit.");
    }
}
