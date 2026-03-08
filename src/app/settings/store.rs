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

use super::DefaultPermissionMode;

const SETTINGS_FILENAME: &str = "settings.json";
const CLAUDE_DIR: &str = ".claude";
const FAST_MODE_KEY: &str = "fastMode";
const MODEL_KEY: &str = "model";
const PERMISSIONS_KEY: &str = "permissions";
const DEFAULT_MODE_KEY: &str = "defaultMode";

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

pub fn fast_mode(document: &Value) -> Result<bool, ()> {
    match document.as_object().and_then(|object| object.get(FAST_MODE_KEY)) {
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(()),
        None => Ok(false),
    }
}

pub fn set_fast_mode(document: &mut Value, enabled: bool) {
    ensure_object_mut(document).insert(FAST_MODE_KEY.to_owned(), Value::Bool(enabled));
}

pub fn model(document: &Value) -> Result<Option<String>, ()> {
    match document.as_object().and_then(|object| object.get(MODEL_KEY)) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(()),
        None => Ok(None),
    }
}

pub fn set_model(document: &mut Value, model: Option<&str>) {
    let object = ensure_object_mut(document);
    if let Some(model) = model {
        object.insert(MODEL_KEY.to_owned(), Value::String(model.to_owned()));
    } else {
        object.remove(MODEL_KEY);
    }
}

pub fn default_permission_mode(document: &Value) -> Result<DefaultPermissionMode, ()> {
    let Some(root) = document.as_object() else {
        return Ok(DefaultPermissionMode::Default);
    };
    match root.get(PERMISSIONS_KEY) {
        None => Ok(DefaultPermissionMode::Default),
        Some(Value::Object(permissions)) => match permissions.get(DEFAULT_MODE_KEY) {
            Some(Value::String(value)) => DefaultPermissionMode::from_stored(value).ok_or(()),
            Some(_) => Err(()),
            None => Ok(DefaultPermissionMode::Default),
        },
        Some(_) => Err(()),
    }
}

pub fn set_default_permission_mode(document: &mut Value, mode: DefaultPermissionMode) {
    ensure_child_object_mut(document, PERMISSIONS_KEY)
        .insert(DEFAULT_MODE_KEY.to_owned(), Value::String(mode.as_stored().to_owned()));
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

fn ensure_child_object_mut<'a>(document: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    let object = ensure_object_mut(document);
    let child = object.entry(key.to_owned()).or_insert_with(|| Value::Object(Map::new()));
    if !child.is_object() {
        *child = Value::Object(Map::new());
    }

    match child {
        Value::Object(object) => object,
        _ => unreachable!("child must be an object after normalization"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
