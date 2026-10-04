// SPDX-License-Identifier: Apache-2.0

use super::{
    common, input,
    overlay::{OverlayChrome, OverlayLayoutSpec, render_overlay_shell},
    theme,
};
use crate::{
    agent::settings::EditorType,
    app::{
        App,
        config::{
            FieldInput, FormRow, SettingOverlayState, StructuredEditor,
            hooks::{self, CreationStep},
        },
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use serde_json::Value;

pub(super) fn render(frame: &mut Frame, area: Rect, app: &App, overlay: &SettingOverlayState) {
    let Some(form) = &overlay.structured else {
        return;
    };
    let root = serde_json::from_str(&overlay.draft).unwrap_or(Value::Null);
    let wizard = form.hook_creation.as_deref();
    let title = if let Some(wizard) = wizard {
        wizard.title(form)
    } else if overlay.setting.id == "hooks" && !form.path.is_empty() {
        hooks::entries(&root).iter().find(|hook| form.path.starts_with(&hook.path)).map_or_else(
            || overlay.setting.label.clone(),
            |hook| format!("{} / {} hook", hook.event, hook.action),
        )
    } else {
        let path = form.display_breadcrumb(&root);
        if path.is_empty() {
            overlay.setting.label.clone()
        } else {
            format!("{} / {path}", overlay.setting.label)
        }
    };
    let help_width =
        if area.width < 56 { area.width } else { (area.width.saturating_mul(85) / 100).max(56) }
            .saturating_sub(4);
    let help = help_text(app, overlay, &root, help_width);
    let rendered = render_overlay_shell(
        frame,
        area,
        OverlayLayoutSpec {
            min_width: 56,
            min_height: 14,
            width_percent: 85,
            height_percent: 85,
            preferred_height: 32,
            fullscreen_below: Some((56, 14)),
            inner_margin: Margin { vertical: 1, horizontal: 2 },
        },
        OverlayChrome {
            title: &title,
            subtitle: Some(super::settings::scope_label(overlay.scope)),
            help: Some(&help),
            message: app.config.overlay_message.as_ref(),
        },
    );
    let stale = app
        .config
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| !snapshot.matches_context(&overlay.context, &app.cwd_raw));
    let source_height = u16::from(stale);
    let [source, body, details] = Layout::vertical([
        Constraint::Length(source_height),
        Constraint::Min(1),
        Constraint::Length(if rendered.body_area.height >= 12 { 2 } else { 0 }),
    ])
    .areas(rendered.body_area);
    if stale {
        frame.render_widget(
            Paragraph::new("Settings location changed. Cancel and reopen to save.")
                .style(Style::default().fg(theme::STATUS_WARNING)),
            source,
        );
    }
    render_content(frame, body, overlay, &root);
    common::render_details(
        frame,
        details,
        vec![
            Line::styled(
                if form.read_only {
                    "Read-only in this source."
                } else {
                    "Changes stay in your draft until Ctrl+S. Esc cancels the current field or goes back."
                },
                Style::default().fg(theme::DIM),
            ),
            Line::styled(
                "Only this scope is edited. Other scopes can supply additional entries.",
                Style::default().fg(theme::DIM),
            ),
        ],
    );
}

fn render_content(frame: &mut Frame, body: Rect, overlay: &SettingOverlayState, root: &Value) {
    let Some(form) = &overlay.structured else {
        return;
    };
    if let Some(wizard) = form.hook_creation.as_deref() {
        if let Some(field) = &wizard.input {
            render_field(
                frame,
                body,
                field,
                match wizard.step {
                    CreationStep::Event => "Choose when the hook runs.",
                    CreationStep::Action => {
                        "command: shell command; prompt: evaluation prompt; agent: agent task; http: endpoint; mcp_tool: configured server tool."
                    }
                    _ => "",
                },
            );
        } else {
            let lines = vec![
                common::detail_kv("Event", &wizard.event, theme::BTW_ACCENT),
                common::detail_kv(
                    "Matcher",
                    if wizard.matcher.is_empty() { "All" } else { &wizard.matcher },
                    theme::BTW_ACCENT,
                ),
                common::detail_kv(
                    "Action",
                    wizard.handler.get("type").and_then(Value::as_str).unwrap_or_default(),
                    theme::BTW_ACCENT,
                ),
                Line::default(),
                Line::from(
                    "Enter adds this hook to your draft and opens its optional fields. Ctrl+S then saves your changes.",
                ),
            ];
            common::render_details(frame, body, lines);
        }
    } else if let Some(field) = &form.input {
        let label = if field.new_key {
            "New entry name".to_owned()
        } else {
            form.rows(root).iter().find(|row| field.path.last() == Some(&row.key)).map_or_else(
                || {
                    if overlay.setting.id == "hooks" {
                        "Matcher (shared by this group)".into()
                    } else {
                        "New entry".into()
                    }
                },
                |row| row.label.clone(),
            )
        };
        let [label_area, field_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(body);
        frame.render_widget(Paragraph::new(label).style(common::title_style()), label_area);
        render_field(frame, field_area, field, "");
    } else {
        render_rows(frame, body, overlay, root);
    }
}

fn help_text(app: &App, overlay: &SettingOverlayState, root: &Value, width: u16) -> String {
    let Some(form) = &overlay.structured else {
        return String::new();
    };
    let hints = if app.config.pending_settings_request.is_some() {
        "Saving…"
    } else if let Some(wizard) = &form.hook_creation {
        match wizard.step {
            CreationStep::Event => "Up/Down choose | Enter next | Esc cancel adding",
            CreationStep::Action => "Up/Down choose | Enter next | Esc back",
            CreationStep::Matcher => "Enter next (blank: all) | Ctrl+U clear | Esc back",
            CreationStep::Field(_) => {
                "* Required | Enter next | Ctrl+J new line | Ctrl+U clear | Esc back"
            }
            CreationStep::Review => "Enter add to draft | Esc back",
        }
    } else if let Some(input) = &form.input {
        if !input.options().is_empty() {
            "Up/Down choose | Enter accept | Esc cancel field"
        } else if input.new_key {
            "Enter accept name | Ctrl+U clear | Esc cancel field"
        } else {
            "Enter accept field | Ctrl+J new line | Ctrl+U clear | Esc cancel field"
        }
    } else if form.read_only {
        "Enter inspect | Up/Down select | Ctrl+J JSON | Esc back"
    } else if overlay.setting.id == "hooks" && form.path.is_empty() {
        "Enter edit hook | a add hook | m matcher | Del remove hook | Ctrl+S save | Ctrl+J JSON | Esc cancel"
    } else if form
        .schema_at(root)
        .is_some_and(|schema| matches!(schema.kind, EditorType::Array | EditorType::Map))
    {
        "Enter edit | a add entry | Del remove entry | Ctrl+S save | Ctrl+J JSON | Ctrl+R reset setting | Esc back"
    } else {
        "Enter edit field | Del reset field | Ctrl+S save | Ctrl+J JSON | Ctrl+R reset setting | Esc back"
    };
    let mut hints = hints.split(" | ").collect::<Vec<_>>();
    let escape = hints.pop().unwrap_or_default();
    if let Some(index) = hints.iter().position(|hint| *hint == "Ctrl+S save") {
        let save = hints.remove(index);
        hints.insert(2.min(hints.len()), save);
    }
    common::hint_text_with_escape(hints, width, escape)
}

fn render_field(frame: &mut Frame, area: Rect, field: &FieldInput, guidance: &str) {
    let description = field.schema.description.as_deref().unwrap_or(guidance);
    let height = if description.is_empty() {
        0
    } else {
        common::wrapped_height(description, area.width).min(area.height.saturating_sub(2))
    };
    let [description_area, body] =
        Layout::vertical([Constraint::Length(height), Constraint::Min(1)]).areas(area);
    common::render_details(
        frame,
        description_area,
        vec![Line::styled(description.to_owned(), Style::default().fg(theme::DIM))],
    );
    let options = field.options();
    if options.is_empty() {
        input::render_multiline_input_field(frame, body, &field.draft, field.cursor);
    } else {
        let selected =
            options.iter().position(|value| value.as_str() == Some(field.draft.as_str()));
        let [current, choices] = Layout::vertical([
            Constraint::Length(u16::from(selected.is_none())),
            Constraint::Min(1),
        ])
        .areas(body);
        if selected.is_none() {
            frame.render_widget(
                Paragraph::new(format!("Current: {} (choose a supported value)", field.draft)),
                current,
            );
        }
        let blocks = options
            .iter()
            .map(|value| vec![Line::from(value.as_str().unwrap_or_default().to_owned())])
            .collect();
        common::render_selection_blocks(
            frame,
            choices,
            blocks,
            selected.unwrap_or(0),
            selected.is_some(),
        );
    }
}

fn render_rows(frame: &mut Frame, area: Rect, overlay: &SettingOverlayState, root: &Value) {
    let Some(form) = &overlay.structured else {
        return;
    };
    if !form.supports_current_shape(root) {
        common::render_message(
            frame,
            area,
            "This value needs advanced JSON",
            "Press Ctrl+J to inspect or correct the original value.",
        );
        return;
    }
    let schema = form.schema_at(root);
    let rows = form.rows(root);
    let guidance = schema.as_ref().and_then(|schema| schema.description.as_deref()).unwrap_or(
        if form.path.is_empty() {
            &overlay.setting.description
        } else {
            "Select a field to edit it. Unset optional fields use native defaults."
        },
    );
    let guidance = if rows.iter().any(|row| row.required) {
        format!("* Required. {guidance}")
    } else {
        guidance.to_owned()
    };
    let guidance_height =
        common::wrapped_height(guidance.as_str(), area.width).min(area.height.saturating_sub(2));
    let [intro, body] =
        Layout::vertical([Constraint::Length(guidance_height), Constraint::Min(1)]).areas(area);
    common::render_details(
        frame,
        intro,
        vec![Line::styled(guidance, Style::default().fg(theme::DIM))],
    );
    let blocks = if overlay.setting.id == "hooks" && form.path.is_empty() {
        hook_blocks(root)
    } else {
        row_blocks(form, rows)
    };
    let count = blocks.len();
    if count == 0 {
        common::render_message(
            frame,
            body,
            "No entries saved in this scope",
            if form.read_only {
                "Other scopes can still supply entries."
            } else if overlay.setting.id == "hooks" {
                "Press a to add a hook. You will choose its event and action, then enter its values."
            } else {
                "Press a to add an entry. Ctrl+S saves this collection; Esc cancels the draft."
            },
        );
    } else {
        common::render_selection_blocks(
            frame,
            body,
            blocks,
            form.selected.min(count.saturating_sub(1)),
            true,
        );
    }
}

fn hook_blocks(root: &Value) -> Vec<Vec<Line<'static>>> {
    let entries = hooks::entries(root);
    entries
        .into_iter()
        .map(|hook| {
            vec![Line::from(vec![
                Span::styled(hook.event, common::title_style()),
                Span::styled(
                    format!(
                        "  {}  ·  {}",
                        hook.matcher
                            .filter(|matcher| !matcher.is_empty())
                            .unwrap_or_else(|| "All".into()),
                        hook.action
                    ),
                    Style::default().fg(theme::BTW_ACCENT),
                ),
            ])]
        })
        .collect()
}

fn row_blocks(form: &StructuredEditor, rows: Vec<FormRow>) -> Vec<Vec<Line<'static>>> {
    rows.into_iter()
        .map(|row| {
            let value = row.value.as_ref().map_or_else(
                || "Not set".into(),
                |value| {
                    if row.schema.kind == EditorType::Json
                        || form
                            .breadcrumb()
                            .split(" / ")
                            .any(|part| part == "headers" || part == "input")
                    {
                        "Configured".into()
                    } else {
                        crate::app::config::structured_summary(value)
                    }
                },
            );
            vec![Line::from(vec![
                Span::styled(
                    format!("{}{}  ", row.label, if row.required { " *" } else { "" }),
                    common::title_style(),
                ),
                Span::styled(value, Style::default().fg(theme::BTW_ACCENT)),
            ])]
        })
        .collect()
}
