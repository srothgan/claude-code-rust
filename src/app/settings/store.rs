// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use serde_json::{Map, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{DefaultPermissionMode, SettingId, SettingKind, SettingSpec, config_setting};

const SETTINGS_FILENAME: &str = "settings.json";
const CLAUDE_DIR: &str = ".claude";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistedSettingValue {
    Missing,
    Bool(bool),
    String(String),
}

pub struct LoadedSettings {
    pub path: PathBuf,
    pub document: Value,
    pub notice: Option<String>,
}

pub fn load(path_override: Option<&Path>) -> Result<LoadedSettings, String> {
    let path = resolve_path(path_override)?;
    match std::fs::read_to_string(&path) {
        Ok(raw) => parse_document(&path, &raw),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(LoadedSettings { path, document: Value::Object(Map::new()), notice: None })
        }
        Err(err) => Err(format!("Failed to read settings file: {err}")),
    }
}

pub fn save(path: &Path, document: &Value) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "Settings path has no parent directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create settings directory: {err}"))?;

    let normalized = normalized_root(document);
    let temp_path = unique_temp_path(parent);
    let mut temp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|err| format!("Failed to create settings temp file: {err}"))?;
    serde_json::to_writer_pretty(&mut temp, &normalized)
        .map_err(|err| format!("Failed to serialize settings: {err}"))?;
    temp.write_all(b"\n").map_err(|err| format!("Failed to finalize settings file: {err}"))?;
    temp.flush().map_err(|err| format!("Failed to flush settings file: {err}"))?;
    temp.sync_all().map_err(|err| format!("Failed to sync settings file: {err}"))?;
    drop(temp);
    std::fs::rename(&temp_path, path)
        .map_err(|err| format!("Failed to move settings file into place: {err}"))?;
    Ok(())
}

pub fn read_persisted_setting(
    document: &Value,
    spec: &SettingSpec,
) -> Result<PersistedSettingValue, ()> {
    let Some(value) = read_json_path(document, spec.json_path) else {
        return Ok(PersistedSettingValue::Missing);
    };

    match spec.kind {
        SettingKind::Bool => match value {
            Value::Bool(flag) => Ok(PersistedSettingValue::Bool(*flag)),
            _ => Err(()),
        },
        SettingKind::Enum | SettingKind::DynamicEnum => match value {
            Value::String(text) => Ok(PersistedSettingValue::String(text.clone())),
            _ => Err(()),
        },
    }
}

pub fn write_persisted_setting(
    document: &mut Value,
    spec: &SettingSpec,
    value: PersistedSettingValue,
) {
    match value {
        PersistedSettingValue::Missing => remove_json_path(document, spec.json_path),
        PersistedSettingValue::Bool(flag) => {
            set_json_path(document, spec.json_path, Value::Bool(flag));
        }
        PersistedSettingValue::String(text) => {
            set_json_path(document, spec.json_path, Value::String(text));
        }
    }
}

pub fn fast_mode(document: &Value) -> Result<bool, ()> {
    match read_persisted_setting(document, config_setting(SettingId::FastMode))? {
        PersistedSettingValue::Missing => Ok(false),
        PersistedSettingValue::Bool(value) => Ok(value),
        PersistedSettingValue::String(_) => Err(()),
    }
}

pub fn set_fast_mode(document: &mut Value, enabled: bool) {
    write_persisted_setting(
        document,
        config_setting(SettingId::FastMode),
        PersistedSettingValue::Bool(enabled),
    );
}

pub fn model(document: &Value) -> Result<Option<String>, ()> {
    match read_persisted_setting(document, config_setting(SettingId::Model))? {
        PersistedSettingValue::Missing => Ok(None),
        PersistedSettingValue::Bool(_) => Err(()),
        PersistedSettingValue::String(value) => Ok(Some(value)),
    }
}

pub fn set_model(document: &mut Value, model: Option<&str>) {
    let value = model.map_or(PersistedSettingValue::Missing, |model| {
        PersistedSettingValue::String(model.to_owned())
    });
    write_persisted_setting(document, config_setting(SettingId::Model), value);
}

pub fn default_permission_mode(document: &Value) -> Result<DefaultPermissionMode, ()> {
    match read_persisted_setting(document, config_setting(SettingId::DefaultPermissionMode))? {
        PersistedSettingValue::Missing => Ok(DefaultPermissionMode::Default),
        PersistedSettingValue::Bool(_) => Err(()),
        PersistedSettingValue::String(value) => {
            DefaultPermissionMode::from_stored(&value).ok_or(())
        }
    }
}

pub fn set_default_permission_mode(document: &mut Value, mode: DefaultPermissionMode) {
    write_persisted_setting(
        document,
        config_setting(SettingId::DefaultPermissionMode),
        PersistedSettingValue::String(mode.as_stored().to_owned()),
    );
}

fn resolve_path(path_override: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = path_override {
        return Ok(path.to_path_buf());
    }

    let home = dirs::home_dir().ok_or_else(|| "Failed to resolve home directory".to_owned())?;
    Ok(home.join(CLAUDE_DIR).join(SETTINGS_FILENAME))
}

fn parse_document(path: &Path, raw: &str) -> Result<LoadedSettings, String> {
    if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(raw) {
        Ok(LoadedSettings {
            path: path.to_path_buf(),
            document: Value::Object(object),
            notice: None,
        })
    } else {
        let backup = backup_malformed_file(path)?;
        Ok(LoadedSettings {
            path: path.to_path_buf(),
            document: Value::Object(Map::new()),
            notice: Some(format!("Malformed settings file backed up to {}", backup.display())),
        })
    }
}

fn backup_malformed_file(path: &Path) -> Result<PathBuf, String> {
    let stamp =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_secs());
    let backup = path.with_extension(format!("json.bak.{stamp}"));
    std::fs::copy(path, &backup)
        .map_err(|err| format!("Failed to back up malformed settings file: {err}"))?;
    Ok(backup)
}

fn unique_temp_path(parent: &Path) -> PathBuf {
    let stamp =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    parent.join(format!(".{SETTINGS_FILENAME}.{stamp}.tmp"))
}

fn read_json_path<'a>(document: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = document;
    for key in path {
        current = current.as_object()?.get(*key)?;
    }
    Some(current)
}

fn set_json_path(document: &mut Value, path: &[&str], value: Value) {
    let Some((last_key, parents)) = path.split_last() else {
        return;
    };

    let mut current = ensure_object_mut(document);
    for key in parents {
        let child = current.entry((*key).to_owned()).or_insert_with(|| Value::Object(Map::new()));
        if !child.is_object() {
            *child = Value::Object(Map::new());
        }
        current = match child {
            Value::Object(object) => object,
            _ => unreachable!("child must be an object after normalization"),
        };
    }

    current.insert((*last_key).to_owned(), value);
}

fn remove_json_path(document: &mut Value, path: &[&str]) {
    if let Value::Object(object) = document {
        remove_from_object_path(object, path);
    }
}

fn remove_from_object_path(object: &mut Map<String, Value>, path: &[&str]) -> bool {
    let Some((head, tail)) = path.split_first() else {
        return object.is_empty();
    };

    if tail.is_empty() {
        object.remove(*head);
        return object.is_empty();
    }

    let should_remove_child = if let Some(child) = object.get_mut(*head) {
        match child {
            Value::Object(child_object) => remove_from_object_path(child_object, tail),
            _ => true,
        }
    } else {
        false
    };

    if should_remove_child {
        object.remove(*head);
    }

    object.is_empty()
}

fn normalized_root(document: &Value) -> Value {
    match document {
        Value::Object(object) => Value::Object(object.clone()),
        _ => Value::Object(Map::new()),
    }
}

fn ensure_object_mut(document: &mut Value) -> &mut Map<String, Value> {
    if !document.is_object() {
        *document = Value::Object(Map::new());
    }

    match document {
        Value::Object(object) => object,
        _ => unreachable!("document must be an object after normalization"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::config_setting;

    #[test]
    fn load_missing_file_returns_empty_object() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");

        let loaded = load(Some(&path)).expect("load");

        assert_eq!(loaded.path, path);
        assert_eq!(loaded.document, Value::Object(Map::new()));
        assert!(loaded.notice.is_none());
    }

    #[test]
    fn load_malformed_file_creates_backup_and_resets_document() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not-json").expect("write malformed");

        let loaded = load(Some(&path)).expect("load");

        assert_eq!(loaded.document, Value::Object(Map::new()));
        let notice = loaded.notice.expect("backup notice");
        assert!(notice.contains("Malformed settings file backed up"));
        let backups = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|candidate| candidate != &path)
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
    }

    #[test]
    fn save_preserves_unknown_keys_and_updates_fast_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut document = serde_json::json!({
            "fastMode": false,
            "unknown": {
                "keep": true
            }
        });
        set_fast_mode(&mut document, true);

        save(&path, &document).expect("save");
        let reloaded = load(Some(&path)).expect("reload");

        assert_eq!(fast_mode(&reloaded.document), Ok(true));
        assert_eq!(reloaded.document["unknown"]["keep"], Value::Bool(true));
    }

    #[test]
    fn default_permission_mode_defaults_to_default() {
        let document = Value::Object(Map::new());

        assert_eq!(default_permission_mode(&document), Ok(DefaultPermissionMode::Default));
    }

    #[test]
    fn model_defaults_to_none() {
        let document = Value::Object(Map::new());

        assert_eq!(model(&document), Ok(None));
    }

    #[test]
    fn model_rejects_invalid_stored_value() {
        let document = serde_json::json!({
            "model": true
        });

        assert_eq!(model(&document), Err(()));
    }

    #[test]
    fn default_permission_mode_rejects_invalid_stored_value() {
        let document = serde_json::json!({
            "permissions": {
                "defaultMode": "not-a-mode"
            }
        });

        assert_eq!(default_permission_mode(&document), Err(()));
    }

    #[test]
    fn save_preserves_unknown_keys_and_updates_default_permission_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut document = serde_json::json!({
            "permissions": {
                "defaultMode": "default",
                "keep": true
            },
            "unknown": {
                "keep": true
            }
        });
        set_default_permission_mode(&mut document, DefaultPermissionMode::Plan);

        save(&path, &document).expect("save");
        let reloaded = load(Some(&path)).expect("reload");

        assert_eq!(default_permission_mode(&reloaded.document), Ok(DefaultPermissionMode::Plan));
        assert_eq!(reloaded.document["permissions"]["keep"], Value::Bool(true));
        assert_eq!(reloaded.document["unknown"]["keep"], Value::Bool(true));
    }

    #[test]
    fn save_preserves_unknown_keys_and_updates_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut document = serde_json::json!({
            "model": "old-model",
            "unknown": {
                "keep": true
            }
        });
        set_model(&mut document, Some("sonnet"));

        save(&path, &document).expect("save");
        let reloaded = load(Some(&path)).expect("reload");

        assert_eq!(model(&reloaded.document), Ok(Some("sonnet".to_owned())));
        assert_eq!(reloaded.document["unknown"]["keep"], Value::Bool(true));
    }

    #[test]
    fn write_persisted_setting_removes_nested_path_and_prunes_empty_parent() {
        let mut document = serde_json::json!({
            "permissions": {
                "defaultMode": "plan"
            },
            "keep": true
        });

        write_persisted_setting(
            &mut document,
            config_setting(SettingId::DefaultPermissionMode),
            PersistedSettingValue::Missing,
        );

        assert_eq!(
            document,
            serde_json::json!({
                "keep": true
            })
        );
    }

    #[test]
    fn read_persisted_setting_uses_json_path_metadata() {
        let document = serde_json::json!({
            "permissions": {
                "defaultMode": "plan"
            }
        });

        let value =
            read_persisted_setting(&document, config_setting(SettingId::DefaultPermissionMode));

        assert_eq!(value, Ok(PersistedSettingValue::String("plan".to_owned())));
    }
}
