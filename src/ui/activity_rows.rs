// SPDX-License-Identifier: Apache-2.0

use std::time::Instant;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::app::{
    App,
    activity::{ActivityHeading, ActivityLabel},
};
use crate::ui::{SpinnerState, theme};

/// Composer-only rows: these have no transcript identity or history boundary.
pub(crate) fn build_activity_rows(app: &App, width: u16, now: Instant) -> Vec<Line<'static>> {
    let Some(presentation) = app.activity_presentation(now) else { return Vec::new() };
    if width == 0 {
        return Vec::new();
    }
    let status = Line::from(Span::styled(
        format!("{} {}", SpinnerState::for_app(app).icon(), presentation.text()),
        Style::default().fg(theme::RUST_ORANGE),
    ));
    let mut rows = if presentation.heading == ActivityHeading::Composer {
        crate::ui::wrap::wrap_lines_to_physical_rows(
            &[crate::ui::inline_chat_rows::assistant_role_label_line()],
            width,
        )
    } else {
        Vec::new()
    };
    rows.extend(railed_rows(status, width, false));
    if let Some(tip) = presentation.tip {
        rows.extend(railed_rows(
            Line::from(vec![
                Span::raw("Tip: "),
                Span::styled(tip, Style::default().fg(theme::DIM)),
            ]),
            width,
            true,
        ));
    } else if presentation.label == ActivityLabel::Compacting {
        rows.extend(railed_rows(
            Line::from(Span::styled(
                "Keep drafting — sending resumes when compaction finishes.",
                Style::default().fg(theme::DIM),
            )),
            width,
            true,
        ));
    }
    rows
}

fn railed_rows(line: Line<'static>, width: u16, wrap: bool) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let rail_style = Style::default().fg(theme::RUST_ORANGE);
    if width <= 2 {
        return vec![Line::from(Span::styled("│", rail_style))];
    }
    let body_rows = crate::ui::wrap::wrap_lines_to_physical_rows(&[line], width - 2);
    body_rows
        .into_iter()
        .take(if wrap { usize::MAX } else { 1 })
        .map(|mut row| {
            row.spans.insert(0, Span::styled("│ ", rail_style));
            row
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent::settings::SettingsSnapshot, app::AppStatus};

    #[test]
    fn activity_groups_immediate_tip_with_orange_rail_and_no_trailing_blank() {
        let mut app = App::test_default();
        let now = Instant::now();
        app.transcript.messages.push(crate::app::ChatMessage::new(
            crate::app::MessageRole::Assistant,
            Vec::new(),
            None,
        ));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        app.begin_turn_activity(now);
        app.config.snapshot =
            Some(SettingsSnapshot::test_value("prefersReducedMotion", serde_json::json!(true)));
        for width in [1, 2, 12, 80] {
            let rows = build_activity_rows(&app, width, now);
            assert!(rows.len() >= 2);
            assert!(rows.iter().all(|row| row.width() <= usize::from(width)));
            assert!(rows.iter().all(|row| row.to_string().starts_with('│')));
            assert!(rows.iter().all(|row| row.spans[0].style.fg == Some(theme::RUST_ORANGE)));
            if width > 2 {
                assert!(rows[0].to_string().starts_with("│ ◆ "));
                assert!(rows[1].to_string().starts_with("│ Tip: "));
                assert_eq!(rows[0].spans[1].style.fg, Some(theme::RUST_ORANGE));
            }
        }
        assert!(build_activity_rows(&app, 0, now).is_empty());
        let working = build_activity_rows(&app, 80, now);
        app.status = AppStatus::Thinking;
        assert_eq!(build_activity_rows(&app, 80, now), working);
        app.turn.cancel_requested = true;
        let rows = build_activity_rows(&app, 80, now);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].to_string().trim_end(), "│ ◆ Cancelling…");
        app.turn.reset_for_turn_exit();
        app.status = AppStatus::Ready;
        assert!(build_activity_rows(&app, 80, now).is_empty());
    }

    #[test]
    fn standalone_compaction_owns_a_temporary_composer_heading_without_transcript_changes() {
        let mut app = App::test_default();
        app.session_runtime.session_id = Some("standalone".into());
        let now = Instant::now();
        app.begin_turn_activity(now);
        crate::app::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::SessionUpdate {
                session_id: "standalone".into(),
                update: crate::agent::model::SessionUpdate::CompactionUpdate(
                    crate::agent::model::CompactionUpdate::Started,
                ),
            },
        );
        let rows = build_activity_rows(&app, 80, now);
        assert_eq!(rows[0].to_string().trim_end(), "Claude");
        assert!(rows[1].to_string().contains("Compacting"));
        assert!(app.transcript.messages.is_empty());
        app.turn.compaction.finish();
        assert!(build_activity_rows(&app, 80, now).is_empty());
    }

    #[test]
    fn disabling_tips_preserves_activity_and_heading_in_every_phase() {
        let mut app = App::test_default();
        let now = Instant::now();
        app.status = AppStatus::Running;
        app.begin_turn_activity(now);
        let original = build_activity_rows(&app, 80, now);
        app.config.snapshot =
            Some(SettingsSnapshot::test_value("spinnerTipsEnabled", serde_json::json!(false)));
        assert_eq!(build_activity_rows(&app, 80, now), original[..2]);
        app.status = AppStatus::Thinking;
        assert_eq!(build_activity_rows(&app, 80, now), original[..2]);
        assert!(app.activity_presentation(now).expect("SDK phase remains available").thinking);
        app.turn.compaction.begin_manual();
        let compacting = build_activity_rows(&app, 80, now);
        assert_eq!(compacting[0], original[0]);
        assert!(compacting[1].to_string().contains("Compacting"));
        assert!(!compacting.iter().any(|row| row.to_string().contains("Tip:")));
        app.turn.cancel_requested = true;
        let cancelling = build_activity_rows(&app, 80, now);
        assert_eq!(cancelling.len(), 2);
        assert_eq!(cancelling[0], original[0]);
        assert!(cancelling[1].to_string().contains("Cancelling"));
        app.turn.cancel_requested = false;
        app.turn.compaction.finish();
        app.status = AppStatus::Running;
        app.config.snapshot =
            Some(SettingsSnapshot::test_value("spinnerTipsEnabled", serde_json::json!(true)));
        assert_eq!(build_activity_rows(&app, 80, now), original);
    }
}
