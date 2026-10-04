// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SETTINGS_DIR_NAME: &str = "claude-code-rust";
const SETTINGS_FILE: &str = "settings.json";
const UPDATE_SOURCE_GITHUB_RELEASE: &str = "github_release";
const GITHUB_RELEASE_BASE_URL: &str = "https://github.com/srothgan/claude-code-rust/releases/tag";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub updates: UpdateSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    // Edited through the config writer; metadata saves must not overwrite it.
    #[serde(default, rename = "autoInstall", skip_serializing)]
    pub auto_install: bool,
    #[serde(default)]
    pub last_result: Option<UpdateCheckResult>,
    #[serde(default)]
    pub skip_until_unix_secs: Option<u64>,
    #[serde(default)]
    pub skipped_version: Option<String>,
    #[serde(default)]
    pub last_install_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub checked_at_unix_secs: u64,
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrompt {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedAppSettings {
    pub path: Option<PathBuf>,
    pub settings: AppSettings,
}

pub fn load_global_settings() -> Result<LoadedAppSettings, String> {
    let Some(path) = global_settings_path() else {
        return Ok(LoadedAppSettings { path: None, settings: AppSettings::default() });
    };
    let settings = load_from_path(&path)?;
    Ok(LoadedAppSettings { path: Some(path), settings })
}

pub fn save_global_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let _lock = crate::json_file::lock(path)
        .map_err(|error| format!("Cannot lock app settings for saving: {error}"))?;
    let mut document = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<serde_json::Value>(&raw)
            .map_err(|error| format!("Invalid app settings: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(format!("Cannot read app settings: {error}")),
    };
    let root =
        document.as_object_mut().ok_or_else(|| "App settings must be a JSON object".to_owned())?;
    let updates = root.entry("updates").or_insert_with(|| serde_json::json!({}));
    let updates = updates
        .as_object_mut()
        .ok_or_else(|| "App update settings must be an object".to_owned())?;
    let serialized = serde_json::to_value(&settings.updates).map_err(|error| error.to_string())?;
    if let serde_json::Value::Object(values) = serialized {
        for (key, value) in values {
            if value.is_null() {
                updates.remove(&key);
            } else {
                updates.insert(key, value);
            }
        }
    }
    crate::json_file::replace(path, &document)
        .map_err(|error| format!("Cannot save app settings: {error}"))
}

pub fn global_settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(SETTINGS_DIR_NAME).join(SETTINGS_FILE))
}

pub fn update_prompt_candidate(
    settings: &AppSettings,
    current_version: &str,
    now_unix_secs: u64,
) -> Option<UpdatePrompt> {
    if settings.updates.auto_install {
        return None;
    }
    let result = settings.updates.last_result.as_ref()?;
    if !super::update_check::is_newer_version(&result.latest_version, current_version) {
        return None;
    }
    if settings.updates.skipped_version.as_deref() == Some(result.latest_version.as_str()) {
        return None;
    }
    if settings.updates.skip_until_unix_secs.is_some_and(|skip_until| skip_until > now_unix_secs) {
        return None;
    }
    let release_url = release_url_for_version(&result.latest_version)?;
    let release_url =
        if result.release_url.trim().is_empty() { release_url } else { result.release_url.clone() };
    Some(UpdatePrompt {
        current_version: current_version.to_owned(),
        latest_version: result.latest_version.clone(),
        release_url,
        last_error: settings.updates.last_install_error.clone(),
    })
}

pub fn record_update_check_result(
    settings: &mut AppSettings,
    current_version: &str,
    latest_version: &str,
    release_url: &str,
    checked_at_unix_secs: u64,
) {
    settings.updates.last_result = Some(UpdateCheckResult {
        checked_at_unix_secs,
        current_version: current_version.to_owned(),
        latest_version: latest_version.to_owned(),
        release_url: release_url.to_owned(),
        source: UPDATE_SOURCE_GITHUB_RELEASE.to_owned(),
    });
    if !super::update_check::is_newer_version(latest_version, current_version) {
        settings.updates.skip_until_unix_secs = None;
        settings.updates.skipped_version = None;
        settings.updates.last_install_error = None;
    }
}

pub fn record_skip_now(settings: &mut AppSettings, now_unix_secs: u64) {
    settings.updates.skip_until_unix_secs = Some(now_unix_secs.saturating_add(6 * 60 * 60));
}

pub fn record_skip_version(settings: &mut AppSettings, latest_version: &str) {
    settings.updates.skipped_version = Some(latest_version.to_owned());
    settings.updates.skip_until_unix_secs = None;
}

pub fn record_install_failure(settings: &mut AppSettings, message: String) {
    settings.updates.last_install_error = Some(message);
}

pub fn clear_install_failure(settings: &mut AppSettings) {
    settings.updates.last_install_error = None;
}

pub fn release_url_for_version(version: &str) -> Option<String> {
    super::update_check::is_valid_version(version)
        .then(|| format!("{GITHUB_RELEASE_BASE_URL}/v{version}"))
}

pub(crate) fn load_from_path(path: &Path) -> Result<AppSettings, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<AppSettings>(&raw)
            .map_err(|err| format!("Failed to parse app settings: {err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AppSettings::default()),
        Err(err) => Err(format!("Failed to read app settings: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_candidate_requires_newer_version() {
        let mut settings = AppSettings::default();
        record_update_check_result(
            &mut settings,
            "0.13.4",
            "0.14.0",
            "https://example.invalid/v0.14.0",
            10,
        );

        assert!(update_prompt_candidate(&settings, "0.13.4", 20).is_some());
        assert!(update_prompt_candidate(&settings, "0.14.0", 20).is_none());
    }

    #[test]
    fn prompt_candidate_respects_skip_until() {
        let mut settings = AppSettings::default();
        record_update_check_result(&mut settings, "0.13.4", "0.14.0", "url", 10);
        record_skip_now(&mut settings, 20);

        assert!(update_prompt_candidate(&settings, "0.13.4", 30).is_none());
        assert!(update_prompt_candidate(&settings, "0.13.4", 22_000).is_some());
    }

    #[test]
    fn prompt_candidate_respects_skipped_version() {
        let mut settings = AppSettings::default();
        record_update_check_result(&mut settings, "0.13.4", "0.14.0", "url", 10);
        record_skip_version(&mut settings, "0.14.0");

        assert!(update_prompt_candidate(&settings, "0.13.4", 20).is_none());
    }

    #[test]
    fn release_url_is_derived_from_version() {
        assert_eq!(
            release_url_for_version("0.14.0").as_deref(),
            Some("https://github.com/srothgan/claude-code-rust/releases/tag/v0.14.0")
        );
    }
    #[test]
    fn updater_save_preserves_personal_preferences_and_unknown_update_fields() {
        let fixture = tempfile::tempdir().expect("tempdir");
        let path = fixture.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"presentation":{"copyFullResponse":true,"showStatusInTerminalTab":false},"updates":{"autoInstall":true,"future":42,"skipped_version":"old"}}"#,
        )
        .expect("fixture");
        save_global_settings(&path, &AppSettings::default()).expect("save");
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read")).expect("JSON");
        assert_eq!(saved["presentation"]["copyFullResponse"], true);
        assert_eq!(saved["presentation"]["showStatusInTerminalTab"], false);
        assert_eq!(saved["updates"]["future"], 42);
        assert_eq!(saved["updates"]["autoInstall"], true);
        assert!(saved["updates"].get("skipped_version").is_none());
    }

    #[test]
    fn updater_save_serializes_with_presentation_edits() {
        let fixture = tempfile::tempdir().expect("tempdir");
        let path = fixture.path().join("settings.json");
        let editor_lock = crate::json_file::lock(&path).expect("editor owns document");
        assert!(save_global_settings(&path, &AppSettings::default()).is_err());
        std::fs::write(&path, r#"{"presentation":{"copyFullResponse":true}}"#)
            .expect("editor saves");
        drop(editor_lock);
        let mut settings = AppSettings::default();
        record_skip_version(&mut settings, "0.15.0");
        save_global_settings(&path, &settings).expect("updater saves after editor");
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
        assert_eq!(saved["presentation"]["copyFullResponse"], true);
        assert_eq!(saved["updates"]["skipped_version"], "0.15.0");
    }
}
