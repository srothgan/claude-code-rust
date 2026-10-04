// SPDX-License-Identifier: Apache-2.0

use super::{
    SettingOverlayState, StructuredEditor, edit,
    structured::{self, DraftKey, FieldInput},
};
use crate::{agent::settings::EditorType, app::App};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

pub(super) fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(mut overlay) = app.config.setting_overlay().cloned() else {
        return false;
    };
    let Some(form) = &mut overlay.structured else {
        return false;
    };
    if form.hook_creation.is_some() {
        return apply_update(app, overlay, key);
    }
    if is_ctrl_key(key, 'f') && form.advanced {
        match serde_json::from_str::<Value>(&overlay.draft) {
            Ok(value)
                if structured::container(&form.schema)
                    && (value.is_object() || value.is_array()) =>
            {
                form.advanced = false;
                form.path.clear();
                form.selected = 0;
            }
            _ => {
                app.config.set_overlay_error("Correct the JSON before returning to the form.");
                return true;
            }
        }
        if let Some(editor) = app.config.setting_overlay_mut() {
            *editor = overlay;
        }
        return true;
    }
    if form.advanced {
        return false;
    }
    if is_ctrl_key(key, 'j') && form.input.is_none() {
        form.advanced = true;
        overlay.cursor = overlay.draft.chars().count();
        if let Some(editor) = app.config.setting_overlay_mut() {
            *editor = overlay;
        }
        return true;
    }
    if form.read_only
        && !matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Enter
                | KeyCode::Esc
                | KeyCode::Home
                | KeyCode::End
        )
    {
        return true;
    }
    if is_ctrl_key(key, 's') && form.input.is_none() {
        edit::confirm_setting(app, false);
        return true;
    }
    if is_ctrl_key(key, 'r') && form.input.is_none() && !form.read_only {
        edit::confirm_setting(app, true);
        return true;
    }
    apply_update(app, overlay, key)
}

fn apply_update(app: &mut App, mut overlay: SettingOverlayState, key: KeyEvent) -> bool {
    let result = update(&mut overlay, key);
    match result {
        Ok(true) => app.config.clear_overlay(),
        Ok(false) => {
            if let Some(editor) = app.config.setting_overlay_mut() {
                *editor = overlay;
            }
            app.config.overlay_message = None;
        }
        Err(error) => app.config.set_overlay_error(error),
    }
    true
}

fn is_ctrl_key(key: KeyEvent, ch: char) -> bool {
    key.code == KeyCode::Char(ch) && key.modifiers == KeyModifiers::CONTROL
}

fn update(overlay: &mut SettingOverlayState, key: KeyEvent) -> Result<bool, String> {
    let mut root: Value =
        serde_json::from_str(&overlay.draft).map_err(|error| error.to_string())?;
    let Some(form) = &mut overlay.structured else {
        return Ok(false);
    };
    if form.hook_creation.is_some() {
        super::hooks::update_creation(form, &mut root, key)?;
    } else if form.input.is_some() {
        update_input(form, &mut root, key)?;
    } else if overlay.setting.id == "hooks" {
        if let Some(close) = super::hooks::handle_collection(form, &mut root, key)? {
            if close {
                return Ok(true);
            }
        } else if update_collection(form, &mut root, key)? {
            return Ok(true);
        }
    } else if update_collection(form, &mut root, key)? {
        return Ok(true);
    }
    overlay.draft = serde_json::to_string_pretty(&root).map_err(|error| error.to_string())?;
    Ok(false)
}

fn update_collection(
    form: &mut StructuredEditor,
    root: &mut Value,
    key: KeyEvent,
) -> Result<bool, String> {
    let rows = form.rows(root);
    let selected = form.selected.min(rows.len().saturating_sub(1));
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            let Some(previous) = form.path.pop() else {
                return Ok(true);
            };
            form.selected = form.rows(root).iter().position(|row| row.key == previous).unwrap_or(0);
        }
        (KeyCode::Up, KeyModifiers::NONE) => form.selected = selected.saturating_sub(1),
        (KeyCode::Down, KeyModifiers::NONE) => {
            form.selected = (selected + 1).min(rows.len().saturating_sub(1));
        }
        (KeyCode::Home, KeyModifiers::NONE) => form.selected = 0,
        (KeyCode::End, KeyModifiers::NONE) => form.selected = rows.len().saturating_sub(1),
        (KeyCode::Char('a'), KeyModifiers::NONE) if !form.read_only => {
            add_item(form, root)?;
        }
        (KeyCode::Delete, KeyModifiers::NONE) if !form.read_only => {
            if let Some(row) = rows.get(selected) {
                let mut path = form.path.clone();
                path.push(row.key.clone());
                structured::patch(root, &path, None)?;
                form.selected = selected.saturating_sub(1);
            }
        }
        (KeyCode::Enter | KeyCode::Char(' '), KeyModifiers::NONE) => {
            if let Some(row) = rows.get(selected) {
                let mut path = form.path.clone();
                path.push(row.key.clone());
                if structured::container(&row.schema) {
                    if row.value.is_none() {
                        if form.read_only {
                            return Ok(false);
                        }
                        structured::patch(root, &path, Some(structured::empty(&row.schema)))?;
                    }
                    form.path = path;
                    form.selected = 0;
                } else if !form.read_only {
                    let draft = row.value.as_ref().map_or_else(
                        || {
                            row.schema
                                .options
                                .first()
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned()
                        },
                        |value| {
                            if row.schema.kind == EditorType::String {
                                value.as_str().unwrap_or_default().to_owned()
                            } else {
                                value.to_string()
                            }
                        },
                    );
                    if row.schema.kind == EditorType::Boolean {
                        structured::patch(
                            root,
                            &path,
                            Some(json!(
                                !row.value.as_ref().and_then(Value::as_bool).unwrap_or(false)
                            )),
                        )?;
                    } else {
                        form.input = Some(FieldInput {
                            path,
                            schema: row.schema.clone(),
                            new_key: false,
                            cursor: draft.chars().count(),
                            draft,
                        });
                    }
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn add_item(form: &mut StructuredEditor, root: &mut Value) -> Result<(), String> {
    let schema = form.schema_at(root).ok_or("This item needs advanced JSON editing.")?;
    if schema.kind == EditorType::Map {
        let draft = schema.keys.first().cloned().unwrap_or_default();
        form.input = Some(FieldInput {
            path: form.path.clone(),
            schema,
            new_key: true,
            cursor: draft.chars().count(),
            draft,
        });
    } else if schema.kind == EditorType::Array {
        let item = schema.item.ok_or("No item editor.")?;
        let index = structured::at(root, &form.path)
            .and_then(Value::as_array)
            .ok_or("Use advanced JSON to correct this list.")?
            .len();
        let mut path = form.path.clone();
        path.push(DraftKey::Index(index));
        if structured::container(&item) {
            structured::patch(root, &path, Some(structured::empty(&item)))?;
            form.path = path;
            form.selected = 0;
        } else {
            form.input = Some(FieldInput {
                path,
                schema: *item,
                new_key: false,
                draft: String::new(),
                cursor: 0,
            });
        }
    }
    Ok(())
}

fn accept_input(form: &mut StructuredEditor, root: &mut Value) -> Result<(), String> {
    let Some(input) = &mut form.input else {
        return Ok(());
    };
    if input.new_key {
        let name = input.draft.clone();
        if name.is_empty() {
            return Err("Enter a name.".into());
        }
        if !input.schema.keys.is_empty() && !input.schema.keys.contains(&name) {
            return Err("Choose a supported event with Left/Right.".into());
        }
        if structured::at(root, &input.path).and_then(|value| value.get(&name)).is_some() {
            return Err("This name already exists. Edit its existing entry.".into());
        }
        let Some(item) = input.schema.item.as_ref() else {
            return Err("No supported item editor.".into());
        };
        let mut path = input.path.clone();
        path.push(DraftKey::Field(name));
        if structured::container(item) {
            structured::patch(root, &path, Some(structured::empty(item)))?;
            form.path = path;
            form.selected = 0;
        } else {
            let schema = *item.clone();
            form.input =
                Some(FieldInput { path, schema, new_key: false, draft: String::new(), cursor: 0 });
            return Ok(());
        }
    } else {
        let selected_key = input.path.last().cloned();
        let value = match input.schema.kind {
            EditorType::String => json!(input.draft),
            _ => serde_json::from_str(&input.draft)
                .map_err(|error| format!("Check the value: {error}"))?,
        };
        structured::patch(root, &input.path, Some(value))?;
        form.input = None;
        if let Some(key) = selected_key {
            form.selected =
                form.rows(root).iter().position(|row| row.key == key).unwrap_or(form.selected);
        }
        return Ok(());
    }
    form.input = None;
    Ok(())
}

fn update_input(
    form: &mut StructuredEditor,
    root: &mut Value,
    key: KeyEvent,
) -> Result<(), String> {
    let Some(input) = &mut form.input else {
        return Ok(());
    };
    if key.code == KeyCode::Esc && key.modifiers.is_empty() {
        form.input = None;
    } else if key.code == KeyCode::Enter && key.modifiers.is_empty() {
        accept_input(form, root)?;
    } else {
        edit_field(input, key);
    }
    Ok(())
}

pub(super) fn edit_field(input: &mut FieldInput, key: KeyEvent) {
    let options = input.options();
    if !options.is_empty() {
        let index = options.iter().position(|value| value.as_str() == Some(input.draft.as_str()));
        let next = match (key.code, key.modifiers) {
            (KeyCode::Up | KeyCode::Left, KeyModifiers::NONE) => Some(
                index
                    .map_or(options.len() - 1, |index| (index + options.len() - 1) % options.len()),
            ),
            (KeyCode::Down | KeyCode::Right, KeyModifiers::NONE) => {
                Some(index.map_or(0, |index| (index + 1) % options.len()))
            }
            (KeyCode::Home, KeyModifiers::NONE) => Some(0),
            (KeyCode::End, KeyModifiers::NONE) => Some(options.len() - 1),
            _ => None,
        };
        if let Some(next) = next {
            options[next].as_str().unwrap_or_default().clone_into(&mut input.draft);
            input.cursor = input.draft.chars().count();
        }
        return;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            input.draft.clear();
            input.cursor = 0;
        }
        (KeyCode::Char('j'), KeyModifiers::CONTROL) if !input.new_key => {
            edit::insert_text_char(Some(input), '\n');
        }
        (KeyCode::Left, KeyModifiers::NONE) => edit::move_text_cursor_left(Some(input)),
        (KeyCode::Right, KeyModifiers::NONE) => edit::move_text_cursor_right(Some(input)),
        (KeyCode::Up, KeyModifiers::NONE) => edit::move_text_line(Some(input), false),
        (KeyCode::Down, KeyModifiers::NONE) => edit::move_text_line(Some(input), true),
        (KeyCode::Home, KeyModifiers::NONE) => edit::set_text_cursor(Some(input), 0),
        (KeyCode::End, KeyModifiers::NONE) => edit::move_text_cursor_to_end(Some(input)),
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            edit::delete_text_before_cursor(Some(input));
        }
        (KeyCode::Delete, KeyModifiers::NONE) => edit::delete_text_at_cursor(Some(input)),
        (KeyCode::Char(ch), modifiers) if edit::accepts_text_input(modifiers) => {
            edit::insert_text_char(Some(input), ch);
        }
        _ => {}
    }
}

pub(super) fn handle_paste(app: &mut App, text: &str) -> bool {
    let Some(form) = app.config.setting_overlay_mut().and_then(|editor| editor.structured.as_mut())
    else {
        return false;
    };
    if form.read_only {
        return true;
    }
    if form.advanced {
        return false;
    }
    let input = if let Some(creation) = &mut form.hook_creation {
        creation.input.as_mut()
    } else {
        form.input.as_mut()
    };
    if let Some(input) = input.filter(|input| input.options().is_empty()) {
        if input.new_key {
            edit::insert_text_str(Some(input), text);
        } else {
            edit::insert_multiline_text(Some(input), text);
        }
    }
    true
}
