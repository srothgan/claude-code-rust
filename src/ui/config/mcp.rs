// SPDX-License-Identifier: Apache-2.0
use super::common::{
    badge_span, detail_kv, overlay_line_style, render_message, section_heading, selected_scroll,
    selection_marker, selection_style, wrapped_height,
};
use super::input::render_text_input_field;
use super::overlay::{
    OverlayChrome, OverlayLayoutSpec, render_overlay_separator, render_overlay_shell,
};
use super::theme;
use crate::agent::model::{McpServerConnectionStatus, McpServerStatus, McpServerStatusConfig};
use crate::agent::types::ElicitationAction;
use crate::app::App;
use crate::app::config::{
    available_mcp_actions, is_mcp_action_available, mcp_server_owner_summary,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};

#[cfg(test)]
#[path = "mcp_audit_tests.rs"]
mod audit_tests;

pub(super) fn render(frame: &mut Frame, area: Rect, app: &App) {
    let content_area = area;
    if content_area.width == 0 || content_area.height == 0 {
        return;
    }

    let summary = summary_lines(app);
    let summary_height = wrapped_height(Text::from(summary.clone()), content_area.width)
        .min(content_area.height.saturating_sub(if app.mcp.servers.is_empty() { 1 } else { 3 }));
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(summary_height), Constraint::Min(1)])
        .split(content_area);
    frame.render_widget(Paragraph::new(summary).wrap(Wrap { trim: false }), sections[0]);

    if app.session_runtime.session_id.is_none() {
        render_message(
            frame,
            sections[1],
            "No active session",
            "Open or resume a session to inspect MCP servers from the live SDK session.",
        );
        return;
    }

    if app.mcp.in_flight && app.mcp.servers.is_empty() {
        render_message(
            frame,
            sections[1],
            "Loading MCP status",
            "Waiting for the current session to return MCP server state.",
        );
        return;
    }

    if app.mcp.servers.is_empty() {
        let body = app.mcp.last_error.as_deref().unwrap_or(
            "The current session did not report any MCP servers. This view only shows live session-backed MCP state.",
        );
        render_message(frame, sections[1], "No MCP servers", body);
        return;
    }

    render_server_list(frame, sections[1], app);
}

pub(super) fn render_details_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(overlay) = app.config.mcp_details_overlay() else {
        return;
    };

    let server = app.mcp.servers.iter().find(|server| server.name == overlay.server_name);
    let action_lines =
        server.map_or_else(Vec::new, |server| mcp_action_lines(app, server, overlay));
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 72,
            min_height: 12,
            width_percent: 78,
            height_percent: 82,
            preferred_height: 24,
            fullscreen_below: Some((80, 18)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: overlay.server_name.as_str(),
            subtitle: None,
            help: Some("Up/Down select | Enter run | PgUp/PgDn scroll | Home/End | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );

    if action_lines.is_empty() {
        let body =
            server.map_or_else(server_missing_lines, |server| server_detail_lines(app, server));
        render_details_body(frame, rendered.body_area, body, &mut app.config);
        return;
    }

    let action_height = action_area_height(
        action_lines.clone(),
        rendered.body_area.width,
        rendered.body_area.height,
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1), Constraint::Length(action_height)])
        .split(rendered.body_area);

    let body = server.map_or_else(server_missing_lines, |server| server_detail_lines(app, server));
    let selected_index = overlay.selected_index;
    render_details_body(frame, sections[0], body, &mut app.config);
    render_overlay_separator(frame, sections[1]);
    let action_scroll =
        selected_scroll(2usize.saturating_add(selected_index), 1, sections[2].height);
    frame.render_widget(
        Paragraph::new(action_lines).scroll((action_scroll, 0)).wrap(Wrap { trim: false }),
        sections[2],
    );
}

fn render_details_body(
    frame: &mut Frame,
    area: Rect,
    body: Vec<Line<'static>>,
    config: &mut crate::app::config::ConfigState,
) {
    let paragraph = Paragraph::new(body).wrap(Wrap { trim: false });
    let height = u16::try_from(paragraph.line_count(area.width.max(1))).unwrap_or(u16::MAX);
    config.overlay_read_only.resize(area.height, height);
    frame.render_widget(paragraph.scroll((config.overlay_read_only.offset(), 0)), area);
}

pub(super) fn render_callback_url_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.mcp_callback_url_overlay() else {
        return;
    };
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 68,
            min_height: 12,
            width_percent: 72,
            height_percent: 42,
            preferred_height: 14,
            fullscreen_below: Some((80, 18)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "Submit callback URL",
            subtitle: Some(overlay.server_name.as_str()),
            help: Some("Enter submit | Esc back"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let header_lines = vec![
        section_heading("Callback"),
        Line::from(Span::styled(
            "Paste the OAuth callback URL returned by the provider.",
            Style::default().fg(theme::DIM),
        )),
        Line::default(),
        detail_kv("Server", &overlay.server_name, Color::White),
    ];
    let footer_lines = vec![Line::from(Span::styled(
        "The URL is sent to the SDK exactly as pasted.",
        Style::default().fg(theme::DIM),
    ))];
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(wrapped_height(
                Text::from(header_lines.clone()),
                rendered.body_area.width,
            )),
            Constraint::Length(1),
            Constraint::Length(wrapped_height(
                Text::from(footer_lines.clone()),
                rendered.body_area.width,
            )),
            Constraint::Min(0),
        ])
        .split(rendered.body_area);
    frame.render_widget(Paragraph::new(header_lines).wrap(Wrap { trim: false }), sections[0]);
    render_text_input_field(
        frame,
        sections[1],
        &overlay.draft,
        overlay.cursor,
        "https://callback.example/...",
    );
    frame.render_widget(Paragraph::new(footer_lines).wrap(Wrap { trim: false }), sections[2]);
}

pub(super) fn render_elicitation_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(entry) = app.mcp.elicitations.active() else {
        return;
    };
    let subtitle = format!(
        "{} - {} waiting",
        entry.request.server_name,
        app.mcp.elicitations.len().saturating_sub(1)
    );
    let message = if entry.sending {
        Some(crate::app::config::OverlayMessage::info("Waiting for SDK confirmation..."))
    } else {
        entry.error.as_ref().map(crate::app::config::OverlayMessage::error)
    };
    let help = if entry.field_count() > 0 {
        "Tab/Up/Down move | Left/Right choice | Space toggle | Ctrl+O omit | Enter next/action | Esc cancel"
    } else {
        "Up/Down select | Enter respond | Esc cancel"
    };
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 60,
            min_height: 16,
            width_percent: 85,
            height_percent: 85,
            preferred_height: 30,
            fullscreen_below: Some((90, 20)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "MCP request",
            subtitle: Some(&subtitle),
            help: Some(help),
            message: message.as_ref(),
        },
    );
    let heading = Paragraph::new(entry.request.message.as_str()).wrap(Wrap { trim: false });
    let heading_height = u16::try_from(heading.line_count(rendered.body_area.width.max(1)))
        .unwrap_or(u16::MAX)
        .saturating_add(1)
        .min(rendered.body_area.height.saturating_sub(2));
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(heading_height), Constraint::Min(0)])
        .split(rendered.body_area);
    frame.render_widget(heading, sections[0]);
    render_elicitation_fields(frame, sections[1], entry);
}

fn render_elicitation_fields(
    frame: &mut Frame,
    fields_area: Rect,
    entry: &crate::app::config::mcp::form::ElicitationEntry,
) {
    let mut lines = Vec::new();
    if let Some(url) = &entry.request.url {
        lines.push(detail_value(url, Color::White));
    }
    if entry.browser_opened {
        lines.push(detail_value("Browser opened. Finish there, then accept.", theme::DIM));
    }
    if let Some(error) = &entry.browser_open_error {
        lines.push(detail_value(error, theme::STATUS_ERROR));
    }

    let mut selected_end = 0;
    match &entry.form {
        Err(reason) => lines
            .push(detail_value(&format!("Cannot edit this form: {reason}"), theme::STATUS_WARNING)),
        Ok(form) => {
            for (index, field) in form.fields.iter().enumerate() {
                let selected = entry.focus == index;
                lines.push(Line::from(Span::styled(
                    elicitation_field_title(field, selected),
                    overlay_line_style(selected, true),
                )));
                lines.push(detail_value(
                    &elicitation_field_value(field, selected && !entry.sending, fields_area.width),
                    Color::White,
                ));
                if selected {
                    selected_end = lines.len();
                }
                if let Some(description) = &field.description {
                    lines.push(detail_value(description, theme::DIM));
                }
                lines.push(Line::default());
            }
        }
    }
    for (index, action) in entry.actions().iter().enumerate() {
        let selected = entry.focus == entry.field_count() + index;
        lines.push(Line::from(Span::styled(
            format!("{} {}", selection_marker(selected), elicitation_action_label(*action)),
            overlay_line_style(selected, !entry.sending),
        )));
        if selected {
            selected_end = lines.len();
        }
    }
    let selected_bottom = Paragraph::new(lines[..selected_end].to_vec())
        .wrap(Wrap { trim: false })
        .line_count(fields_area.width.max(1));
    let content = Paragraph::new(lines).wrap(Wrap { trim: false });
    let max_scroll = content
        .line_count(fields_area.width.max(1))
        .saturating_sub(usize::from(fields_area.height));
    let scroll = u16::try_from(
        selected_bottom.saturating_sub(usize::from(fields_area.height)).min(max_scroll),
    )
    .unwrap_or(u16::MAX);
    frame.render_widget(content.scroll((scroll, 0)), fields_area);
}

fn elicitation_field_title(field: &crate::app::config::mcp::form::Field, selected: bool) -> String {
    let state = if field.required {
        "required"
    } else if field.present {
        "optional, included"
    } else {
        "optional, omitted"
    };
    format!("{} {} ({state})", selection_marker(selected), field.title)
}

fn elicitation_field_value(
    field: &crate::app::config::mcp::form::Field,
    editing: bool,
    width: u16,
) -> String {
    use crate::app::config::mcp::form::Draft;
    match &field.draft {
        Draft::Text(text) => {
            elicitation_text_value(&text.text, editing.then_some(text.cursor), width)
        }
        Draft::Boolean(value) => format!("  [{}] {}", if *value { "x" } else { " " }, value),
        Draft::Choice { options, selected } => format!(
            "  < {} >",
            selected
                .and_then(|index| options.get(index))
                .map_or("Choose...", |item| item.1.as_str())
        ),
        Draft::Multi { options, selected, cursor } => {
            let label = options.get(*cursor).map_or("", |item| item.1.as_str());
            format!(
                "  < [{}] {label} ({}/{}) >  {} selected",
                if selected.contains(cursor) { "x" } else { " " },
                cursor + 1,
                options.len(),
                selected.len()
            )
        }
    }
}

fn elicitation_text_value(text: &str, cursor: Option<usize>, width: u16) -> String {
    use unicode_width::UnicodeWidthStr;

    // Measure the displayed units after escaping line breaks. A wide character
    // or an escaped newline must not wrap the answer away from its field title.
    let units: Vec<String> = text
        .chars()
        .map(|ch| match ch {
            '\n' => "\\n".into(),
            '\r' => "\\r".into(),
            _ => ch.to_string(),
        })
        .collect();
    let cursor = cursor.map(|index| index.min(units.len()));
    let fits = units.iter().map(|unit| unit.width()).sum::<usize>() + usize::from(cursor.is_some())
        <= usize::from(width.saturating_sub(4));
    // Reserve overflow indicators only when needed; short answers remain whole.
    let budget = usize::from(width.saturating_sub(if fits { 4 } else { 10 }));
    let mut start = if fits { 0 } else { cursor.unwrap_or(0) };
    let mut used = usize::from(cursor.is_some());
    while start > 0 {
        let previous_width = units[start - 1].width();
        if used + previous_width > budget / 2 {
            break;
        }
        start -= 1;
        used += previous_width;
    }
    let mut end = if fits { 0 } else { cursor.unwrap_or(0) };
    while end < units.len() && used + units[end].width() <= budget {
        used += units[end].width();
        end += 1;
    }
    let mut display = String::from("  [");
    if start > 0 {
        display.push_str("...");
    }
    for (index, unit) in units.iter().enumerate().take(end).skip(start) {
        if cursor == Some(index) {
            display.push('|');
        }
        display.push_str(unit);
    }
    if cursor == Some(end) {
        display.push('|');
    }
    if end < units.len() {
        display.push_str("...");
    }
    display.push(']');
    display
}

pub(super) fn render_auth_redirect_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.mcp_auth_redirect_overlay() else {
        return;
    };
    let action_lines = auth_redirect_action_lines(overlay);
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 72,
            min_height: 16,
            width_percent: 80,
            height_percent: 78,
            preferred_height: 22,
            fullscreen_below: Some((90, 20)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: "MCP authentication",
            subtitle: Some(overlay.redirect.server_name.as_str()),
            help: Some("Up/Down select | Enter run | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let action_height = action_area_height(
        action_lines.clone(),
        rendered.body_area.width,
        rendered.body_area.height,
    );
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1), Constraint::Length(action_height)])
        .split(rendered.body_area);
    frame.render_widget(
        Paragraph::new(auth_redirect_body_lines(overlay)).wrap(Wrap { trim: false }),
        sections[0],
    );
    render_overlay_separator(frame, sections[1]);
    let action_scroll = selected_scroll(
        2usize.saturating_add(overlay.selected_index.saturating_mul(2)),
        1,
        sections[2].height,
    );
    frame.render_widget(
        Paragraph::new(action_lines).scroll((action_scroll, 0)).wrap(Wrap { trim: false }),
        sections[2],
    );
}

fn render_server_list(frame: &mut Frame, area: Rect, app: &App) {
    let items = app.mcp.servers.iter().enumerate().map(|(index, server)| {
        let selected = index == app.config.mcp_selected_server_index;
        ListItem::new(server_list_lines(server, selected)).style(selection_style(selected))
    });

    let mut state = ListState::default().with_selected(Some(app.config.mcp_selected_server_index));
    frame.render_stateful_widget(List::new(items).highlight_symbol(""), area, &mut state);
}

fn action_area_height(lines: Vec<Line<'static>>, viewport_width: u16, body_height: u16) -> u16 {
    let desired = wrapped_height(Text::from(lines), viewport_width);
    desired.min(body_height.saturating_sub(2).max(1))
}

fn summary_lines(app: &App) -> Vec<Line<'static>> {
    let counts = status_counts(app);
    let mut stats_spans = vec![
        super::common::position_counter(
            app.config.mcp_selected_server_index,
            app.mcp.servers.len(),
        ),
        Span::raw(" "),
        badge_span(&format!("total {}", app.mcp.servers.len()), Color::Black, Color::White),
        Span::styled(" ", Style::default()),
        badge_span(&format!("connected {}", counts.connected), Color::Black, theme::RUST_ORANGE),
        Span::styled(" ", Style::default()),
        badge_span(
            &format!("needs auth {}", counts.needs_auth),
            Color::Black,
            theme::STATUS_WARNING,
        ),
        Span::styled(" ", Style::default()),
        badge_span(&format!("pending {}", counts.pending), Color::Black, Color::Cyan),
        Span::styled(" ", Style::default()),
        badge_span(&format!("disabled {}", counts.disabled), Color::White, Color::DarkGray),
        Span::styled(" ", Style::default()),
        badge_span(&format!("failed {}", counts.failed), Color::White, theme::STATUS_ERROR),
    ];
    if app.mcp.in_flight {
        stats_spans.push(Span::styled(" ", Style::default()));
        stats_spans.push(badge_span("refreshing", Color::Black, Color::Cyan));
    }

    let mut lines = vec![Line::from(stats_spans), Line::default()];

    if let Some(error) = app.mcp.last_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("Last MCP error: {error}"),
            Style::default().fg(theme::STATUS_ERROR),
        )));
        lines.push(Line::default());
    }

    lines
}

fn server_list_lines(server: &McpServerStatus, selected: bool) -> Vec<Line<'static>> {
    vec![
        super::common::selection_line(
            Line::from(vec![
                Span::styled(server.name.clone(), super::common::title_style()),
                Span::styled("  ", Style::default()),
                badge_span(
                    status_label(server.status),
                    status_badge_fg(server.status),
                    status_color(server.status),
                ),
                Span::styled(" ", Style::default()),
                badge_span(scope_label(server.scope.as_deref()), Color::White, Color::DarkGray),
                Span::styled(" ", Style::default()),
                badge_span(transport_label(server.config.as_ref()), Color::Black, Color::White),
            ]),
            selected,
        ),
        Line::from(Span::styled(
            format!("  {}", server_summary_line(server)),
            server_secondary_style(server),
        )),
        Line::default(),
    ]
}

fn server_detail_lines(app: &App, server: &McpServerStatus) -> Vec<Line<'static>> {
    let mut lines = vec![
        section_heading("Status"),
        detail_kv("Status", status_label(server.status), status_color(server.status)),
        detail_kv(
            "Enabled",
            if matches!(server.status, McpServerConnectionStatus::Disabled) { "No" } else { "Yes" },
            Color::White,
        ),
        detail_kv("Scope", scope_label(server.scope.as_deref()), Color::White),
        detail_kv("Source", scope_label(server.source.as_deref()), Color::White),
        detail_kv("Transport", transport_label(server.config.as_ref()), Color::White),
        detail_kv("Tools", &tool_summary(server.tools.len()), Color::White),
    ];

    if let Some(owner) = mcp_server_owner_summary(app, server) {
        lines.push(detail_kv("Owner", &owner, Color::White));
    }

    if let Some(info) = server.server_info.as_ref() {
        lines.push(detail_kv("Server name", &info.name, Color::White));
        lines.push(detail_kv("Version", &info.version, Color::White));
    }

    if let Some(config) = server.config.as_ref() {
        lines.push(Line::default());
        lines.push(section_heading("Configuration"));
        lines.extend(config_lines(config));
    }

    if let Some(error) = server.error.as_deref() {
        lines.push(Line::default());
        lines.push(section_heading("Error"));
        lines.push(detail_value(error, theme::STATUS_ERROR));
    }

    lines
}

fn server_missing_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "The selected server is no longer present in the latest MCP snapshot.",
            Style::default().fg(theme::DIM),
        )),
        Line::default(),
        Line::from(Span::styled(
            "Close this overlay and refresh the MCP list.",
            Style::default().fg(theme::DIM),
        )),
    ]
}

fn mcp_action_lines(
    app: &App,
    server: &McpServerStatus,
    overlay: &crate::app::config::McpDetailsOverlayState,
) -> Vec<Line<'static>> {
    let actions = available_mcp_actions(app, server);
    if actions.is_empty() {
        return vec![detail_value("No actions available.", theme::DIM)];
    }

    let mut lines = vec![section_heading("Actions"), Line::default()];
    for (index, action) in actions.into_iter().enumerate() {
        let selected = index == overlay.selected_index;
        let mut spans = vec![Span::styled(
            format!("{} {}", selection_marker(selected), action.label()),
            overlay_line_style(selected, true),
        )];
        if !is_mcp_action_available(app, server, action) {
            spans.push(Span::styled("  ", Style::default()));
            spans.push(badge_span("not available", Color::Black, theme::STATUS_WARNING));
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn auth_redirect_body_lines(
    overlay: &crate::app::config::McpAuthRedirectOverlayState,
) -> Vec<Line<'static>> {
    let redirect = &overlay.redirect;
    let mut lines = vec![
        section_heading("Request"),
        detail_value(
            "Claude Code returned a browser auth redirect for this MCP server.",
            Color::White,
        ),
        Line::default(),
        section_heading("URL"),
        detail_value(&redirect.auth_url, Color::White),
    ];
    if overlay.browser_opened {
        lines.push(Line::default());
        lines.push(detail_value(
            "Opened your browser automatically. Finish auth there, then refresh.",
            theme::DIM,
        ));
    }
    if let Some(error) = overlay.browser_open_error.as_deref() {
        lines.push(Line::default());
        lines.push(detail_value(error, theme::STATUS_ERROR));
    }
    lines
}

fn auth_redirect_action_lines(
    overlay: &crate::app::config::McpAuthRedirectOverlayState,
) -> Vec<Line<'static>> {
    const ACTIONS: [&str; 3] = ["Refresh", "Copy URL", "Close"];
    let mut lines = vec![section_heading("Actions"), Line::default()];
    for (index, label) in ACTIONS.iter().enumerate() {
        let selected = index == overlay.selected_index;
        lines.push(Line::from(Span::styled(
            format!("{} {}", selection_marker(selected), label),
            overlay_line_style(selected, true),
        )));
        if index + 1 < ACTIONS.len() {
            lines.push(Line::default());
        }
    }
    lines.push(Line::default());
    lines
}

fn config_lines(config: &McpServerStatusConfig) -> Vec<Line<'static>> {
    match config {
        McpServerStatusConfig::Stdio {
            command,
            args,
            env,
            timeout,
            request_timeout_ms,
            always_load,
        } => {
            let args_label = if args.is_empty() { "(none)".to_owned() } else { args.join(" ") };
            let mut lines = vec![
                detail_kv("Command", command, Color::White),
                detail_kv("Args", &args_label, Color::White),
                detail_kv("Env", &format!("{} variable(s)", env.len()), Color::White),
            ];
            append_mcp_runtime_config_lines(
                &mut lines,
                *timeout,
                *request_timeout_ms,
                *always_load,
                &[],
            );
            lines
        }
        McpServerStatusConfig::Sse {
            url,
            headers,
            tools,
            timeout,
            request_timeout_ms,
            always_load,
        }
        | McpServerStatusConfig::Http {
            url,
            headers,
            tools,
            timeout,
            request_timeout_ms,
            always_load,
        } => {
            let mut lines = vec![
                detail_kv("URL", url, Color::White),
                detail_kv("Headers", &format!("{} configured", headers.len()), Color::White),
            ];
            append_mcp_runtime_config_lines(
                &mut lines,
                *timeout,
                *request_timeout_ms,
                *always_load,
                tools,
            );
            lines
        }
        McpServerStatusConfig::Sdk { name } => vec![detail_kv("SDK server", name, Color::White)],
        McpServerStatusConfig::ClaudeaiProxy { url, id, timeout } => {
            let mut lines = vec![
                detail_kv("Proxy URL", url, Color::White),
                detail_kv("Proxy ID", id, Color::White),
            ];
            append_mcp_runtime_config_lines(&mut lines, *timeout, None, None, &[]);
            lines
        }
        McpServerStatusConfig::Unknown { raw_type } => {
            vec![detail_kv("Config type", &format!("unknown ({raw_type})"), theme::DIM)]
        }
    }
}

fn append_mcp_runtime_config_lines(
    lines: &mut Vec<Line<'static>>,
    timeout: Option<u64>,
    request_timeout_ms: Option<u64>,
    always_load: Option<bool>,
    tools: &[crate::agent::model::McpServerToolPolicy],
) {
    if let Some(timeout_ms) = timeout {
        lines.push(detail_kv("Timeout", &format!("{timeout_ms} ms"), Color::White));
    }
    if let Some(timeout_ms) = request_timeout_ms {
        lines.push(detail_kv("Request timeout", &format!("{timeout_ms} ms"), Color::White));
    }
    if let Some(always_load) = always_load {
        lines.push(detail_kv(
            "Always load",
            if always_load { "enabled" } else { "disabled" },
            Color::White,
        ));
    }
    if !tools.is_empty() {
        lines.push(detail_kv(
            "Tool policies",
            &format!("{} configured", tools.len()),
            Color::White,
        ));
        for tool in tools {
            let policy_summary = format_mcp_tool_policy_summary(tool);
            lines.push(detail_kv(&format!("  {}", tool.name), &policy_summary, Color::White));
        }
    }
}

fn format_mcp_tool_policy_summary(tool: &crate::agent::model::McpServerToolPolicy) -> String {
    let permission = tool.permission_policy.map_or("not specified", |policy| policy.label());
    match tool.org_max_permission {
        Some(org_max_permission) => {
            format!("{permission}, org max {}", org_max_permission.label())
        }
        None => permission.to_owned(),
    }
}

fn detail_value(value: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(value.to_owned(), Style::default().fg(color)))
}

fn server_secondary_style(server: &McpServerStatus) -> Style {
    if server.error.as_deref().is_some_and(|error| !error.trim().is_empty()) {
        Style::default().fg(theme::STATUS_ERROR)
    } else {
        Style::default().fg(theme::DIM)
    }
}

fn server_summary_line(server: &McpServerStatus) -> String {
    if let Some(error) = server.error.as_deref()
        && !error.trim().is_empty()
    {
        return error.to_owned();
    }

    let mut parts = Vec::new();
    if let Some(info) = server.server_info.as_ref() {
        parts.push(format!("{} {}", info.name, info.version));
    }
    parts.push(tool_summary(server.tools.len()));
    match server.config.as_ref() {
        Some(McpServerStatusConfig::Stdio { command, .. }) => parts.push(format!("cmd {command}")),
        Some(
            McpServerStatusConfig::Sse { url, .. }
            | McpServerStatusConfig::Http { url, .. }
            | McpServerStatusConfig::ClaudeaiProxy { url, .. },
        ) => parts.push(url.clone()),
        Some(McpServerStatusConfig::Sdk { name }) => parts.push(format!("sdk {name}")),
        Some(McpServerStatusConfig::Unknown { raw_type }) => {
            parts.push(format!("unknown config {raw_type}"));
        }
        None => {}
    }
    parts.join("  |  ")
}

fn tool_summary(tool_count: usize) -> String {
    match tool_count {
        0 => "no tools".to_owned(),
        1 => "1 tool".to_owned(),
        count => format!("{count} tools"),
    }
}

fn status_color(status: McpServerConnectionStatus) -> Color {
    match status {
        McpServerConnectionStatus::Connected => theme::RUST_ORANGE,
        McpServerConnectionStatus::NeedsAuth => theme::STATUS_WARNING,
        McpServerConnectionStatus::Pending => Color::Cyan,
        McpServerConnectionStatus::Disabled => Color::DarkGray,
        McpServerConnectionStatus::Failed => theme::STATUS_ERROR,
    }
}

fn status_badge_fg(status: McpServerConnectionStatus) -> Color {
    match status {
        McpServerConnectionStatus::Connected
        | McpServerConnectionStatus::NeedsAuth
        | McpServerConnectionStatus::Pending => Color::Black,
        McpServerConnectionStatus::Disabled | McpServerConnectionStatus::Failed => Color::White,
    }
}

fn status_label(status: McpServerConnectionStatus) -> &'static str {
    match status {
        McpServerConnectionStatus::Connected => "connected",
        McpServerConnectionStatus::Failed => "failed",
        McpServerConnectionStatus::NeedsAuth => "needs auth",
        McpServerConnectionStatus::Pending => "pending",
        McpServerConnectionStatus::Disabled => "disabled",
    }
}

fn scope_label(scope: Option<&str>) -> &str {
    scope.filter(|value| !value.trim().is_empty()).unwrap_or("unspecified")
}

fn transport_label(config: Option<&McpServerStatusConfig>) -> &'static str {
    match config {
        Some(McpServerStatusConfig::Stdio { .. }) => "stdio",
        Some(McpServerStatusConfig::Sse { .. }) => "sse",
        Some(McpServerStatusConfig::Http { .. }) => "http",
        Some(McpServerStatusConfig::Sdk { .. }) => "sdk",
        Some(McpServerStatusConfig::ClaudeaiProxy { .. }) => "claudeai-proxy",
        Some(McpServerStatusConfig::Unknown { .. }) | None => "unknown",
    }
}

fn status_counts(app: &App) -> StatusCounts {
    app.mcp.servers.iter().fold(StatusCounts::default(), |mut counts, server| {
        match server.status {
            McpServerConnectionStatus::Connected => counts.connected += 1,
            McpServerConnectionStatus::NeedsAuth => counts.needs_auth += 1,
            McpServerConnectionStatus::Pending => counts.pending += 1,
            McpServerConnectionStatus::Disabled => counts.disabled += 1,
            McpServerConnectionStatus::Failed => counts.failed += 1,
        }
        counts
    })
}

fn elicitation_action_label(action: ElicitationAction) -> &'static str {
    match action {
        ElicitationAction::Accept => "Accept",
        ElicitationAction::Decline => "Decline",
        ElicitationAction::Cancel => "Cancel",
    }
}

#[derive(Default)]
struct StatusCounts {
    connected: usize,
    needs_auth: usize,
    pending: usize,
    disabled: usize,
    failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_form(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| super::render_elicitation_overlay(frame, frame.area(), app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect()
    }
    fn form_request(
        schema: serde_json::Value,
        order: Vec<String>,
    ) -> crate::agent::types::ElicitationRequest {
        crate::agent::types::ElicitationRequest {
            request_id: "ui-form".into(),
            server_name: "form-server".into(),
            message: "UI form request".into(),
            mode: crate::agent::types::ElicitationMode::Form,
            url: None,
            elicitation_id: None,
            requested_schema: Some(schema),
            field_order: order,
        }
    }
    #[test]
    fn elicitation_fields_keep_order_value_association_and_focus_at_narrow_widths() {
        let mut app = App::test_default();
        let schema = serde_json::json!({"type":"object","properties":{
            "zulu":{"type":"boolean","default":false},"alpha":{"type":"string","default":"alpha-answer"}
        },"required":["zulu","alpha"]});
        app.mcp.elicitations.insert(form_request(schema, vec!["zulu".into(), "alpha".into()]));
        for (width, height) in [(120, 32), (61, 25), (35, 18)] {
            let rows = render_form(&app, width, height);
            let zulu = rows.iter().position(|row| row.contains("zulu (required)")).unwrap();
            let alpha = rows.iter().position(|row| row.contains("alpha (required)")).unwrap();
            assert!(zulu < alpha);
            assert!(rows[zulu].contains('\u{203a}'));
            assert!(rows[zulu + 1].contains("[ ] false"));
            assert!(rows[alpha + 1].contains("alpha-answer"));
            let footer = rows.iter().rev().take(6).collect::<Vec<_>>();
            assert!(
                footer.iter().any(|row| row.contains("Esc"))
                    && footer.iter().any(|row| row.contains("cancel")),
                "{}",
                rows.join("\n")
            );
        }
    }
    #[test]
    fn a_form_that_fits_keeps_every_field_stationary_through_action_navigation() {
        let mut app = App::test_default();
        let properties = (0..5)
            .map(|index| {
                (
                    format!("field-{index}"),
                    serde_json::json!({"type":"string","default":format!("answer-{index}")}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        app.mcp.elicitations.insert(form_request(
            serde_json::json!({"type":"object","properties":properties}),
            vec![],
        ));
        let original = render_form(&app, 120, 40);
        let heading = original.iter().position(|row| row.contains("UI form request")).unwrap();
        let field_rows = (0..5)
            .map(|index| {
                original.iter().position(|row| row.contains(&format!("field-{index} ("))).unwrap()
            })
            .collect::<Vec<_>>();
        for focus in (0..8).chain((0..8).rev()) {
            app.mcp.elicitations.active_mut().unwrap().focus = focus;
            let rows = render_form(&app, 120, 40);
            assert!(rows[heading].contains("UI form request"));
            for (index, row) in field_rows.iter().copied().enumerate() {
                assert!(rows[row].contains(&format!("field-{index} (")), "{}", rows.join("\n"));
                assert!(rows[row + 1].contains(&format!("answer-{index}")));
                assert_eq!(rows[row].contains('\u{203a}'), focus == index);
            }
            for (index, action) in ["Accept", "Decline", "Cancel"].iter().enumerate() {
                let row = rows.iter().find(|row| row.contains(action)).unwrap();
                assert_eq!(row.contains('\u{203a}'), focus == 5 + index);
            }
        }
    }
}
