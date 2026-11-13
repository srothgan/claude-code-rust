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
use crate::ui::message_layout::MessageLayout;
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
    let layout = MessageLayout::new(width);
    let body_rows = crate::ui::wrap::wrap_lines_to_physical_rows(&[line], layout.content_width);
    body_rows
        .into_iter()
        .take(if wrap { usize::MAX } else { 1 })
        .map(|row| layout.row(row, Some(("│", theme::RUST_ORANGE)), None))
        .collect()
}
