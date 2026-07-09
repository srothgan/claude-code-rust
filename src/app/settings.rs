// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SETTINGS_DIR_NAME: &str = "claude-code-rust";
const SETTINGS_FILE: &str = "settings.json";
const OLD_UPDATE_CACHE_FILE: &str = "update-check.json";
const UPDATE_SOURCE_GITHUB_RELEASE: &str = "github_release";
const GITHUB_RELEASE_BASE_URL: &str = "https://github.com/srothgan/claude-code-rust/releases/tag";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub updates: UpdateSettings,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<UpdateCheckResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_until_unix_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, Deserialize)]
struct OldUpdateCheckCache {
    checked_at_unix_secs: u64,
    latest_version: String,
}

pub fn load_global_settings(current_version: &str) -> Result<LoadedAppSettings, String> {
    let Some(path) = global_settings_path() else {
        return Ok(LoadedAppSettings { path: None, settings: AppSettings::default() });
    };

    let mut settings = load_from_path(&path)?;
    migrate_old_update_cache(
        &path,
        old_update_cache_path().as_deref(),
        &mut settings,
        current_version,
    )?;

    Ok(LoadedAppSettings { path: Some(path), settings })
}

#[cfg(test)]
fn load_global_settings_from_paths(
    settings_path: &Path,
    old_cache_path: Option<&Path>,
    current_version: &str,
) -> Result<AppSettings, String> {
    let mut settings = load_from_path(settings_path)?;
    migrate_old_update_cache(settings_path, old_cache_path, &mut settings, current_version)?;
    Ok(settings)
}

pub fn save_global_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let parent =
        path.parent().ok_or_else(|| "App settings path has no parent directory".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create app settings directory: {err}"))?;

    let temp_path = unique_temp_path(parent, path.file_name().and_then(std::ffi::OsStr::to_str));
    let mut temp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|err| format!("Failed to create app settings temp file: {err}"))?;
    serde_json::to_writer_pretty(&mut temp, settings)
        .map_err(|err| format!("Failed to serialize app settings: {err}"))?;
    temp.write_all(b"\n").map_err(|err| format!("Failed to finalize app settings file: {err}"))?;
    temp.flush().map_err(|err| format!("Failed to flush app settings file: {err}"))?;
    temp.sync_all().map_err(|err| format!("Failed to sync app settings file: {err}"))?;
    drop(temp);
    std::fs::rename(&temp_path, path)
        .map_err(|err| format!("Failed to move app settings file into place: {err}"))?;
    Ok(())
}

pub fn global_settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(SETTINGS_DIR_NAME).join(SETTINGS_FILE))
}

pub fn old_update_cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join(SETTINGS_DIR_NAME).join(OLD_UPDATE_CACHE_FILE))
}

pub fn update_prompt_candidate(
    settings: &AppSettings,
    current_version: &str,
    now_unix_secs: u64,
) -> Option<UpdatePrompt> {
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

fn load_from_path(path: &Path) -> Result<AppSettings, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<AppSettings>(&raw)
            .map_err(|err| format!("Failed to parse app settings: {err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(AppSettings::default()),
        Err(err) => Err(format!("Failed to read app settings: {err}")),
    }
}

fn migrate_old_update_cache(
    settings_path: &Path,
    old_path: Option<&Path>,
    settings: &mut AppSettings,
    current_version: &str,
) -> Result<(), String> {
    if settings.updates.last_result.is_some() {
        return Ok(());
    }
    let Some(old_path) = old_path else {
        return Ok(());
    };
    let raw = match std::fs::read_to_string(old_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            tracing::warn!(
                target: crate::logging::targets::APP_UPDATE,
                event_name = "old_update_cache_read_failed",
                message = "failed to read old update cache during migration",
                outcome = "failure",
                cache_path = %old_path.display(),
                error_message = %err,
            );
            return Ok(());
        }
    };
    let Ok(cache) = serde_json::from_str::<OldUpdateCheckCache>(&raw) else {
        tracing::warn!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "old_update_cache_parse_failed",
            message = "failed to parse old update cache during migration",
            outcome = "failure",
            cache_path = %old_path.display(),
        );
        return Ok(());
    };
    if !super::update_check::is_valid_version(&cache.latest_version) {
        return Ok(());
    }
    let Some(release_url) = release_url_for_version(&cache.latest_version) else {
        return Ok(());
    };

    record_update_check_result(
        settings,
        current_version,
        &cache.latest_version,
        &release_url,
        cache.checked_at_unix_secs,
    );
    save_global_settings(settings_path, settings)?;
    if let Err(err) = std::fs::remove_file(old_path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            target: crate::logging::targets::APP_UPDATE,
            event_name = "old_update_cache_cleanup_failed",
            message = "failed to delete old update cache after migration",
            outcome = "failure",
            cache_path = %old_path.display(),
            error_message = %err,
        );
    }
    Ok(())
}

fn unique_temp_path(parent: &Path, filename_hint: Option<&str>) -> PathBuf {
    let stamp =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |duration| duration.as_nanos());
    let filename = filename_hint.unwrap_or(SETTINGS_FILE);
    parent.join(format!(".{filename}.{stamp}.tmp"))
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
    fn migrates_old_update_cache_into_global_settings_and_deletes_old_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_path = dir.path().join("config").join("settings.json");
        let old_cache_path = dir.path().join("cache").join("update-check.json");
        std::fs::create_dir_all(old_cache_path.parent().expect("old cache parent"))
            .expect("create cache dir");
        std::fs::write(
            &old_cache_path,
            r#"{"checked_at_unix_secs":1783580000,"latest_version":"0.14.0"}"#,
        )
        .expect("write old cache");

        let settings =
            load_global_settings_from_paths(&settings_path, Some(&old_cache_path), "0.13.4")
                .expect("load settings");

        let result = settings.updates.last_result.expect("last result");
        assert_eq!(result.checked_at_unix_secs, 1_783_580_000);
        assert_eq!(result.current_version, "0.13.4");
        assert_eq!(result.latest_version, "0.14.0");
        assert_eq!(
            result.release_url,
            "https://github.com/srothgan/claude-code-rust/releases/tag/v0.14.0"
        );
        assert!(!old_cache_path.exists());
        assert!(settings_path.exists());
    }
}
