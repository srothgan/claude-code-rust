// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

mod action_overlay;
mod help;
mod input;
mod mcp;
mod overlay;
mod plugin_overlay;
mod plugins;
mod settings;
mod settings_overlay;
mod status;
mod usage;

use crate::app::{App, ConfigTab};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;

use super::theme;
use overlay::{OverlayChrome, OverlayLayoutSpec, overlay_line_style, render_overlay_shell};
pub fn render(frame: &mut Frame, app: &mut App) {
    let frame_area = frame.area();
    if app.config.active_tab == ConfigTab::Settings
        && (frame_area.width < 30 || frame_area.height < 12)
        && (app.config.overlay.is_none() || app.config.setting_overlay().is_some())
    {
        frame.render_widget(
            Paragraph::new(if app.config.setting_overlay().is_some() {
                "Window too small. Resize to view settings.\nEsc cancel"
            } else {
                "Window too small. Resize to view settings.\nEsc close"
            })
            .wrap(Wrap { trim: false }),
            frame_area,
        );
        return;
    }

    let inner = frame_area.inner(Margin { vertical: 1, horizontal: 2 });
    let (message, is_error) = if let Some(error) = app.config.last_error.clone() {
        (error, true)
    } else if let Some(status) = app.config.status_message.clone() {
        (status, false)
    } else {
        (String::new(), false)
    };
    let help = config_help_text(app, inner.width);
    let settings_tab = app.config.active_tab == ConfigTab::Settings;
    let message_height = if settings_tab {
        u16::try_from(
            Paragraph::new(message.as_str())
                .wrap(Wrap { trim: false })
                .line_count(inner.width.max(1)),
        )
        .unwrap_or(u16::MAX)
        .clamp(1, (inner.height / 3).max(1))
    } else {
        1
    };
    let help_height = if settings_tab {
        u16::try_from(
            Paragraph::new(help.as_str()).wrap(Wrap { trim: false }).line_count(inner.width.max(1)),
        )
        .unwrap_or(u16::MAX)
        .clamp(1, 3)
    } else {
        1
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(message_height),
            Constraint::Length(help_height),
        ])
        .split(inner);

    render_tab_header(frame, chunks[0], app.config.active_tab);

    match app.config.active_tab {
        ConfigTab::Settings => settings::render(frame, chunks[1], app),
        ConfigTab::Plugins => plugins::render(frame, chunks[1], app),
        ConfigTab::Status => status::render(frame, chunks[1], app),
        ConfigTab::Usage => usage::render(frame, chunks[1], app),
        ConfigTab::Mcp => mcp::render(frame, chunks[1], app),
        ConfigTab::Help => help::render(frame, chunks[1], app),
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            message,
            Style::default().fg(if is_error { theme::STATUS_ERROR } else { theme::DIM }),
        )))
        .wrap(Wrap { trim: false }),
        chunks[2],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(help, Style::default().fg(theme::RUST_ORANGE))))
            .wrap(Wrap { trim: false }),
        chunks[3],
    );

    render_active_overlay(frame, frame_area, app);
}

fn config_help_text(app: &App, width: u16) -> String {
    if app.config.overlay.is_some() {
        return String::new();
    }

    match app.config.active_tab {
        ConfigTab::Settings => {
            let Some(setting) = app.config.selected_setting() else { return "r refresh | Tab tabs | Esc close".to_owned(); };
            if !setting.writable_at(app.config.selected_scope) {
                if width < 60 { return "Read-only | Esc close\n↑↓ scroll | Tab tabs\ns scope | r refresh".to_owned(); }
                return "Up/Down scroll | Read-only | s scope | r refresh | Tab tabs | Esc close".to_owned();
            }
            if width < 60 { return "Space edit | Esc close\n↑↓ scroll | Tab tabs\ns scope | Del reset".to_owned(); }
            format!("Up/Down scroll | Space edit{} | s scope | Del reset | r refresh | Tab tabs | Esc close", if setting.options.is_empty() { "" } else { " | Left/Right change" })
        }
        ConfigTab::Plugins => {
            if crate::app::plugins::search_enabled(app.plugins.active_tab) {
                if app.plugins.search_focused {
                    "Left/Right switch list | Down list | Type search | Backspace erase | Del clear | Tab next tab | Shift+Tab prev tab | Enter close | Esc close".to_owned()
                } else if matches!(
                    app.plugins.active_tab,
                    crate::app::plugins::PluginsViewTab::Installed
                        | crate::app::plugins::PluginsViewTab::Plugins
                ) {
                    "Left/Right switch list | Up search | Up/Down move | Enter actions | Tab next tab | Shift+Tab prev tab | Esc close".to_owned()
                } else {
                    "Left/Right switch list | Up search | Up/Down move | Tab next tab | Shift+Tab prev tab | Enter close | Esc close".to_owned()
                }
            } else if matches!(
                app.plugins.active_tab,
                crate::app::plugins::PluginsViewTab::Marketplace
            ) {
                "Left/Right switch list | Up/Down move | Enter actions | Tab next tab | Shift+Tab prev tab | Esc close".to_owned()
            } else {
                "Left/Right switch list | Up/Down move | Tab next tab | Shift+Tab prev tab | Enter close | Esc close".to_owned()
            }
        }
        ConfigTab::Usage => {
            "r refresh | Tab next tab | Shift+Tab prev tab | Enter close | Esc close".to_owned()
        }
        ConfigTab::Mcp => {
            "Up/Down select | Enter actions | r refresh | Tab next tab | Shift+Tab prev tab | Esc close"
                .to_owned()
        }
        ConfigTab::Help => {
            "Left/Right switch section | Up/Down scroll | Tab next tab | Shift+Tab prev tab | Enter close | Esc close"
                .to_owned()
        }
        ConfigTab::Status => {
            if app.session_runtime.session_id.is_some() {
                "g generate | r rename | Tab next tab | Shift+Tab prev tab | Enter close | Esc close"
                    .to_owned()
            } else {
                "Tab next tab | Shift+Tab prev tab | Enter close | Esc close".to_owned()
            }
        }
    }
}

fn render_active_overlay(frame: &mut Frame, frame_area: Rect, app: &App) {
    if app.config.setting_overlay().is_some() {
        settings_overlay::render_setting_overlay(frame, frame_area, app);
    } else if app.config.session_rename_overlay().is_some() {
        settings_overlay::render_session_rename_overlay(frame, frame_area, app);
    } else if app.config.installed_plugin_actions_overlay().is_some() {
        plugin_overlay::render_installed_plugin_actions_overlay(frame, frame_area, app);
    } else if app.config.plugin_install_overlay().is_some() {
        plugin_overlay::render_plugin_install_overlay(frame, frame_area, app);
    } else if app.config.marketplace_actions_overlay().is_some() {
        plugin_overlay::render_marketplace_actions_overlay(frame, frame_area, app);
    } else if app.config.add_marketplace_overlay().is_some() {
        plugin_overlay::render_add_marketplace_overlay(frame, frame_area, app);
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
    let body_height = u16::try_from(
        wrapped_text_height(Text::from(body_lines.clone()), rendered.body_area.width)
            .min(usize::from(rendered.body_area.height)),
    )
    .unwrap_or(rendered.body_area.height);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(body_height), Constraint::Min(1)])
        .split(rendered.body_area);
    frame.render_widget(Paragraph::new(body_lines).wrap(Wrap { trim: false }), sections[0]);
    frame.render_widget(
        Paragraph::new(confirmation_overlay_lines(overlay)).wrap(Wrap { trim: false }),
        sections[1],
    );
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
                format!("{} {label}", if selected { ">" } else { " " }),
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

fn wrapped_text_height(text: Text<'static>, viewport_width: u16) -> usize {
    Paragraph::new(text).wrap(Wrap { trim: false }).line_count(viewport_width.max(1)).max(1)
}

fn render_tab_header(frame: &mut Frame, area: Rect, active_tab: ConfigTab) {
    let mut spans = Vec::new();
    for (index, tab) in ConfigTab::ALL.iter().copied().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(theme::DIM)));
        }

        let style = if tab == active_tab {
            Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(tab.title().to_owned(), style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
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

    fn render_config_lines(width: u16, height: u16, mut app: App) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");
        buffer_lines(terminal.backend().buffer())
    }

    #[test]
    fn scoped_settings_and_editor_render_the_received_contract() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for (width, height) in [(80, 24), (140, 40)] {
            let mut app = App::test_default();
            app.session_runtime.current_model = None;
            app.surface_mode =
                crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            app.config.snapshot = Some(serde_json::from_value(serde_json::json!({
                "cwd": app.cwd_raw, "context": "received", "diagnostics": [], "resolution_sources": [], "provenance": {},
                "catalog": [{ "id": "model", "label": "Default model", "description": "Saved model for future sessions", "key_path": ["model"], "kind": "string", "options": ["opus", "sonnet"], "allows_custom": false, "writable_scopes": ["user", "project", "local"], "reset": "Reset removes the saved value here", "application": "next_session" }],
                "sources": [{ "scope": "user", "path": "profile/settings.json", "status": "valid", "values": [{ "id": "model", "revision": "r1", "value": "opus" }] }],
                "values": [{ "id": "model", "value": "opus", "contributors": ["user"], "policy_restricted": false }]
            })).expect("SDK snapshot"));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            terminal.draw(|frame| super::render(frame, &mut app)).expect("draw");
            let text = buffer_text(terminal.backend().buffer());
            assert!(
                text.lines().any(|line| line.contains("Default model") && line.contains("opus"))
            );
            assert!(text.contains("Saved in user: opus"));
            assert!(text.contains("Save in: User (all projects)"));
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            );
            terminal.draw(|frame| super::render(frame, &mut app)).expect("editor");
            let text = buffer_text(terminal.backend().buffer());
            assert!(text.contains("Saved in user: opus"));
            assert!(text.contains("Enter save"));
            assert!(text.contains("Ctrl+R reset"));
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            );
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            );
            assert_eq!(app.config.selected_scope, crate::agent::settings::SettingsScope::Project);
        }
    }

    #[test]
    fn session_rename_overlay_input_uses_placeholder_when_empty() {
        let line = super::input::text_input_line("", 0, "Custom session name").to_string();

        assert!(line.contains("Custom session name"));
    }

    fn settings_preview_app() -> App {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.snapshot = Some(serde_json::from_value(serde_json::json!({
            "cwd": app.cwd_raw, "context": "preview", "diagnostics": [], "resolution_sources": [], "provenance": {},
            "catalog": [
                {"id": "alwaysThinkingEnabled", "label": "Thinking", "description": "Prefer thinking where the model permits it.", "key_path": ["alwaysThinkingEnabled"], "kind": "boolean", "options": [true, false], "allows_custom": false, "writable_scopes": ["user", "project", "local"], "reset": "Reset clears this scope's value and uses the other scopes or Default.", "application": "next_session"},
                {"id": "language", "label": "Language", "description": "Preferred response language or ISO code.", "key_path": ["language"], "kind": "string", "options": [], "allows_custom": true, "writable_scopes": ["user", "project", "local"], "reset": "Reset clears this scope's value and uses the other scopes or Default.", "application": "next_session"}
            ],
            "sources": [{"scope": "user", "path": "settings.json", "status": "valid", "values": [{"id": "alwaysThinkingEnabled", "revision": "r", "value": true}, {"id": "language", "revision": "r"}]}],
            "values": [{"id": "alwaysThinkingEnabled", "value": true, "contributors": ["user"], "policy_restricted": false}]
        })).expect("snapshot"));
        app
    }

    #[test]
    fn settings_render_distinct_values_selection_spacing_and_field_specific_controls() {
        use crate::agent::settings::*;
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::style::Modifier;
        let mut app = settings_preview_app();
        let mut terminal = Terminal::new(TestBackend::new(100, 32)).expect("terminal");
        terminal.draw(|frame| super::render(frame, &mut app)).expect("list");
        let buffer = terminal.backend().buffer();
        let lines = buffer_lines(buffer);
        let thinking_row =
            lines.iter().position(|line| line.contains("Thinking")).expect("thinking");
        let language_row =
            lines.iter().position(|line| line.contains("Language")).expect("language");
        assert_eq!(language_row - thinking_row, 2, "rows need breathing space");
        let thinking_row = u16::try_from(thinking_row).expect("terminal row");
        let language_row = u16::try_from(language_row).expect("terminal row");
        let value_column = (0..buffer.area.width)
            .find(|x| buffer[(*x, thinking_row)].symbol() == "O")
            .expect("On value");
        assert_eq!(buffer[(value_column, thinking_row)].fg, super::theme::BTW_ACCENT);
        assert!(buffer[(value_column, thinking_row)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(value_column, language_row)].symbol(), "D");
        assert_eq!(buffer[(value_column, language_row)].fg, super::theme::DIM);
        assert_eq!(buffer[(value_column, thinking_row)].bg, super::theme::USER_MSG_BG);
        assert!(lines.iter().any(|line| line.contains("1–2 of 2")));
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        terminal.draw(|frame| super::render(frame, &mut app)).expect("choice editor");
        assert!(buffer_text(terminal.backend().buffer()).contains("Value: On"));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        terminal.draw(|frame| super::render(frame, &mut app)).expect("changed choice");
        assert!(buffer_text(terminal.backend().buffer()).contains("Value: Off"));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert_eq!(app.config.selected_setting().expect("selected").kind, SettingKind::String);
        terminal.draw(|frame| super::render(frame, &mut app)).expect("text editor");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Enter a value"));
        assert!(text.contains("Enter save | Ctrl+R reset | Esc cancel"));
        assert!(text.contains("Saved in user: not set"));
        assert!(!text.contains("Options:"), "free text has no enumerated options");
        assert!(!text.contains("Up/Down options"), "free text needs its own controls");
    }

    #[test]
    fn compact_settings_keep_the_selected_value_and_close_control_visible() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for (width, height) in [(30, 16), (50, 20), (80, 24)] {
            let text = render_config_text(width, height, settings_preview_app());
            assert!(
                text.lines().any(|line| line.contains("Thinking") && line.contains("On")),
                "{text}"
            );
            assert!(text.contains("Esc close"), "{text}");
            assert!(text.contains("Space edit"), "{text}");
            assert!(text.contains("of 2"), "{text}");
        }
        let text = render_config_text(28, 10, settings_preview_app());
        assert!(text.contains("Window too small"));
        assert!(text.contains("Esc close"));
        let mut app = settings_preview_app();
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        let mut terminal = Terminal::new(TestBackend::new(28, 10)).expect("terminal");
        terminal.draw(|frame| super::render(frame, &mut app)).expect("small editor");
        assert!(buffer_text(terminal.backend().buffer()).contains("Esc cancel"));
        crate::app::config::handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        terminal.draw(|frame| super::render(frame, &mut app)).expect("small list");
        assert!(buffer_text(terminal.backend().buffer()).contains("Esc close"));
    }

    #[test]
    fn settings_explain_overridden_values_and_offer_read_only_controls() {
        use crate::agent::settings::*;
        let mut app = settings_preview_app();
        let snapshot = app.config.snapshot.as_mut().expect("snapshot");
        snapshot.sources[0].values[1].value = Some(serde_json::json!("German"));
        snapshot.values.push(SavedSetting {
            id: "language".to_owned(),
            value: Some(serde_json::json!("English")),
            contributors: vec!["local".to_owned()],
            policy_restricted: false,
        });
        app.config.selected_setting_index = 1;
        let mut terminal = Terminal::new(TestBackend::new(110, 32)).expect("terminal");
        terminal.draw(|frame| super::render(frame, &mut app)).expect("override");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.lines().any(|line| line.contains("Language") && line.contains("English")));
        assert!(text.contains("Saved in user: German"));
        assert!(text.contains("Another scope overrides this value"));
        let setting = &mut app.config.snapshot.as_mut().expect("snapshot").catalog[1];
        setting.writable_scopes.clear();
        setting.unavailable = Some("Controlled by your organization".to_owned());
        terminal.draw(|frame| super::render(frame, &mut app)).expect("read-only");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Controlled by your organization"));
        assert!(text.contains("Read-only | s scope | r refresh"));
    }

    #[test]
    fn model_picker_keeps_the_selected_option_and_save_controls_visible() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = settings_preview_app();
        app.config.selected_setting_index = 1;
        let snapshot = app.config.snapshot.as_mut().expect("snapshot");
        snapshot.sources[0].values[1].id = "model".to_owned();
        let setting = &mut snapshot.catalog[1];
        setting.id = "model".to_owned();
        setting.key_path = vec!["model".to_owned()];
        setting.label = "Default model".to_owned();
        setting.allows_custom = false;
        setting.options =
            (1..=12).map(|index| serde_json::json!(format!("model-{index}"))).collect();
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        for _ in 0..12 {
            crate::app::config::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            );
        }
        let text = render_config_text(80, 24, app);
        assert!(text.contains("Models  ↑"), "{text}");
        assert!(text.contains("› model-12"));
        assert!(text.contains("Enter save"));
        assert!(text.contains("Ctrl+R reset"));
        assert!(text.contains("Esc cancel"));
    }

    #[test]
    fn setting_save_errors_wrap_without_hiding_the_editor_controls() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = settings_preview_app();
        app.config.selected_setting_index = 1;
        crate::app::config::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        app.config.set_overlay_error("The saved value changed while you were editing. Your draft is still here. Review the latest saved value before trying again.");
        let text = render_config_text(80, 24, app);
        let words = text.replace('│', " ").split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(words.contains("before trying again."), "{text}");
        assert!(text.contains("Enter save"));
        assert!(text.contains("Ctrl+R reset"));
        assert!(text.contains("Esc cancel"));
    }

    #[test]
    fn received_catalog_keeps_the_selected_row_visible_in_a_small_window() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        let mut snapshot = crate::agent::settings::SettingsSnapshot::default();
        for index in 0..22 {
            snapshot.catalog.push(
                serde_json::from_value(serde_json::json!({
                    "id": format!("row-{index}"), "label": format!("Setting {index}"),
                    "description": "Received setting", "key_path": [format!("row-{index}")],
                    "kind": "boolean", "options": [true, false], "writable_scopes": ["user"],
                    "allows_custom": false, "reset": "Reset removes the saved value here",
                    "application": "next_session"
                }))
                .expect("descriptor"),
            );
        }
        app.config.snapshot = Some(snapshot);
        crate::app::config::handle_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::End,
                crossterm::event::KeyModifiers::NONE,
            ),
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).expect("terminal");
        terminal.draw(|frame| super::render(frame, &mut app)).expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.lines().any(|line| line.contains("Setting 21") && line.contains("Default")));
        assert!(text.contains("of 22"));
        assert!(text.contains("Up/Down scroll"));
        assert!(app.config.settings_scroll_offset > 0);
    }

    #[test]
    fn large_terminal_overlay_is_centered_not_fullscreen() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::SessionRename(
            crate::app::config::SessionRenameOverlayState {
                draft: "Review settings".to_owned(),
                cursor: 15,
            },
        ));
        let rendered = render_config_lines(120, 30, app);
        let (row, column) = rendered
            .iter()
            .enumerate()
            .find_map(|(row, line)| line.find("Rename session").map(|column| (row, column)))
            .expect("overlay title");
        assert!(row > 0);
        assert!(column > 0);
    }

    #[test]
    fn small_terminal_confirmation_covers_footer_status() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.status_message = Some("BACKGROUND FOOTER STATUS".to_owned());
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::Confirmation(
            crate::app::config::ConfirmationOverlayState {
                title: "Tiny Confirm".to_owned(),
                body: "Keep the dialog readable on a tiny terminal.".to_owned(),
                confirm_label: "Run".to_owned(),
                cancel_label: "Cancel".to_owned(),
                selected_index: 0,
                action: crate::app::config::ConfirmationAction::MarketplaceRemove,
                previous: None,
            },
        ));
        let rendered = render_config_text(40, 8, app);
        assert!(rendered.contains("Tiny Confirm"));
        assert!(rendered.contains("Up/Down select"));
        assert!(!rendered.contains("BACKGROUND FOOTER STATUS"));
    }

    #[test]
    fn very_short_terminal_confirmation_keeps_title_body_and_help_visible() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::Confirmation(
            crate::app::config::ConfirmationOverlayState {
                title: "Confirm".to_owned(),
                body: "Proceed?".to_owned(),
                confirm_label: "Proceed".to_owned(),
                cancel_label: "Cancel".to_owned(),
                selected_index: 0,
                action: crate::app::config::ConfirmationAction::MarketplaceRemove,
                previous: None,
            },
        ));
        let rendered = render_config_text(32, 6, app);
        assert!(rendered.contains("Confirm"));
        assert!(rendered.contains("Proceed?"));
        assert!(rendered.contains("Up/Down select"));
    }

    #[test]
    fn config_header_is_left_aligned_without_outer_border() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);

        let rendered = render_config_lines(80, 24, app);

        assert!(rendered[0].trim().is_empty());
        assert!(rendered[1].starts_with("  Settings"));
        assert!(!rendered[0].contains("Config"));
        assert!(!rendered[0].contains('┌'));
    }

    #[test]
    fn normal_layout_shows_snapshot_availability() {
        let backend = TestBackend::new(180, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("Settings are currently unavailable."));
    }

    #[test]
    fn compact_layout_shows_snapshot_availability() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());

        assert!(rendered.contains("Settings are currently unavailable."));
    }

    #[test]
    fn status_tab_renders_session_info() {
        let backend = TestBackend::new(100, 32);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Status;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Version"), "missing Version");
        assert!(rendered.contains("cwd"), "missing cwd");
        assert!(rendered.contains("Model"), "missing Model");
    }

    #[test]
    fn status_tab_help_omits_space_edit() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Status;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(!rendered.contains("Space edit"), "Status tab should not show Space edit");
        assert!(rendered.contains("Tab next tab"), "missing tab navigation hint");
        assert!(rendered.contains("Enter close"), "missing Enter close");
    }

    #[test]
    fn usage_tab_help_shows_refresh_hint() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Usage;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("r refresh"));
        assert!(rendered.contains("Shift+Tab prev tab"));
    }

    #[test]
    fn plugins_tab_renders_inventory_shell() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.plugins.installed =
            vec![installed_plugin_entry("frontend-design@claude-plugins-official", "user", None)];
        app.plugins.marketplace = vec![crate::app::plugins::MarketplaceEntry {
            plugin_id: "frontend-design@claude-plugins-official".to_owned(),
            name: "frontend-design".to_owned(),
            description: Some("Create distinctive interfaces".to_owned()),
            marketplace_name: Some("claude-plugins-official".to_owned()),
            version: Some("1.0.0".to_owned()),
            install_count: Some(42),
            source: None,
        }];
        app.plugins.marketplaces = vec![crate::app::plugins::MarketplaceSourceEntry {
            name: "claude-plugins-official".to_owned(),
            source: Some("github".to_owned()),
            repo: Some("anthropics/claude-plugins-official".to_owned()),
        }];

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Installed (1)"));
        assert!(rendered.contains("Plugins (1)"));
        assert!(rendered.contains("Marketplace (1)"));
        assert!(rendered.contains("Search"));
        assert!(rendered.contains("Type to filter this list"));
        assert!(rendered.contains("Frontend Design From Claude Plugins Official"));
        assert!(rendered.contains("SKILL"));
        assert!(rendered.contains("Left/Right switch list"));
    }

    #[test]
    fn plugins_tab_renders_marketplace_plugin_title_and_plugin_id() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Plugins;
        app.plugins.marketplace = vec![crate::app::plugins::MarketplaceEntry {
            plugin_id: "frontend-design@claude-plugins-official".to_owned(),
            name: "frontend-design".to_owned(),
            description: Some("Review UI".to_owned()),
            marketplace_name: Some("claude-plugins-official".to_owned()),
            version: Some("1.0.0".to_owned()),
            install_count: Some(42),
            source: None,
        }];

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Frontend Design"));
        assert!(rendered.contains("Plugin: frontend-design@claude-plugins-official"));
    }

    #[test]
    fn plugins_tab_hides_plugins_from_other_projects() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.cwd_raw = "C:\\work\\project-b".to_owned();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.plugins.installed = vec![
            installed_plugin_entry(
                "other-local@claude-plugins-official",
                "local",
                Some("C:\\work\\project-a"),
            ),
            installed_plugin_entry("user-plugin@claude-plugins-official", "user", None),
            installed_plugin_entry(
                "current-local@claude-plugins-official",
                "local",
                Some("C:\\work\\project-b"),
            ),
        ];

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("User Plugin From Claude Plugins Official"));
        assert!(rendered.contains("Current Local From Claude Plugins Official"));
        assert!(!rendered.contains("Other Local From Claude Plugins Official"));
        assert!(!rendered.contains("Available here"));
        assert!(!rendered.contains("Installed elsewhere"));
    }

    #[test]
    fn plugins_tab_shows_loading_copy_instead_of_empty_state_during_refresh() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.plugins.loading = true;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Loading installed plugins..."));
        assert!(!rendered.contains("No installed plugins found."));
    }

    #[test]
    fn marketplace_tab_renders_configured_heading_and_add_placeholder() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.plugins.active_tab = crate::app::plugins::PluginsViewTab::Marketplace;

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Configured marketplaces"));
        assert!(rendered.contains("Add marketplace"));
    }

    #[test]
    fn installed_plugin_overlay_renders_title_description_and_actions() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::InstalledPluginActions(
            crate::app::config::InstalledPluginActionOverlayState {
                plugin_id: "frontend-design@claude-plugins-official".to_owned(),
                title: "Frontend Design From Claude Plugins Official".to_owned(),
                description: "Create distinctive interfaces".to_owned(),
                scope: "local".to_owned(),
                project_path: Some("C:\\work\\project-a".to_owned()),
                selected_index: 0,
                actions: vec![
                    crate::app::config::InstalledPluginActionKind::Disable,
                    crate::app::config::InstalledPluginActionKind::Update,
                    crate::app::config::InstalledPluginActionKind::Uninstall,
                ],
            },
        ));

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Installed plugin"));
        assert!(rendered.contains("Frontend Design From Claude Plugins Official"));
        assert!(rendered.contains("Create distinctive interfaces"));
        assert!(rendered.contains("Uninstall"));
        assert!(rendered.contains("Up/Down select"));
    }

    #[test]
    fn long_plugin_description_clips_with_indicator_without_hiding_actions() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::InstalledPluginActions(
            crate::app::config::InstalledPluginActionOverlayState {
                plugin_id: "frontend-design@claude-plugins-official".to_owned(),
                title: "Frontend Design From Claude Plugins Official".to_owned(),
                description: "A very long plugin description that keeps going across many words and should be clipped before it can overwrite the action rows below the description area.".to_owned(),
                scope: "user".to_owned(),
                project_path: None,
                selected_index: 2,
                actions: vec![
                    crate::app::config::InstalledPluginActionKind::Disable,
                    crate::app::config::InstalledPluginActionKind::Update,
                    crate::app::config::InstalledPluginActionKind::Uninstall,
                ],
            },
        ));

        let rendered = render_config_text(56, 14, app);

        assert!(rendered.contains("..."));
        assert!(rendered.contains("> Uninstall"));
    }

    #[test]
    fn plugin_install_overlay_renders_title_description_and_actions() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::PluginInstallActions(
            crate::app::config::PluginInstallOverlayState {
                plugin_id: "frontend-design@claude-plugins-official".to_owned(),
                title: "Frontend Design".to_owned(),
                description: "Create distinctive interfaces".to_owned(),
                selected_index: 0,
                actions: vec![
                    crate::app::config::PluginInstallActionKind::User,
                    crate::app::config::PluginInstallActionKind::Project,
                    crate::app::config::PluginInstallActionKind::Local,
                ],
            },
        ));

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Install plugin"));
        assert!(rendered.contains("Frontend Design"));
        assert!(rendered.contains("Create distinctive interfaces"));
        assert!(rendered.contains("Install for project"));
        assert!(rendered.contains("Up/Down select"));
    }

    #[test]
    fn marketplace_actions_overlay_renders_title_description_and_actions() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::MarketplaceActions(
            crate::app::config::MarketplaceActionsOverlayState {
                name: "claude-plugins-official".to_owned(),
                title: "Claude Plugins Official".to_owned(),
                description: "Source: github\nRepo: anthropics/claude-plugins-official".to_owned(),
                selected_index: 0,
                actions: vec![
                    crate::app::config::MarketplaceActionKind::Update,
                    crate::app::config::MarketplaceActionKind::Remove,
                ],
            },
        ));

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Marketplace"));
        assert!(rendered.contains("Claude Plugins Official"));
        assert!(rendered.contains("Source: github"));
        assert!(rendered.contains("Remove"));
    }

    #[test]
    fn add_marketplace_overlay_renders_examples() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::AddMarketplace(
            crate::app::config::AddMarketplaceOverlayState { draft: String::new(), cursor: 0 },
        ));

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Add Marketplace"));
        assert!(rendered.contains("Enter marketplace source:"));
        assert!(rendered.contains("owner/repo (GitHub)"));
        assert!(rendered.contains("Enter add"));
    }

    #[test]
    fn long_marketplace_source_keeps_cursor_suffix_visible() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        let draft =
            "https://example.com/some/really/long/path/to/custom-marketplace.json".to_owned();
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::AddMarketplace(
            crate::app::config::AddMarketplaceOverlayState { cursor: draft.chars().count(), draft },
        ));

        let rendered = render_config_text(64, 18, app);

        assert!(rendered.contains('<'));
        assert!(rendered.contains("marketplace.json"));
    }

    #[test]
    fn mcp_details_overlay_renders_selected_server_details() {
        use std::collections::BTreeMap;

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Mcp;
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::McpDetails(
            crate::app::config::McpDetailsOverlayState {
                server_name: "filesystem".to_owned(),
                selected_index: 0,
            },
        ));
        app.mcp.servers = vec![crate::agent::model::McpServerStatus {
            name: "filesystem".to_owned(),
            status: crate::agent::model::McpServerConnectionStatus::Connected,
            server_info: Some(crate::agent::model::McpServerInfo {
                name: "Filesystem".to_owned(),
                version: "1.2.3".to_owned(),
            }),
            error: None,
            config: Some(crate::agent::model::McpServerStatusConfig::Stdio {
                command: "npx".to_owned(),
                args: vec!["@modelcontextprotocol/server-filesystem".to_owned()],
                env: BTreeMap::new(),
                timeout: Some(5000),
                request_timeout_ms: Some(30000),
                always_load: Some(true),
            }),
            scope: Some("project".to_owned()),
            source: None,
            tools: vec![crate::agent::model::McpTool {
                name: "read_file".to_owned(),
                description: Some("Read a file".to_owned()),
                annotations: Some(crate::agent::model::McpToolAnnotations {
                    read_only: Some(true),
                    destructive: Some(false),
                    open_world: Some(false),
                }),
            }],
        }];

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("filesystem"));
        assert!(rendered.contains("project"));
        assert!(rendered.contains("stdio"));
        assert!(rendered.contains("Reconnect server"));
        assert!(rendered.contains("Disable server"));
        assert!(rendered.contains("Enter run"));
    }

    #[test]
    fn mcp_overlay_message_renders_inside_overlay() {
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Mcp;
        app.config.overlay = Some(crate::app::config::ConfigOverlayState::McpCallbackUrl(
            crate::app::config::McpCallbackUrlOverlayState {
                server_name: "filesystem".to_owned(),
                draft: String::new(),
                cursor: 0,
            },
        ));
        app.config.set_overlay_error("Callback URL cannot be empty");
        app.config.last_error = Some("BACKGROUND ERROR".to_owned());

        let rendered = render_config_text(80, 16, app);

        assert!(rendered.contains("Callback URL cannot be empty"));
        assert!(!rendered.contains("BACKGROUND ERROR"));
    }

    #[test]
    fn status_tab_help_shows_generate_and_rename_when_session_is_active() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Status;
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("session-1"));

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("g generate"));
        assert!(rendered.contains("r rename"));
    }

    #[test]
    fn config_footer_renders_status_message_when_present() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = App::test_default();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.status_message = Some("Renaming session...".to_owned());

        terminal
            .draw(|frame| {
                super::render(frame, &mut app);
            })
            .expect("draw");

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains("Renaming session..."));
    }
}
