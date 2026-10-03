// SPDX-License-Identifier: Apache-2.0

use crate::agent::settings::{
    SettingsApplication, SettingsMutation, SettingsOperation, SettingsPersistence, SettingsResult,
};
use crate::app::App;

pub(crate) fn request_settings(app: &mut App) {
    let (Some(conn), Some(session_id)) =
        (app.session_runtime.conn.as_ref(), app.session_runtime.session_id.as_ref())
    else {
        if app.status != crate::app::AppStatus::Connecting {
            app.config.last_error = Some("Settings are currently unavailable.".to_owned());
        }
        return;
    };
    if app.config.pending_settings_request.is_some() {
        return;
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    match conn.inspect_settings(session_id.to_string(), request_id.clone()) {
        Ok(()) => {
            app.config.pending_settings_request = Some(request_id);
            app.config.last_error = None;
            if app.config.snapshot.is_some() {
                app.config.status_message = Some("Refreshing settings...".to_owned());
            }
        }
        Err(error) => app.config.last_error = Some(error.to_string()),
    }
}

pub(crate) fn send_mutation(app: &mut App, mutation: SettingsMutation) {
    if app.config.pending_settings_request.is_some() {
        return;
    }
    let (Some(conn), Some(session_id)) =
        (app.session_runtime.conn.as_ref(), app.session_runtime.session_id.as_ref())
    else {
        app.config
            .set_overlay_error("Settings are currently unavailable. Your change was not saved.");
        return;
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    match conn.mutate_setting(session_id.to_string(), request_id.clone(), mutation) {
        Ok(()) => {
            app.config.pending_settings_request = Some(request_id);
            app.config.last_error = None;
            app.config.status_message = Some("Saving setting…".to_owned());
        }
        Err(error) => app.config.set_overlay_error(error.to_string()),
    }
}

pub(crate) fn reset_selected(app: &mut App) {
    let Some(snapshot) = app.config.snapshot.as_ref() else {
        return;
    };
    let Some(setting) = app.config.selected_setting() else {
        return;
    };
    if !setting.writable_at(app.config.selected_scope) {
        return;
    }
    let Some(value) = snapshot.scoped(&setting.id, app.config.selected_scope) else {
        return;
    };
    let mutation = SettingsMutation {
        context: snapshot.context.clone(),
        id: setting.id.clone(),
        scope: app.config.selected_scope,
        expected_revision: value.revision.clone(),
        operation: SettingsOperation::Remove,
        value: None,
    };
    send_mutation(app, mutation);
}

pub(crate) fn apply_settings_result(
    app: &mut App,
    request_id: Option<&str>,
    result: SettingsResult,
) {
    if request_id.is_none() || request_id != app.config.pending_settings_request.as_deref() {
        return;
    }
    app.config.pending_settings_request = None;
    let previous_fast_mode = app.config.fast_mode_effective();
    let previous_gitignore = app.config.respect_gitignore_effective();
    if let Some(snapshot) = result.snapshot {
        if snapshot.cwd != app.cwd_raw {
            return;
        }
        app.config.selected_setting_index =
            app.config.selected_setting_index.min(snapshot.catalog.len().saturating_sub(1));
        if result.persistence == SettingsPersistence::Conflict
            && let Some(editor) = app.config.setting_overlay_mut()
            && let Some(value) = snapshot.scoped(&editor.id, editor.scope)
        {
            editor.context.clone_from(&snapshot.context);
            editor.revision.clone_from(&value.revision);
        }
        app.config.snapshot = Some(snapshot);
    }
    if previous_gitignore != app.config.respect_gitignore_effective() {
        crate::app::file_index::restart(app);
    }
    if !previous_fast_mode && app.config.fast_mode_effective() {
        crate::app::events::maybe_emit_fast_mode_disabled_notice(app, None);
    }
    let operation_error = result.error.clone();
    app.config.last_error = result.error;
    app.config.status_message = match result.persistence {
        SettingsPersistence::Saved | SettingsPersistence::Unchanged => {
            app.config.clear_overlay();
            Some(if result.persistence == SettingsPersistence::Unchanged {
                "No changes to save.".to_owned()
            } else {
                match result.application {
                    SettingsApplication::Host => "Saved and applied.",
                    SettingsApplication::NextSession => "Saved. Changes apply to the next session.",
                    SettingsApplication::Blocked => "Saved. The change could not be applied.",
                }
                .to_owned()
            })
        }
        _ => None,
    };
    if app.config.setting_overlay().is_some()
        && let Some(error) = operation_error
    {
        app.config.set_overlay_error(error);
    }
}

#[cfg(test)]
mod tests;
