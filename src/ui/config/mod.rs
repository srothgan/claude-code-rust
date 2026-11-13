// SPDX-License-Identifier: Apache-2.0

mod action_overlay;
/// List, selection, badge and hint helpers shared by the fullscreen views.
pub(super) mod common;
mod footer;
mod help;
mod input;
mod mcp;
mod overlay;
mod plugin_overlay;
mod plugins;
mod settings;
mod settings_form;
mod settings_overlay;
mod status;
mod usage;
mod viewport;

#[cfg(test)]
mod status_usage_tests;

use crate::app::{App, ConfigTab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;

use super::theme;
use common::overlay_line_style;
use overlay::{OverlayChrome, OverlayLayoutSpec, render_overlay_shell};
pub fn render(frame: &mut Frame, app: &mut App) {
    let frame_area = frame.area();
    if (frame_area.width < 30 || frame_area.height < 12)
        && matches!(app.config.overlay, Some(crate::app::config::ConfigOverlayState::Footer(_)))
    {
        footer::render(frame, frame_area, app);
        return;
    }
    if (frame_area.width < 30 || frame_area.height < 12)
        && (app.config.overlay.is_none() || app.config.setting_overlay().is_some())
    {
        frame.render_widget(
            Paragraph::new(if app.config.setting_overlay().is_some() {
                "Window too small. Resize to view config.\nEsc cancel"
            } else {
                "Window too small. Resize to view config.\nEsc close"
            })
            .wrap(Wrap { trim: false }),
            frame_area,
        );
        return;
    }

    let compact = frame_area.height < 18;
    let inner = frame_area.inner(Margin { vertical: u16::from(!compact), horizontal: 2 });
    let (message, is_error) = if let Some(error) = app.config.last_error.clone() {
        (error, true)
    } else if let Some(status) = app.config.status_message.clone() {
        let status = if app.config.active_tab == ConfigTab::Settings {
            match status.as_str() {
                "Saved and applied." | "Saved. Changes apply to the next session." => {
                    "Saved.".to_owned()
                }
                _ => status,
            }
        } else {
            status
        };
        (status, false)
    } else {
        (String::new(), false)
    };
    let help = config_help_text(app, inner.width);
    let message_height = if message.is_empty() {
        0
    } else {
        common::wrapped_height(message.as_str(), inner.width).clamp(1, (inner.height / 3).max(1))
    };
    let help_height = common::wrapped_height(help.as_str(), inner.width).clamp(1, 3);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(u16::from(!compact)),
            Constraint::Min(3),
            Constraint::Length(message_height),
            Constraint::Length(help_height),
        ])
        .split(inner);

    render_tab_header(frame, chunks[0], app.config.active_tab);

    match app.config.active_tab {
        ConfigTab::Settings => settings::render(frame, chunks[2], app),
        ConfigTab::Plugins => plugins::render(frame, chunks[2], app),
        ConfigTab::Status => status::render(frame, chunks[2], app),
        ConfigTab::Usage => usage::render(frame, chunks[2], app),
        ConfigTab::Mcp => mcp::render(frame, chunks[2], app),
        ConfigTab::Help => help::render(frame, chunks[2], app),
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            message,
            Style::default().fg(if is_error { theme::STATUS_ERROR } else { theme::DIM }),
        )))
        .wrap(Wrap { trim: false }),
        chunks[3],
    );

    frame.render_widget(
        Paragraph::new(help).style(common::help_style()).wrap(Wrap { trim: false }),
        chunks[4],
    );

    render_active_overlay(frame, frame_area, app);
}

fn config_help_text(app: &App, width: u16) -> String {
    if app.config.overlay.is_some() {
        return String::new();
    }

    let hints = match app.config.active_tab {
        ConfigTab::Settings => return settings::help_text(app, width),
        ConfigTab::Plugins => {
            if app.plugins.search_focused
                && crate::app::plugins::search_enabled(app.plugins.active_tab)
            {
                vec![
                    "Type search",
                    "Down list",
                    "Tab/Shift+Tab tabs",
                    "Left/Right switch list",
                    "Backspace erase",
                    "Del clear",
                    "Enter close",
                ]
            } else {
                let mut hints = vec![
                    "Enter actions",
                    "l load folder",
                    "Up/Down select",
                    "Tab/Shift+Tab tabs",
                    "Left/Right switch list",
                ];
                if crate::app::plugins::search_enabled(app.plugins.active_tab) {
                    hints.push("Up search");
                }
                hints.push("r refresh");
                hints
            }
        }
        ConfigTab::Usage => {
            vec!["Up/Down/PgUp/PgDn scroll", "r refresh", "Tab/Shift+Tab tabs", "Enter close"]
        }
        ConfigTab::Mcp => {
            vec!["Enter actions", "Up/Down select", "Tab/Shift+Tab tabs", "r refresh"]
        }
        ConfigTab::Help => {
            vec!["Up/Down select", "Tab/Shift+Tab tabs", "Left/Right switch section", "Enter close"]
        }
        ConfigTab::Status => {
            let mut hints = vec!["Tab/Shift+Tab tabs", "Up/Down scroll"];
            if app.session_runtime.session_id.is_some() {
                hints.extend(["g generate", "r rename"]);
            }
            hints.push("Enter close");
            hints
        }
    };
    common::hint_text(hints, width)
}

fn render_active_overlay(frame: &mut Frame, frame_area: Rect, app: &mut App) {
    if matches!(app.config.overlay, Some(crate::app::config::ConfigOverlayState::Footer(_))) {
        footer::render(frame, frame_area, app);
    } else if app.config.setting_overlay().is_some() {
        settings_overlay::render_setting_overlay(frame, frame_area, app);
    } else if app.config.session_rename_overlay().is_some() {
        settings_overlay::render_session_rename_overlay(frame, frame_area, app);
    } else if app.config.installed_plugin_actions_overlay().is_some() {
        plugin_overlay::render_installed_plugin_actions_overlay(frame, frame_area, app);
    } else if app.config.plugin_install_overlay().is_some() {
        plugin_overlay::render_plugin_install_overlay(frame, frame_area, app);
    } else if app.config.marketplace_actions_overlay().is_some() {
        plugin_overlay::render_marketplace_actions_overlay(frame, frame_area, app);
    } else if app.config.plugin_source_input_overlay().is_some() {
        plugin_overlay::render_plugin_source_overlay(frame, frame_area, app);
    } else if app.config.mcp_details_overlay().is_some() {
        mcp::render_details_overlay(frame, frame_area, app);
    } else if app.config.mcp_callback_url_overlay().is_some() {
        mcp::render_callback_url_overlay(frame, frame_area, app);
    } else if app.config.mcp_auth_redirect_overlay().is_some() {
        mcp::render_auth_redirect_overlay(frame, frame_area, app);
    } else if app.config.mcp_elicitation_overlay().is_some() {
        mcp::render_elicitation_overlay(frame, frame_area, app);
    } else if app.config.confirmation_overlay().is_some() {
        render_confirmation_overlay(frame, frame_area, app);
    }
}

fn render_confirmation_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(overlay) = app.config.confirmation_overlay() else {
        return;
    };
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 9,
            width_percent: 64,
            height_percent: 42,
            preferred_height: 12,
            fullscreen_below: Some((56, 14)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: overlay.title.as_str(),
            subtitle: None,
            help: Some("Up/Down select | Enter confirm | Esc cancel"),
            message: app.config.overlay_message.as_ref(),
        },
    );
    debug_assert!(rendered.rect.width > 0);
    let body_lines = vec![
        Line::from(Span::styled(overlay.body.clone(), Style::default().fg(Color::White))),
        Line::default(),
    ];
    let mut choices = confirmation_overlay_lines(overlay);
    let mut choice_height = common::wrapped_height(choices.clone(), rendered.body_area.width);
    if rendered.body_area.height <= choice_height {
        let mut compact = Line::default();
        for (index, choice) in choices.into_iter().enumerate() {
            if index > 0 {
                compact.spans.push(Span::raw("  "));
            }
            compact.spans.extend(choice.spans);
        }
        choices = vec![compact];
        choice_height = common::wrapped_height(choices.clone(), rendered.body_area.width);
    }
    let body_height = common::wrapped_height(Text::from(body_lines), rendered.body_area.width)
        .min(rendered.body_area.height.saturating_sub(choice_height));
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(body_height), Constraint::Min(choice_height)])
        .split(rendered.body_area);
    render_clipped_plain_text(frame, sections[0], &overlay.body, Style::default().fg(Color::White));
    frame.render_widget(Paragraph::new(choices).wrap(Wrap { trim: false }), sections[1]);
}

fn confirmation_overlay_lines(
    overlay: &crate::app::config::ConfirmationOverlayState,
) -> Vec<Line<'static>> {
    [overlay.cancel_label.as_str(), overlay.confirm_label.as_str()]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let selected = index == overlay.selected_index;
            Line::from(Span::styled(
                format!("{} {label}", common::selection_marker(selected)),
                overlay_line_style(selected, true),
            ))
        })
        .collect()
}

fn render_clipped_plain_text(frame: &mut Frame, area: Rect, text: &str, style: Style) {
    let lines = clipped_plain_text_lines(text, area.width, area.height, style);
    frame.render_widget(Paragraph::new(lines), area);
}

fn clipped_plain_text_lines(
    text: &str,
    viewport_width: u16,
    viewport_height: u16,
    style: Style,
) -> Vec<Line<'static>> {
    if viewport_width == 0 || viewport_height == 0 {
        return Vec::new();
    }
    let wrapped = wrap_plain_text(text, viewport_width);
    let max_height = usize::from(viewport_height);
    let clipped = wrapped.len() > max_height;
    let mut visible = wrapped.into_iter().take(max_height).collect::<Vec<_>>();
    if clipped && let Some(last) = visible.last_mut() {
        *last = line_with_overflow_indicator(last, viewport_width);
    }
    visible.into_iter().map(|line| Line::from(Span::styled(line, style))).collect()
}

fn wrap_plain_text(text: &str, viewport_width: u16) -> Vec<String> {
    let width = usize::from(viewport_width.max(1));
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        let mut current_width = 0usize;
        for ch in raw_line.chars() {
            let ch_width = char_width(ch);
            if current_width > 0 && current_width.saturating_add(ch_width) > width {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(ch);
            current_width = current_width.saturating_add(ch_width);
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn line_with_overflow_indicator(line: &str, viewport_width: u16) -> String {
    const INDICATOR: &str = "...";
    let width = usize::from(viewport_width.max(1));
    if width <= INDICATOR.len() {
        return INDICATOR.chars().take(width).collect();
    }
    let target_width = width.saturating_sub(INDICATOR.len() + 1);
    let mut out = String::new();
    let mut out_width = 0usize;
    for ch in line.chars() {
        let ch_width = char_width(ch);
        if out_width.saturating_add(ch_width) > target_width {
            break;
        }
        out.push(ch);
        out_width = out_width.saturating_add(ch_width);
    }
    out.push(' ');
    out.push_str(INDICATOR);
    out
}

fn char_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

fn render_tab_header(frame: &mut Frame, area: Rect, active_tab: ConfigTab) {
    let labels = ConfigTab::ALL.iter().map(|tab| tab.title().to_owned()).collect::<Vec<_>>();
    let active = ConfigTab::ALL.iter().position(|tab| *tab == active_tab).unwrap_or(0);
    frame.render_widget(
        Paragraph::new(common::tab_line(&labels, active, area.width, common::TabStyle::Primary)),
        area,
    );
}

#[cfg(test)]
mod tests {
    mod overlay_audit;
    mod overlay_support;
    mod plugin_audit;
    mod shell_audit;
    use super::ConfigTab;
    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn buffer_text(buffer: &Buffer) -> String {
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn buffer_lines(buffer: &Buffer) -> Vec<String> {
        let width = usize::from(buffer.area.width);
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect()
    }

    fn installed_plugin_entry(
        id: &str,
        scope: &str,
        project_path: Option<&str>,
    ) -> crate::app::plugins::InstalledPluginEntry {
        crate::app::plugins::InstalledPluginEntry {
            id: id.to_owned(),
            version: Some("1.0.0".to_owned()),
            scope: scope.to_owned(),
            enabled: true,
            installed_at: None,
            last_updated: None,
            project_path: project_path.map(ToOwned::to_owned),
            mcp_server_names: Vec::new(),
        }
    }

    fn render_config_text(width: u16, height: u16, mut app: App) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    fn settings_preview_app() -> App {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.snapshot = Some(serde_json::from_value(serde_json::json!({
            "cwd": app.cwd_raw, "context": "preview", "diagnostics": [], "resolution_sources": [], "provenance": {},
            "categories": [{"id":"general", "label":"General", "short_label":"General"}], "catalog": [
                {"id": "alwaysThinkingEnabled", "label": "Thinking", "description": "Prefer thinking where the model permits it.", "key_path": ["alwaysThinkingEnabled"], "category":"general", "kind": "boolean", "options": [true, false], "allows_custom": false, "writable_scopes": ["user", "project", "local"], "reset": "Reset clears this scope's value and uses the other scopes or Default.", "application": "next_session"},
                {"id": "language", "label": "Language", "description": "Preferred response language or ISO code.", "key_path": ["language"], "category":"general", "kind": "string", "options": [], "allows_custom": true, "writable_scopes": ["user", "project", "local"], "reset": "Reset clears this scope's value and uses the other scopes or Default.", "application": "next_session"}
            ],
            "sources": [{"scope": "user", "path": "settings.json", "status": "valid", "values": [{"id": "alwaysThinkingEnabled", "revision": "r", "value": true}, {"id": "language", "revision": "r"}]}],
            "values": [{"id": "alwaysThinkingEnabled", "value": true, "contributors": ["user"], "policy_restricted": false}]
        })).expect("snapshot"));
        app.config.settings.select("alwaysThinkingEnabled".into());
        app
    }

    /// Renders the rename dialog and returns, for the rows between its field
    /// and the dim hint or help line, each row's text inside the borders and
    /// its colour, and the help line.
    #[test]
    fn unavailable_settings_keep_the_close_action_usable_at_both_sizes() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for (width, height) in [(180, 30), (80, 24)] {
            let mut app = App::test_default();
            app.surface_mode =
                crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            terminal.draw(|frame| super::render(frame, &mut app)).expect("unavailable settings");
            let rendered = buffer_text(terminal.backend().buffer());
            assert!(rendered.contains("unavailable"), "explain the unavailable state: {rendered}");
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            );
            assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
        }
    }

    fn config_list_preview(tab: ConfigTab) -> App {
        use crate::agent::model::{McpServerConnectionStatus, McpServerStatus, SessionId};
        let mut app = settings_preview_app();
        app.config.active_tab = tab;
        app.config.settings.select("language".into());
        app.session_runtime.session_id = Some(SessionId::new("preview-session"));
        app.plugins.installed = (0..9)
            .map(|index| installed_plugin_entry(&format!("p{index}@market"), "user", None))
            .collect();
        app.plugins.installed_selected_index = 8;
        app.mcp.servers = (0..9)
            .map(|index| McpServerStatus {
                name: format!("server-{index}"),
                status: McpServerConnectionStatus::Connected,
                server_info: None,
                error: None,
                config: None,
                scope: Some("user".to_owned()),
                source: None,
                tools: Vec::new(),
            })
            .collect();
        app.config.mcp_selected_server_index = 8;
        app.config.help_dialog.selected = 1;
        app
    }

    #[test]
    fn session_plugin_details_stay_with_the_selected_attachment_at_narrow_widths() {
        for width in [42, 100] {
            let mut app = config_list_preview(ConfigTab::Plugins);
            app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Session;
            app.plugins.session_selected_index = 1;
            app.plugins.session_plugins = ["first", "chosen"]
                .into_iter()
                .map(|name| crate::agent::types::SessionPlugin {
                    name: name.to_owned(),
                    path: format!("C:/plugins/{name}"),
                    source: Some(format!("{name}@inline")),
                    version: Some("1.2.3".to_owned()),
                })
                .collect();
            let text = render_config_text(width, 24, app);
            let rows: Vec<_> = text.lines().collect();
            let selected =
                rows.iter().position(|row| row.contains("Chosen")).expect("selected attachment");
            assert!(rows[selected].contains('›') || rows[selected].contains('>'), "{text}");
            let details = rows[selected + 1..].join("\n");
            for expected in
                ["Source: Session", "C:/plugins/chosen", "chosen@inline", "Version: 1.2.3"]
            {
                assert!(details.contains(expected), "missing {expected}: {text}");
            }
            assert!(!details.contains("C:/plugins/first"), "{text}");
        }
    }
}
