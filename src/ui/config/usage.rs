// SPDX-License-Identifier: Apache-2.0
use super::common::{render_message, wrapped_height};
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
    let content_area = area;
    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    let windows = app.usage.snapshot.as_ref().map_or_else(Vec::new, usage::visible_windows);

    let mut constraints = vec![Constraint::Length(u16::from(app.usage.snapshot.is_some()))];
    let snapshot = app.usage.snapshot.as_ref();
    let has_snapshot_content = !windows.is_empty()
        || snapshot.and_then(|snapshot| snapshot.extra_usage.as_ref()).is_some()
        || snapshot.and_then(|snapshot| snapshot.session.as_ref()).is_some()
        || snapshot.and_then(|snapshot| snapshot.activity.as_ref()).is_some();
    if has_snapshot_content {
        for window in &windows {
            constraints.push(Constraint::Length(window_height(window, content_area.width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(extra_usage) = snapshot.and_then(|snapshot| snapshot.extra_usage.as_ref()) {
            constraints
                .push(Constraint::Length(extra_usage_height(extra_usage, content_area.width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(session) = snapshot.and_then(|snapshot| snapshot.session.as_ref()) {
            constraints.push(Constraint::Length(session_usage_height(session, content_area.width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(activity) = snapshot.and_then(|snapshot| snapshot.activity.as_ref()) {
            constraints.push(Constraint::Length(activity_height(activity, content_area.width)));
            constraints.push(Constraint::Length(1));
        }
        if let Some(error) = app.usage.last_error.as_deref() {
            constraints.push(Constraint::Length(error_height(error, content_area.width)));
        }
        constraints.push(Constraint::Min(0));
    } else {
        constraints.push(Constraint::Min(3));
    }

    app.config.usage_visible_rows = area.height;
    if !has_snapshot_content {
        app.config.usage_scroll = 0;
        app.config.usage_max_scroll = 0;
        render_empty_state(frame, area, app);
        return;
    }
    let height = constraints
        .iter()
        .fold(0u16, |height, constraint| {
            height.saturating_add(match constraint {
                Constraint::Length(rows) => *rows,
                _ => 0,
            })
        })
        .max(area.height);
    app.config.usage_max_scroll = height.saturating_sub(area.height);
    app.config.usage_scroll = app.config.usage_scroll.min(app.config.usage_max_scroll);
    let mut buffer = Buffer::empty(Rect::new(0, 0, area.width, height));
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(buffer.area);

    if let Some(snapshot) = snapshot {
        let plan = snapshot
            .subscription_type
            .as_deref()
            .map(|value| format!("Plan: {value}"))
            .unwrap_or_default();
        Paragraph::new(plan)
            .style(Style::default().fg(theme::DIM))
            .render(sections[0], &mut buffer);
    }

    let Some(snapshot) = app.usage.snapshot.as_ref() else {
        return;
    };
    let mut section_index = 1usize;
    for window in &windows {
        render_window(&mut buffer, sections[section_index], window);
        section_index += 2;
    }

    if let Some(extra_usage) = snapshot.extra_usage.as_ref() {
        render_extra_usage(&mut buffer, sections[section_index], extra_usage);
        section_index += 2;
    }

    if let Some(session) = snapshot.session.as_ref() {
        render_session_usage(&mut buffer, sections[section_index], session);
        section_index += 2;
    }

    if let Some(activity) = snapshot.activity.as_ref() {
        render_activity(&mut buffer, sections[section_index], activity);
        section_index += 2;
    }

    if let Some(error) = app.usage.last_error.as_deref() {
        render_error(&mut buffer, sections[section_index], error);
    }
    for y in 0..area.height {
        for x in 0..area.width {
            frame.buffer_mut()[(area.x + x, area.y + y)] =
                buffer[(x, y + app.config.usage_scroll)].clone();
        }
    }
}

fn render_empty_state(frame: &mut Frame, area: Rect, app: &App) {
    let (title, body) = if app.usage.in_flight {
        ("Loading usage data", "Waiting for the current account's usage snapshot.")
    } else if let Some(error) = app.usage.last_error.as_deref() {
        ("Unable to load usage", error)
    } else {
        ("No usage snapshot yet", "Press r to fetch Claude usage for the current account.")
    };
    render_message(frame, area, title, body);
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
    Paragraph::new(label_line).wrap(Wrap { trim: false }).render(sections[0], buffer);

    let gauge_area = sections[1];
    let gauge_style = gauge_style(window.utilization);
    Gauge::default()
        .gauge_style(gauge_style)
        .label("")
        .ratio((window.utilization / 100.0).clamp(0.0, 1.0))
        .render(gauge_area, buffer);

    Paragraph::new(Line::from(Span::styled(reset_line, Style::default().fg(theme::DIM))))
        .wrap(Wrap { trim: false })
        .render(sections[2], buffer);
}

fn render_extra_usage(buffer: &mut Buffer, area: Rect, extra_usage: &ExtraUsage) {
    let detail = format_extra_usage(extra_usage);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Extra credits")
        .border_style(Style::default().fg(theme::DIM));
    block.render(area, buffer);
    let inner = area.inner(Margin { vertical: 1, horizontal: 2 });
    Paragraph::new(Line::from(Span::styled(detail, Style::default().fg(Color::White))))
        .wrap(Wrap { trim: false })
        .render(inner, buffer);
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
    block.render(area, buffer);
    Paragraph::new(detail)
        .wrap(Wrap { trim: false })
        .render(area.inner(Margin { vertical: 1, horizontal: 2 }), buffer);
}

fn render_error(buffer: &mut Buffer, area: Rect, error: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Latest refresh error")
        .border_style(Style::default().fg(theme::STATUS_ERROR));
    block.render(area, buffer);
    Paragraph::new(error)
        .wrap(Wrap { trim: false })
        .render(area.inner(Margin { vertical: 1, horizontal: 2 }), buffer);
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
            None => "Usage available".to_owned(),
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
    if session.total_lines_added.is_some() || session.total_lines_removed.is_some() {
        details.push(format!(
            "+{:.0} / -{:.0} lines",
            session.total_lines_added.unwrap_or(0.0),
            session.total_lines_removed.unwrap_or(0.0)
        ));
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
    use crate::app::{UsageSnapshot, UsageSourceKind, UsageSourceMode, UsageState};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::SystemTime;

    fn render_usage_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                super::render(frame, frame.area(), app);
            })
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn buffer_rows(buffer: &Buffer) -> Vec<String> {
        buffer
            .content
            .chunks(usize::from(buffer.area.width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect()
    }

    fn render_usage_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        buffer_rows(&render_usage_buffer(app, width, height))
    }

    fn render_usage(app: &mut App) -> String {
        render_usage_rows(app, 100, 24).join("\n")
    }

    fn usage_app() -> App {
        let mut app = App::test_default();
        app.usage = UsageState {
            snapshot: None,
            in_flight: false,
            last_error: None,
            active_source: UsageSourceMode::Auto,
            last_attempted_source: None,
        };
        app
    }

    fn recommendation_app() -> App {
        let raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/sdk-usage-recommendations.json"
        ))
        .expect("SDK contract fixture");
        let event: crate::agent::wire::BridgeEvent = serde_json::from_value(serde_json::json!({
            "event":"usage_snapshot", "session_id":"usage-session", "snapshot":{
                "five_hour":raw["rate_limits"]["five_hour"],
                "session":raw["session"],
                "activity_day":raw["behaviors"]["day"],
                "activity_week":raw["behaviors"]["week"]
            }
        }))
        .expect("wire response");
        let crate::agent::wire::BridgeEvent::UsageSnapshot { snapshot, error, .. } = event else {
            panic!("usage event");
        };
        let mut app = usage_app();
        app.config.active_tab = crate::app::ConfigTab::Usage;
        crate::app::usage::apply_structured_sdk_result(&mut app, snapshot, error);
        app
    }

    #[test]
    fn activity_labels_and_recommendations_have_visible_hierarchy() {
        for width in [55, 100] {
            let mut app = recommendation_app();
            let activity = app.usage.snapshot.as_mut().unwrap().activity.as_mut().unwrap();
            activity.day.as_mut().unwrap().behaviors = vec![crate::app::UsageBehaviorAttribution {
                key: "long_context".to_owned(),
                pct: 25.0,
                count: 5,
            }];
            activity.day.as_mut().unwrap().recommendations.as_mut().unwrap()[0].detail = Some(
                "Review /effort before your next task.\nUse a lower effort for routine work."
                    .to_owned(),
            );
            let buffer = render_usage_buffer(&mut app, width, 80);
            let rows = buffer_rows(&buffer);
            let position = |text: &str| {
                let y = rows.iter().position(|row| row.contains(text)).expect(text);
                let x = rows[y].find(text).unwrap();
                // String offsets include the multibyte card border; buffer columns do not.
                let x = rows[y][..x].chars().count();
                (u16::try_from(x).unwrap(), u16::try_from(y).unwrap())
            };
            for label in [
                "Last 24 hours",
                "Last 7 days",
                "Activity",
                "Behaviors",
                "Recommendations",
                "Lower effort for routine work",
            ] {
                let (x, y) = position(label);
                for column in x..x + u16::try_from(label.len()).unwrap() {
                    assert!(buffer[(column, y)].modifier.contains(Modifier::BOLD), "{label}");
                }
            }
            let (activity_x, activity_y) = position("Activity");
            let (count_x, count_y) = position("20 requests");
            let (behavior_x, behavior_y) = position("Behaviors");
            let (value_x, value_y) = position("long context");
            assert_eq!(activity_y, count_y);
            assert_eq!(behavior_y, value_y);
            assert_eq!(activity_x, behavior_x);
            assert_eq!(count_x, value_x, "field values share a column");
            assert!(value_x >= behavior_x + u16::try_from("Behaviors".len()).unwrap() + 2);
            assert!(!buffer[(value_x, value_y)].modifier.contains(Modifier::BOLD));
            for label in ["Recommendations", "Last 7 days", "Review the Alpha plugin"] {
                let (_, y) = position(label);
                assert!(
                    rows[usize::from(y - 1)].trim_matches(['\u{2502}', ' ']).is_empty(),
                    "space before {label}"
                );
            }
            let (title_x, title_y) = position("Lower effort for routine work");
            let (detail_x, detail_y) = position("Review /effort before your next task.");
            assert_eq!(detail_y, title_y + 1);
            assert_eq!(detail_x, title_x, "detail aligns with its recommendation title");
            assert_eq!(buffer[(detail_x, detail_y)].fg, theme::DIM);
            assert!(!buffer[(detail_x, detail_y)].modifier.contains(Modifier::BOLD));
            let (continuation_x, continuation_y) = position("Use a lower effort for routine work.");
            assert_eq!(continuation_y, detail_y + 1);
            assert_eq!(continuation_x, detail_x);
            assert_eq!(buffer[(continuation_x, continuation_y)].fg, theme::DIM);
            assert_eq!(
                rows.iter().filter(|row| row.contains("Recommendation percentages")).count(),
                1
            );
            assert!(
                rows.iter().any(|row| row.contains("claude.ai.")),
                "wrapped footer remains visible"
            );
        }
    }

    #[test]
    fn recommendations_keep_their_period_values_order_and_optional_details() {
        let mut app = recommendation_app();
        let rows = render_usage_rows(&mut app, 100, 50);
        let day = rows.iter().position(|row| row.contains("Last 24 hours")).expect("day");
        let week = rows.iter().position(|row| row.contains("Last 7 days")).expect("week");
        let end =
            rows.iter().position(|row| row.contains("Local transcript scan")).expect("disclaimer");
        let day_rows = &rows[day..week];
        let week_rows = &rows[week..end];
        let effort = day_rows
            .iter()
            .position(|row| row.contains("Lower effort for routine work (71%)"))
            .expect("effort recommendation");
        assert!(day_rows[effort + 1].contains("Review /effort before your next task."));
        let alpha = day_rows
            .iter()
            .position(|row| row.contains("Review the Alpha plugin (53%)"))
            .expect("first plugin");
        let beta = day_rows
            .iter()
            .position(|row| row.contains("Review the Beta plugin (35%)"))
            .expect("second plugin sharing key");
        assert!(effort < alpha && alpha < beta, "preserve SDK ordering and repeated source keys");
        assert!(week_rows.iter().any(|row| row.contains("Review the weekly workflow (81%)")));
        assert!(!week_rows.iter().any(|row| row.contains("Lower effort")));
        assert!(!day_rows.iter().any(|row| row.contains("weekly workflow")));
        assert!(
            rows[week..]
                .iter()
                .any(|row| row.contains("local usage affected, not estimated savings"))
        );
        assert!(!rows.iter().any(|row| row.contains("FIX-MUST-NOT-BE-SUBMITTED")));
    }

    #[test]
    fn usage_navigation_reaches_recommendations_and_recovers_after_resize_or_refresh() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for width in [55, 100] {
            let mut app = recommendation_app();
            let start = render_usage_rows(&mut app, width, 12);
            assert!(start.iter().any(|row| row.contains("5-hour")));
            assert!(!start.iter().any(|row| row.contains("weekly workflow")));
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            );
            let end = render_usage_rows(&mut app, width, 12);
            assert!(
                end.iter().any(|row| row.contains("Review the weekly workflow (81%)")),
                "{end:?}"
            );
            assert!(!end.iter().any(|row| row.contains("5-hour")));
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            );
            assert_eq!(render_usage_rows(&mut app, width, 12), start);
            for _ in 0..10 {
                crate::app::config::handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                );
            }
            assert_eq!(render_usage_rows(&mut app, width, 12), end);
            let expanded = render_usage_rows(&mut app, width, 80);
            assert!(expanded.iter().any(|row| row.contains("5-hour")));
            assert!(expanded.iter().any(|row| row.contains("Review the weekly workflow (81%)")));
            render_usage_rows(&mut app, width, 12);
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            );
            let replacement =
                serde_json::from_value(serde_json::json!({"five_hour":{"utilization":7}}))
                    .expect("replacement snapshot");
            crate::app::usage::apply_structured_sdk_result(&mut app, Some(replacement), None);
            let refreshed = render_usage_rows(&mut app, width, 12);
            assert!(refreshed.iter().any(|row| row.contains("5-hour") && row.contains("7% used")));
            assert!(!refreshed.iter().any(|row| row.contains("weekly workflow")));
        }
    }

    #[test]
    fn usage_distinguishes_unavailable_recommendations_from_an_empty_result() {
        let mut app = recommendation_app();
        let snapshot = serde_json::from_value(serde_json::json!({
            "activity_day":{"request_count":2,"session_count":1},
            "activity_week":{"request_count":3,"session_count":1,"recommendations":[]}
        }))
        .expect("availability snapshot");
        crate::app::usage::apply_structured_sdk_result(&mut app, Some(snapshot), None);
        let rows = render_usage_rows(&mut app, 100, 24);
        let day = rows.iter().position(|row| row.contains("Last 24 hours")).expect("day");
        let week = rows.iter().position(|row| row.contains("Last 7 days")).expect("week");
        assert!(rows[day..week].iter().any(|row| row.contains("Recommendations unavailable")));
        assert!(rows[week..].iter().any(|row| row.contains("No recommendations")));
    }

    #[test]
    fn renders_idle_state() {
        let mut app = usage_app();
        let rendered = render_usage(&mut app);
        assert!(rendered.contains("No usage snapshot yet"));
    }

    #[test]
    fn renders_loading_state() {
        let mut app = usage_app();
        app.usage.in_flight = true;
        let rendered = render_usage(&mut app);
        assert!(rendered.contains("Loading usage data"));
    }

    #[test]
    fn renders_snapshot_with_extra_usage_and_error() {
        let mut app = usage_app();
        app.usage.snapshot = Some(UsageSnapshot {
            source: UsageSourceKind::Oauth,
            fetched_at: SystemTime::now(),
            subscription_type: None,
            five_hour: Some(UsageWindow {
                label: "5-hour".to_owned(),
                utilization: 47.0,
                resets_at: None,
                reset_description: Some("resets in 2h 14m".to_owned()),
            }),
            seven_day: Some(UsageWindow {
                label: "7-day".to_owned(),
                utilization: 62.0,
                resets_at: None,
                reset_description: Some("resets in 4d 11h".to_owned()),
            }),
            seven_day_oauth_apps: None,
            seven_day_opus: None,
            seven_day_sonnet: None,
            model_scoped: Vec::new(),
            extra_usage: Some(ExtraUsage {
                monthly_limit: Some(20.0),
                used_credits: Some(12.4),
                utilization: Some(62.0),
                currency: Some("USD".to_owned()),
            }),
            session: None,
            activity: None,
        });
        app.usage.last_error = Some("Network timeout while refreshing cached data.".to_owned());

        let rendered = render_usage(&mut app);
        assert!(rendered.contains("5-hour"));
        assert_eq!(rendered.matches("47%").count(), 1);
        assert!(rendered.contains("12.40"));
        assert!(rendered.contains("20.00"));
        assert!(rendered.contains("USD"));
        assert!(rendered.contains("Extra credits"));
        assert!(rendered.contains("Latest refresh error"));
        let rendered_lines = rendered.lines().collect::<Vec<_>>();
        let first_reset_index = rendered_lines
            .iter()
            .position(|line| line.contains("resets in 2h 14m"))
            .expect("reset line");
        assert!(rendered_lines[first_reset_index + 1].trim().is_empty());
    }

    #[test]
    fn extra_usage_wraps_inside_card_on_narrow_widths() {
        let mut app = usage_app();
        app.usage.snapshot = Some(UsageSnapshot {
            source: UsageSourceKind::Oauth,
            fetched_at: SystemTime::now(),
            subscription_type: None,
            five_hour: Some(UsageWindow {
                label: "5-hour".to_owned(),
                utilization: 47.0,
                resets_at: None,
                reset_description: Some("resets soon".to_owned()),
            }),
            seven_day: None,
            seven_day_oauth_apps: None,
            seven_day_opus: None,
            seven_day_sonnet: None,
            model_scoped: Vec::new(),
            extra_usage: Some(ExtraUsage {
                monthly_limit: Some(100.0),
                used_credits: Some(99.99),
                utilization: Some(99.0),
                currency: Some("USD".to_owned()),
            }),
            session: None,
            activity: None,
        });

        let rows = render_usage_rows(&mut app, 20, 18);
        assert!(rows.iter().any(|row| row.contains("Extra credits")));
        assert!(rows.iter().any(|row| row.contains("99.99")));
        assert!(rows.iter().any(|row| row.contains("USD")));
        assert!(rows.iter().any(|row| row.contains("used")));
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
            let rows = render_usage_rows(&mut app, width, 60);
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

    #[test]
    fn renders_structured_session_totals_and_approximate_activity_without_plan_windows() {
        let mut app = usage_app();
        app.usage.snapshot = Some(UsageSnapshot {
            source: UsageSourceKind::Sdk,
            fetched_at: SystemTime::now(),
            subscription_type: Some("max".to_owned()),
            five_hour: None,
            seven_day: None,
            seven_day_oauth_apps: None,
            seven_day_opus: None,
            seven_day_sonnet: None,
            model_scoped: Vec::new(),
            extra_usage: None,
            session: Some(SessionUsageSummary {
                total_cost_usd: Some(0.125),
                total_api_duration_ms: Some(1_500.0),
                total_duration_ms: Some(2_000.0),
                total_lines_added: Some(12.0),
                total_lines_removed: Some(3.0),
                model_count: Some(2),
            }),
            activity: Some(UsageActivitySummary {
                day: Some(UsageActivityWindow {
                    request_count: 4,
                    session_count: 2,
                    recommendations: None,
                    behaviors: vec![crate::app::UsageBehaviorAttribution {
                        key: "long_context".to_owned(),
                        pct: 25.0,
                        count: 1,
                    }],
                    agents: vec![crate::app::UsageNamedAttribution {
                        name: "Explore".to_owned(),
                        pct: 40.0,
                    }],
                    skills: Vec::new(),
                    plugins: Vec::new(),
                    mcp_servers: Vec::new(),
                }),
                week: None,
            }),
        });

        let rendered = render_usage_rows(&mut app, 100, 24).join("\n");
        assert!(rendered.contains("Current session totals"));
        assert!(rendered.contains("Plan: max"));
        assert!(rendered.contains("$0.1250 cost"));
        assert!(rendered.contains("+12 / -3 lines"));
        assert!(rendered.contains("Approximate local activity"));
        assert!(rendered.contains("Last 24 hours"));
        assert!(rendered.contains("4 requests across 2 sessions"));
        assert!(rendered.contains("Behaviors    long context 25% (1 request)"));
        assert!(rendered.contains("Agents       Explore 40%"));
        assert!(rendered.contains("behavior categories overlap"));
        assert!(rendered.contains("claude.ai"));
    }
}
