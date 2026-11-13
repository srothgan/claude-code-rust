// SPDX-License-Identifier: Apache-2.0
use super::common::{message, wrapped_height};
use super::theme;
use crate::app::usage;
use crate::app::{
    App, ExtraUsage, SessionUsageSummary, UsageActivitySummary, UsageActivityWindow, UsageWindow,
};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Widget, Wrap};

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.is_empty() {
        return;
    }
    let content = content(app, area.width);
    super::viewport::present(frame, area, &content, &mut app.config);
}

fn content(app: &App, width: u16) -> Buffer {
    let windows = app.usage.snapshot.as_ref().map_or_else(Vec::new, usage::visible_windows);

    let mut constraints = vec![Constraint::Length(u16::from(app.usage.snapshot.is_some()))];
    let snapshot = app.usage.snapshot.as_ref();
    let has_snapshot_content = !windows.is_empty()
        || snapshot.and_then(|snapshot| snapshot.extra_usage.as_ref()).is_some()
        || snapshot.and_then(|snapshot| snapshot.session.as_ref()).is_some()
        || snapshot.and_then(|snapshot| snapshot.activity.as_ref()).is_some();
    if has_snapshot_content {
        for window in &windows {
            constraints.push(Constraint::Length(window_height(window, width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(extra_usage) = snapshot.and_then(|snapshot| snapshot.extra_usage.as_ref()) {
            constraints.push(Constraint::Length(extra_usage_height(extra_usage, width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(session) = snapshot.and_then(|snapshot| snapshot.session.as_ref()) {
            constraints.push(Constraint::Length(session_usage_height(session, width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(activity) = snapshot.and_then(|snapshot| snapshot.activity.as_ref()) {
            constraints.push(Constraint::Length(activity_height(activity, width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(error) = app.usage.last_error.as_deref() {
            constraints.push(Constraint::Length(error_height(error, width)));
        }
    } else {
        let (title, body) = empty_message(app);
        constraints.push(Constraint::Length(
            u16::try_from(message(title, body).line_count(width)).unwrap_or(u16::MAX),
        ));
    }

    let height = constraints.iter().fold(0u16, |height, constraint| {
        if let Constraint::Length(length) = constraint {
            height.saturating_add(*length)
        } else {
            height
        }
    });
    let content_area = Rect::new(0, 0, width, height);
    let mut content = Buffer::empty(content_area);
    let buffer = &mut content;
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(content_area);

    if let Some(snapshot) = snapshot {
        let plan = snapshot
            .subscription_type
            .as_deref()
            .map(|value| format!("Plan: {value}"))
            .unwrap_or_default();
        draw(buffer, Paragraph::new(plan).style(Style::default().fg(theme::DIM)), sections[0]);
    } else {
        render_spacer(buffer, sections[0]);
    }

    if !has_snapshot_content {
        let (title, body) = empty_message(app);
        message(title, body).render(sections[1], buffer);
        return content;
    }

    let Some(snapshot) = app.usage.snapshot.as_ref() else {
        return content;
    };
    let mut section_index = 1usize;
    for window in &windows {
        render_window(buffer, sections[section_index], window);
        render_spacer(buffer, sections[section_index + 1]);
        section_index += 2;
    }

    if let Some(extra_usage) = snapshot.extra_usage.as_ref() {
        render_extra_usage(buffer, sections[section_index], extra_usage);
        render_spacer(buffer, sections[section_index + 1]);
        section_index += 2;
    }

    if let Some(session) = snapshot.session.as_ref() {
        render_session_usage(buffer, sections[section_index], session);
        render_spacer(buffer, sections[section_index + 1]);
        section_index += 2;
    }

    if let Some(activity) = snapshot.activity.as_ref() {
        render_activity(buffer, sections[section_index], activity);
        render_spacer(buffer, sections[section_index + 1]);
        section_index += 2;
    }

    if let Some(error) = app.usage.last_error.as_deref() {
        render_error(buffer, sections[section_index], error);
    }
    content
}

fn draw(buffer: &mut Buffer, widget: impl Widget, area: Rect) {
    widget.render(area, buffer);
}

fn render_spacer(buffer: &mut Buffer, area: Rect) {
    draw(buffer, Paragraph::new(Line::default()), area);
}

fn empty_message(app: &App) -> (&str, &str) {
    if app.usage.in_flight {
        ("Loading usage data", "Waiting for the current account's usage snapshot.")
    } else if let Some(error) = app.usage.last_error.as_deref() {
        ("Unable to load usage", error)
    } else if app.usage.snapshot.is_some() {
        (
            "No usage metrics reported",
            "The current account returned no usage metrics. Press r to refresh.",
        )
    } else {
        ("No usage snapshot yet", "Press r to fetch Claude usage for the current account.")
    }
}

fn render_window(buffer: &mut Buffer, area: Rect, window: &UsageWindow) {
    let label_line = Line::from(vec![
        Span::styled(window.label.clone(), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("   {}", window_detail_text(window)), Style::default().fg(theme::DIM)),
    ]);
    let label_height = wrapped_height(Text::from(vec![label_line.clone()]), area.width);
    let reset_line = usage::format_window_reset(window).unwrap_or_default();
    let reset_height = wrapped_height(
        Text::from(vec![Line::from(Span::styled(
            reset_line.clone(),
            Style::default().fg(theme::DIM),
        ))]),
        area.width,
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(label_height),
            Constraint::Length(1),
            Constraint::Length(reset_height),
        ])
        .split(area);
    draw(buffer, Paragraph::new(label_line).wrap(Wrap { trim: false }), sections[0]);

    let gauge_area = sections[1];
    let gauge_style = gauge_style(window.utilization);
    draw(
        buffer,
        Gauge::default()
            .gauge_style(gauge_style)
            .label("")
            .ratio((window.utilization / 100.0).clamp(0.0, 1.0)),
        gauge_area,
    );

    draw(
        buffer,
        Paragraph::new(Line::from(Span::styled(reset_line, Style::default().fg(theme::DIM))))
            .wrap(Wrap { trim: false }),
        sections[2],
    );
}

fn render_extra_usage(buffer: &mut Buffer, area: Rect, extra_usage: &ExtraUsage) {
    let detail = format_extra_usage(extra_usage);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Extra credits")
        .border_style(Style::default().fg(theme::DIM));
    draw(buffer, block.clone(), area);
    let inner = area.inner(Margin { vertical: 1, horizontal: 2 });
    draw(
        buffer,
        Paragraph::new(Line::from(Span::styled(detail, Style::default().fg(Color::White))))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_session_usage(buffer: &mut Buffer, area: Rect, session: &SessionUsageSummary) {
    render_detail_card(
        buffer,
        area,
        "Current session totals",
        Text::from(format_session_usage(session)),
    );
}

fn render_activity(buffer: &mut Buffer, area: Rect, activity: &UsageActivitySummary) {
    render_detail_card(buffer, area, "Approximate local activity", format_activity(activity));
}

fn render_detail_card(buffer: &mut Buffer, area: Rect, title: &str, detail: Text<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(theme::DIM));
    draw(buffer, block.clone(), area);
    draw(
        buffer,
        Paragraph::new(detail).wrap(Wrap { trim: false }),
        area.inner(Margin { vertical: 1, horizontal: 2 }),
    );
}

fn render_error(buffer: &mut Buffer, area: Rect, error: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Latest refresh error")
        .border_style(Style::default().fg(theme::STATUS_ERROR));
    draw(buffer, block.clone(), area);
    draw(
        buffer,
        Paragraph::new(error).wrap(Wrap { trim: false }),
        area.inner(Margin { vertical: 1, horizontal: 2 }),
    );
}

fn window_detail_text(window: &UsageWindow) -> String {
    format!("{:.0}% used", window.utilization)
}

fn gauge_style(utilization: f64) -> Style {
    let color = if utilization >= 85.0 {
        theme::STATUS_ERROR
    } else if utilization >= 65.0 {
        theme::STATUS_WARNING
    } else {
        theme::RUST_ORANGE
    };
    Style::default().fg(color).bg(Color::DarkGray)
}

fn format_extra_usage(extra_usage: &ExtraUsage) -> String {
    let currency = extra_usage.currency.as_deref().unwrap_or("USD");
    match (extra_usage.used_credits, extra_usage.monthly_limit) {
        (Some(used), Some(limit)) => format!("{used:.2} of {limit:.2} {currency} used"),
        (Some(used), None) => format!("{used:.2} {currency} used"),
        (None, Some(limit)) => format!("{limit:.2} {currency} limit"),
        (None, None) => match extra_usage.utilization {
            Some(utilization) => format!("{utilization:.0}% of monthly budget"),
            None => "No credit usage reported.".to_owned(),
        },
    }
}

fn format_session_usage(session: &SessionUsageSummary) -> String {
    let mut details = Vec::new();
    if let Some(cost) = session.total_cost_usd {
        details.push(format!("${cost:.4} cost"));
    }
    if let Some(duration) = session.total_duration_ms {
        details.push(format!("{} elapsed", format_duration_ms(duration)));
    }
    if let Some(duration) = session.total_api_duration_ms {
        details.push(format!("{} API time", format_duration_ms(duration)));
    }
    match (session.total_lines_added, session.total_lines_removed) {
        (Some(added), Some(removed)) => details.push(format!("+{added:.0} / -{removed:.0} lines")),
        (Some(added), None) => details.push(format!("+{added:.0} lines added")),
        (None, Some(removed)) => details.push(format!("-{removed:.0} lines removed")),
        (None, None) => {}
    }
    if let Some(model_count) = session.model_count {
        details
            .push(format!("{model_count} {}", if model_count == 1 { "model" } else { "models" }));
    }
    if details.is_empty() { "No session totals reported.".to_owned() } else { details.join(" · ") }
}

fn format_duration_ms(milliseconds: f64) -> String {
    let seconds = (milliseconds.max(0.0) / 1_000.0).round();
    let minutes = (seconds / 60.0).floor();
    let remaining_seconds = seconds % 60.0;
    if minutes > 0.0 {
        format!("{minutes:.0}m {remaining_seconds:.0}s")
    } else {
        format!("{remaining_seconds:.0}s")
    }
}

fn format_activity(activity: &UsageActivitySummary) -> Text<'static> {
    let mut rows = Vec::new();
    if let Some(day) = activity.day.as_ref() {
        rows.extend(format_activity_window("Last 24 hours", day));
    }
    if let Some(week) = activity.week.as_ref() {
        if !rows.is_empty() {
            rows.push(Line::default());
        }
        rows.extend(format_activity_window("Last 7 days", week));
    }
    rows.push(Line::default());
    rows.push(Line::styled(
        "Recommendation percentages show local usage affected, not estimated savings.",
        Style::default().fg(theme::DIM),
    ));
    rows.push(Line::styled(
        "Local transcript scan; approximate, behavior categories overlap, and data excludes other devices and claude.ai."
            .to_owned(),
        Style::default().fg(theme::DIM),
    ));
    Text::from(rows)
}

fn format_activity_window(label: &str, window: &UsageActivityWindow) -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut rows = vec![
        Line::styled(label.to_owned(), bold),
        activity_field(
            "Activity",
            format!(
                "{} {} across {} {}",
                window.request_count,
                if window.request_count == 1 { "request" } else { "requests" },
                window.session_count,
                if window.session_count == 1 { "session" } else { "sessions" }
            ),
        ),
    ];
    for (index, item) in window.behaviors.iter().take(5).enumerate() {
        rows.push(activity_field(
            if index == 0 { "Behaviors" } else { "" },
            format!(
                "{} {:.0}% ({} {})",
                item.key.replace('_', " "),
                item.pct,
                item.count,
                if item.count == 1 { "request" } else { "requests" }
            ),
        ));
    }
    append_named_attributions(&mut rows, "Agents", &window.agents);
    append_named_attributions(&mut rows, "Skills", &window.skills);
    append_named_attributions(&mut rows, "Plugins", &window.plugins);
    append_named_attributions(&mut rows, "MCP servers", &window.mcp_servers);
    rows.push(Line::default());
    rows.push(Line::styled("Recommendations", bold));
    match &window.recommendations {
        None => rows.push(Line::styled(
            "  Recommendations unavailable for this period.",
            Style::default().fg(theme::DIM),
        )),
        Some(items) if items.is_empty() => {
            rows.push(Line::styled(
                "  No recommendations for this period.",
                Style::default().fg(theme::DIM),
            ));
        }
        Some(items) => {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    rows.push(Line::default());
                }
                rows.push(Line::from(vec![
                    Span::raw("  - "),
                    Span::styled(item.title.clone(), bold),
                    Span::raw(format!(" ({:.0}%)", item.pct)),
                ]));
                if let Some(detail) = &item.detail {
                    rows.extend(detail.lines().map(|line| {
                        Line::styled(format!("    {line}"), Style::default().fg(theme::DIM))
                    }));
                }
            }
        }
    }
    rows
}

fn activity_field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(format!("{label:<11}"), Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::raw(value),
    ])
}

fn append_named_attributions(
    rows: &mut Vec<Line<'static>>,
    label: &str,
    items: &[crate::app::UsageNamedAttribution],
) {
    if items.is_empty() {
        return;
    }
    rows.push(activity_field(
        label,
        items
            .iter()
            .take(5)
            .map(|item| format!("{} {:.0}%", item.name, item.pct))
            .collect::<Vec<_>>()
            .join(", "),
    ));
}

fn window_height(window: &UsageWindow, width: u16) -> u16 {
    let label_line = Line::from(vec![
        Span::styled(window.label.clone(), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("   {}", window_detail_text(window)), Style::default().fg(theme::DIM)),
    ]);
    let reset_line = Line::from(Span::styled(
        usage::format_window_reset(window).unwrap_or_default(),
        Style::default().fg(theme::DIM),
    ));
    wrapped_height(Text::from(vec![label_line]), width)
        .saturating_add(1)
        .saturating_add(wrapped_height(Text::from(vec![reset_line]), width))
}

fn extra_usage_height(extra_usage: &ExtraUsage, width: u16) -> u16 {
    let inner_width = width.saturating_sub(4);
    wrapped_height(
        Text::from(vec![Line::from(Span::styled(
            format_extra_usage(extra_usage),
            Style::default().fg(Color::White),
        ))]),
        inner_width,
    )
    .saturating_add(2)
}

fn session_usage_height(session: &SessionUsageSummary, width: u16) -> u16 {
    detail_card_height(Text::from(format_session_usage(session)), width)
}

fn activity_height(activity: &UsageActivitySummary, width: u16) -> u16 {
    detail_card_height(format_activity(activity), width)
}

fn detail_card_height(detail: Text<'_>, width: u16) -> u16 {
    wrapped_height(detail, width.saturating_sub(4)).saturating_add(2)
}

fn error_height(error: &str, width: u16) -> u16 {
    let inner_width = width.saturating_sub(4);
    wrapped_height(Text::from(error.to_owned()), inner_width).saturating_add(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::UsageState;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_usage_rows(app: &App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                let content = content(app, area.width);
                let mut state = app.config.clone();
                super::super::viewport::present(frame, area, &content, &mut state);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect()
    }

    fn usage_app() -> App {
        let mut app = App::test_default();
        app.usage = UsageState {
            snapshot: None,
            in_flight: false,
            last_error: None,
            last_attempted_source: None,
        };
        app
    }

    #[test]
    fn sdk_behavior_labels_keep_their_values_in_their_rendered_activity_window() {
        let raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/sdk-usage-attribution.json"
        ))
        .expect("SDK fixture");
        let snapshot = serde_json::from_value(serde_json::json!({
            "rate_limits_available":false,
            "activity_day":raw["behaviors"]["day"], "activity_week":raw["behaviors"]["week"]
        }))
        .expect("wire snapshot");
        let mut app = usage_app();
        crate::app::usage::apply_structured_sdk_result(&mut app, Some(snapshot), None);
        for width in [80, 120] {
            let rows = render_usage_rows(&app, width, 40);
            let day =
                rows.iter().position(|row| row.contains("Last 24 hours")).expect("day heading");
            let week =
                rows.iter().position(|row| row.contains("Last 7 days")).expect("week heading");
            let end = rows
                .iter()
                .position(|row| row.contains("Local transcript scan"))
                .expect("activity explanation");
            assert!(day < week && week < end);
            let day_rows = rows[day + 1..week].join(" ");
            let week_rows = rows[week + 1..end].join(" ");
            // Remove the card gutter and wrapping whitespace, keeping label/value association.
            let compact = |text: &str| {
                text.chars()
                    .filter(|ch| !ch.is_whitespace() && *ch != '\u{2502}')
                    .collect::<String>()
            };
            let day_rows = compact(&day_rows);
            let week_rows = compact(&week_rows);
            assert!(day_rows.contains(&compact("20 requests across 2 sessions")));
            assert!(week_rows.contains(&compact("100 requests across 9 sessions")));
            for (day_value, week_value) in [
                ("max effort 71% (7 requests)", "max effort 81% (17 requests)"),
                ("workflow 62% (6 requests)", "workflow 72% (16 requests)"),
                ("always on context 53% (5 requests)", "always on context 63% (15 requests)"),
                ("large tool results 44% (4 requests)", "large tool results 54% (14 requests)"),
                ("future behavior 35% (3 requests)", "future behavior 45% (13 requests)"),
            ] {
                assert!(day_rows.contains(&compact(day_value)), "{day_rows}");
                assert!(!week_rows.contains(&compact(day_value)));
                assert!(week_rows.contains(&compact(week_value)), "{week_rows}");
                assert!(!day_rows.contains(&compact(week_value)));
            }
            // The existing display bound does not discard the sixth entry from state.
            assert!(!rows.iter().any(|row| row.contains("sixth")));
            let activity = app.usage.snapshot.as_ref().unwrap().activity.as_ref().unwrap();
            assert_eq!(
                activity.day.as_ref().unwrap().behaviors.last().unwrap().key,
                "sixth_behavior"
            );
            assert_eq!(activity.week.as_ref().unwrap().behaviors.last().unwrap().count, 12);
        }
    }
}

#[cfg(test)]
#[path = "usage_recommendation_tests.rs"]
mod recommendation_tests;
