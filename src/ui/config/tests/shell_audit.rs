// SPDX-License-Identifier: Apache-2.0
use super::{buffer_lines, config_list_preview, settings_preview_app};
use crate::agent::{model, settings::*};
use crate::app::{App, ConfigHelpSection, ConfigTab, SurfaceMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;

fn key(app: &mut App, code: KeyCode) {
    crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}
fn line(buffer: &Buffer, needle: &str) -> (u16, String) {
    buffer_lines(buffer).into_iter().enumerate().find(|(_, s)| s.contains(needle)).map_or_else(
        || panic!("missing {needle}:\n{}", buffer_lines(buffer).join("\n")),
        |(i, s)| (u16::try_from(i).unwrap(), s),
    )
}
fn cell_column(text: &str, needle: &str) -> u16 {
    // These fixtures contain only single-cell characters, including the selection marker.
    u16::try_from(text[..text.find(needle).unwrap()].chars().count()).unwrap()
}
fn footer(buffer: &Buffer) -> String {
    let rows = buffer_lines(buffer);
    rows[rows.len().saturating_sub(4)..].join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}
fn modal(buffer: &Buffer) -> Rect {
    let (top, left) = (0..buffer.area.height)
        .find_map(|y| {
            let text = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>();
            if text.contains("Search") {
                return None;
            }
            (0..buffer.area.width).find(|&x| buffer[(x, y)].symbol() == "┌").map(|x| (y, x))
        })
        .unwrap();
    let right = (left + 1..buffer.area.width).find(|&x| buffer[(x, top)].symbol() == "┐").unwrap();
    let bottom =
        (top + 1..buffer.area.height).find(|&y| buffer[(left, y)].symbol() == "└").unwrap();
    Rect::new(left + 1, top + 1, right - left - 1, bottom - top - 1)
}
fn region(buffer: &Buffer, rect: Rect) -> Vec<String> {
    (rect.y..rect.bottom())
        .map(|y| (rect.x..rect.right()).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}
fn configured_language(app: &mut App) {
    let snapshot = app.config.snapshot.as_mut().unwrap();
    snapshot.sources[0].values.iter_mut().find(|v| v.id == "language").unwrap().value =
        Some(json!("German"));
    snapshot.sources.push(serde_json::from_value(json!({"scope":"project","path":"project.json","status":"valid","values":[{"id":"language","revision":"project-r2","value":"Greek"}]})).unwrap());
    snapshot.values.push(SavedSetting {
        id: "language".into(),
        value: Some(json!("Greek")),
        contributors: vec!["project".into()],
        policy_restricted: false,
    });
}
fn assert_selected_row(buffer: &Buffer, label: &str, value: &str) {
    let (_, row) = line(buffer, label);
    assert!(row.contains('›') && row.contains(value), "{row}");
}
fn field_and_help(buffer: &Buffer) -> (String, String) {
    let rows = region(buffer, modal(buffer));
    let description = rows.iter().position(|r| r.contains("Description:")).unwrap();
    let field = rows[..description].iter().skip(1).map(|s| s.trim()).collect::<Vec<_>>().join(" ");
    let help = rows[rows.len().saturating_sub(3)..]
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (field, help)
}

#[test]
fn settings_associate_default_and_configured_values_with_selection_and_the_captured_editor() {
    let mut app = settings_preview_app();
    let buffer = draw(&mut app, 100, 32);
    assert_selected_row(&buffer, "Thinking", "On");
    assert!(line(&buffer, "Language").1.contains("Default"));
    assert!(!line(&buffer, "Language").1.contains('›'));
    configured_language(&mut app);
    key(&mut app, KeyCode::Down);
    let buffer = draw(&mut app, 100, 32);
    assert_selected_row(&buffer, "Language", "Greek");
    assert!(!line(&buffer, "Thinking").1.contains('›'));
    key(&mut app, KeyCode::Char(' '));
    for (width, height) in [(80, 24), (140, 40)] {
        let buffer = draw(&mut app, width, height);
        let rows = region(&buffer, modal(&buffer));
        assert!(rows[0].contains("User"));
        let (field, help) = field_and_help(&buffer);
        assert_eq!(field.trim(), "German");
        assert!(!field.contains("Greek"));
        for control in ["Enter save", "Ctrl+R reset", "Esc cancel"] {
            assert!(help.contains(control), "{help}");
        }
        assert_eq!(app.config.setting_overlay().unwrap().scope, SettingsScope::User);
    }
}

#[test]
fn compact_selection_and_tiny_editor_cancel_preserve_saved_values_and_return_to_chat() {
    let mut app = settings_preview_app();
    configured_language(&mut app);
    for (width, height) in [(30, 16), (50, 20), (80, 24)] {
        let buffer = draw(&mut app, width, height);
        assert_selected_row(&buffer, "Thinking", "On");
        let help = footer(&buffer);
        assert!(help.contains("Space change") && help.contains("Esc close"), "{help}");
    }
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char(' '));
    crate::app::handle_terminal_event(&mut app, Event::Paste(" unsaved draft".into()));
    let saved = app.config.saved_value("language").cloned();
    let draft = app.config.setting_overlay().unwrap().draft.clone();
    assert_eq!(draft, "German unsaved draft");
    let buffer = draw(&mut app, 28, 10);
    let text = buffer_lines(&buffer).join(" ");
    assert!(text.contains("Resize") && text.contains("Esc cancel"));
    assert_eq!(app.config.setting_overlay().unwrap().draft, draft);
    key(&mut app, KeyCode::Esc);
    assert!(app.config.overlay.is_none());
    assert_ne!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.config.saved_value("language"), saved.as_ref());
    let buffer = draw(&mut app, 28, 10);
    assert!(buffer_lines(&buffer).join(" ").contains("Esc close"));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
}

#[test]
fn read_only_scalar_disables_its_own_label_value_and_edit_controls_beside_a_writable_neighbor() {
    let mut app = settings_preview_app();
    configured_language(&mut app);
    let setting = app
        .config
        .snapshot
        .as_mut()
        .unwrap()
        .catalog
        .iter_mut()
        .find(|s| s.id == "alwaysThinkingEnabled")
        .unwrap();
    setting.writable_scopes.clear();
    setting.unavailable = Some("Independent organization restriction".into());
    let buffer = draw(&mut app, 110, 32);
    assert_selected_row(&buffer, "Thinking", "On");
    let (disabled_y, disabled) = line(&buffer, "Thinking");
    let (enabled_y, enabled) = line(&buffer, "Language");
    for (left, right) in [("Thinking", "Language"), ("On", "Greek")] {
        let dx = cell_column(&disabled, left);
        let ex = cell_column(&enabled, right);
        assert_ne!(
            buffer[(dx, disabled_y)].fg,
            buffer[(ex, enabled_y)].fg,
            "disabled and writable {left}/{right} must be visually distinguishable"
        );
    }
    let (description_y, _) = line(&buffer, "Description:");
    let (reason_y, _) = line(&buffer, "Independent organization restriction");
    assert!(reason_y > description_y);
    let help = footer(&buffer);
    assert!(help.contains("Read-only") && help.contains("Esc close"));
    assert!(
        !help.contains("Space edit")
            && !help.contains("Space change")
            && !help.contains("Del reset")
    );
    key(&mut app, KeyCode::Down);
    let buffer = draw(&mut app, 110, 32);
    assert_selected_row(&buffer, "Language", "Greek");
    assert!(footer(&buffer).contains("Space edit"));
}

#[test]
fn wrapped_save_errors_remain_inside_the_editor_beside_the_draft_and_usable_controls() {
    let mut app = settings_preview_app();
    configured_language(&mut app);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char(' '));
    crate::app::handle_terminal_event(&mut app, Event::Paste(" changed".into()));
    let error = "SAVE_ERROR_START: the independent file revision is newer. Review the scoped change before retrying. SAVE_ERROR_END";
    app.config.set_overlay_error(error);
    for (width, height) in [(60, 18), (100, 30)] {
        let buffer = draw(&mut app, width, height);
        let rows = region(&buffer, modal(&buffer));
        let (field, help) = field_and_help(&buffer);
        assert_eq!(field.trim(), "German changed");
        let start = rows.iter().position(|r| r.contains("SAVE_ERROR_START:")).unwrap();
        let end = rows.iter().position(|r| r.contains("Enter save")).unwrap();
        assert!(start < end);
        let message = rows[start..end].iter().map(|s| s.trim()).collect::<Vec<_>>().join(" ");
        assert_eq!(message.trim(), error);
        for control in ["Enter save", "Ctrl+R reset", "Esc cancel"] {
            assert!(help.contains(control), "{help}");
        }
    }
}

fn long_settings(app: &mut App) {
    let snapshot = app.config.snapshot.as_mut().unwrap();
    for number in 0..22 {
        snapshot.catalog.push(serde_json::from_value(json!({"id":format!("entry-{number}"),"label":if number==21 {"Final preference".to_owned()} else {format!("Earlier {number}")},"description":"Independent received control","key_path":[format!("entry-{number}")],"category":"general","kind":"boolean","options":[true,false],"allows_custom":false,"writable_scopes":["user"],"reset":"Remove this saved value","application":"next_session"})).unwrap());
    }
    snapshot.values.push(SavedSetting {
        id: "entry-21".into(),
        value: Some(json!(true)),
        contributors: vec!["user".into()],
        policy_restricted: false,
    });
    snapshot.sources[0].values.push(ScopedSetting {
        id: "entry-21".into(),
        value: Some(json!(true)),
        revision: "late-r1".into(),
    });
    key(app, KeyCode::End);
}
fn assert_selected_block(buffer: &Buffer, target: &str) {
    let marker = buffer.content.iter().find(|c| c.symbol() == "›").unwrap();
    let (y, text) = line(buffer, target);
    let x = cell_column(&text, target);
    assert_eq!(buffer[(x, y)].bg, marker.bg, "target must belong to the selected block");
    assert_ne!(marker.bg, buffer[(0, 0)].bg);
}

// Each pane owns different content/controls; only the shared layout sizes repeat.
#[allow(clippy::too_many_lines)]
#[test]
fn config_panes_keep_their_selected_content_and_available_controls_visible_after_resize() {
    for tab in [
        ConfigTab::Settings,
        ConfigTab::Plugins,
        ConfigTab::Mcp,
        ConfigTab::Help,
        ConfigTab::Usage,
        ConfigTab::Status,
    ] {
        let mut app = config_list_preview(tab);
        match tab {
            ConfigTab::Settings => long_settings(&mut app),
            ConfigTab::Help => {
                app.config.help_section = ConfigHelpSection::Subagents;
                app.sdk_inventory.available_agents = vec![
                    model::AvailableAgent::new("alpha", "Alpha description"),
                    model::AvailableAgent::new("target", "Target description"),
                ];
                app.config.help_dialog.selected = 1;
            }
            ConfigTab::Status => {
                app.session_runtime.session_title = Some("STATUS_TARGET_TITLE".into());
            }
            _ => {}
        }
        for (width, height) in [(80, 24), (40, 16)] {
            let buffer = draw(&mut app, width, height);
            let help = footer(&buffer);
            assert!(
                help.contains("Esc close") && help.contains("Tab/Shift+Tab tabs"),
                "{tab:?}: {help}"
            );
            match tab {
                ConfigTab::Settings => {
                    assert_selected_row(&buffer, "Final preference", "On");
                    assert!(help.contains("Space change"));
                }
                ConfigTab::Plugins => {
                    assert_selected_block(&buffer, "P8");
                    assert!(help.contains("Enter actions"));
                }
                ConfigTab::Mcp => {
                    assert_selected_block(&buffer, "server-8");
                    assert!(help.contains("Enter actions"));
                }
                ConfigTab::Help => {
                    assert_selected_row(&buffer, "&target", "");
                    assert!(line(&buffer, "Target description").0 > line(&buffer, "Subagents").0);
                    assert!(help.contains("Up/Down select"));
                }
                ConfigTab::Usage => {
                    assert!(line(&buffer, "No usage snapshot").0 > 0);
                    assert!(help.contains("r refresh"));
                }
                ConfigTab::Status => {
                    assert!(line(&buffer, "Session name").1.contains("STATUS_TARGET_TITLE"));
                    assert!(help.contains("g generate") && help.contains("r rename"));
                }
            }
        }
    }
    let mut app = config_list_preview(ConfigTab::Status);
    app.session_runtime.session_id = None;
    let buffer = draw(&mut app, 80, 24);
    let help = footer(&buffer);
    assert!(!help.contains("g generate") && !help.contains("r rename"));
    assert!(help.contains("Esc close"));
}

#[test]
fn shared_footer_preserves_complete_wrapped_feedback_and_tiny_shell_can_close() {
    let mut app = config_list_preview(ConfigTab::Mcp);
    let message = "FEEDBACK_START: changes were recorded in the selected source. Refresh to inspect the latest values. FEEDBACK_END";
    app.config.status_message = Some(message.into());
    for (width, height) in [(80, 24), (40, 16)] {
        let buffer = draw(&mut app, width, height);
        let rows = buffer_lines(&buffer);
        let start = rows.iter().position(|r| r.contains("FEEDBACK_START:")).unwrap();
        let end = rows.iter().position(|r| r.contains("Enter actions")).unwrap();
        assert!(start < end);
        let actual = rows[start..end].iter().map(|r| r.trim()).collect::<Vec<_>>().join(" ");
        assert_eq!(actual.trim(), message);
        assert!(footer(&buffer).contains("Esc close"));
    }
    let buffer = draw(&mut app, 28, 10);
    let text = buffer_lines(&buffer).join(" ");
    assert!(text.contains("Resize") && text.contains("Esc close"));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
}
