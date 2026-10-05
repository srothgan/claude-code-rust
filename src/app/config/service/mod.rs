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
    match conn.inspect_settings(
        session_id.to_string(),
        request_id.clone(),
        app.global_settings_path.as_ref().map(|path| path.to_string_lossy().into_owned()),
    ) {
        Ok(()) => {
            app.config.pending_settings_request =
                Some(super::PendingSettingsRequest::Inspection(request_id));
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
    if app
        .config
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| !snapshot.matches_context(&mutation.context, &app.cwd_raw))
    {
        app.config.set_overlay_error(
            "The settings location changed. Cancel and reopen this editor before saving.",
        );
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
    let pending = super::PendingSettingsRequest::mutation(request_id.clone(), &mutation);
    match conn.mutate_setting(
        session_id.to_string(),
        request_id.clone(),
        mutation,
        app.global_settings_path.as_ref().map(|path| path.to_string_lossy().into_owned()),
    ) {
        Ok(()) => {
            app.config.pending_settings_request = Some(pending);
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
    if request_id.is_none()
        || request_id
            != app
                .config
                .pending_settings_request
                .as_ref()
                .map(super::PendingSettingsRequest::request_id)
    {
        return;
    }
    let Some(pending) = app.config.pending_settings_request.take() else {
        return;
    };
    let owns_editor =
        app.config.setting_overlay().is_some_and(|editor| pending.owns_editor(editor));
    let previous_fast_mode = app.config.fast_mode_effective();
    let previous_gitignore = app.config.respect_gitignore_effective();
    let previous_scroll = app.config.auto_scroll_effective();
    let previous_clock = clock_preferences(app);
    if let Some(snapshot) = result.snapshot {
        if snapshot.cwd != app.cwd_raw {
            return;
        }
        let previous_index = app
            .config
            .snapshot
            .as_ref()
            .map_or(0, |previous| app.config.settings.selected_index(previous));
        app.config.settings.reconcile(&snapshot, previous_index);
        if result.persistence == SettingsPersistence::Conflict
            && let Some(editor) = app.config.setting_overlay_mut()
            && let Some(value) = snapshot.scoped(&editor.setting.id, editor.scope)
            && owns_editor
            && editor.context == snapshot.context
        {
            editor.revision.clone_from(&value.revision);
            if let Some(form) = &mut editor.structured {
                form.path.clear();
                form.selected = 0;
            }
        }
        app.config.snapshot = Some(snapshot);
    }
    if previous_scroll != app.config.auto_scroll_effective() {
        if app.config.auto_scroll_effective() {
            app.chat_render.viewport.resume();
        } else {
            app.chat_render.viewport.pause();
        }
    }
    let clock = clock_preferences(app);
    if previous_clock != clock {
        app.request_chat_purge_replay_rebuild(
            crate::app::ChatPurgeReplayOptions::terminal_history_out_of_sync(),
        );
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
            if owns_editor {
                app.config.clear_overlay();
            }
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
        && owns_editor
    {
        app.config.set_overlay_error(error);
    }
}

fn clock_preferences(app: &App) -> (bool, bool, String, Option<String>) {
    (
        app.config.show_message_timestamps_effective(),
        app.config.show_turn_duration_effective(),
        app.config.time_format().to_owned(),
        app.config.time_zone().map(str::to_owned),
    )
}

#[cfg(test)]
mod tests;
