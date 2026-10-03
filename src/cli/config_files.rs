// SPDX-License-Identifier: Apache-2.0

//! Raw offline inspection only. Effective settings are owned by the SDK bridge.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
const SETTINGS_FILENAME: &str = "settings.json";
const LOCAL_SETTINGS_FILENAME: &str = "settings.local.json";
const CLAUDE_DIR: &str = ".claude";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SettingsPaths {
    pub settings: PathBuf,
    pub project_settings: PathBuf,
    pub local_settings: PathBuf,
    pub preferences: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InspectedConfigFileKind {
    Settings,
    ProjectSettings,
    LocalSettings,
    Preferences,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InspectedConfigFileStatus {
    Missing,
    Valid,
    Invalid,
    Unreadable,
    NotFile,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InspectedConfigFile {
    pub kind: InspectedConfigFileKind,
    pub label: &'static str,
    pub scope: &'static str,
    pub path: PathBuf,
    pub status: InspectedConfigFileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InspectedConfigDocuments {
    pub paths: SettingsPaths,
    pub files: Vec<InspectedConfigFile>,
}

pub fn inspect_read_only(
    home_override: Option<&Path>,
    project_root_override: Option<&Path>,
) -> Result<InspectedConfigDocuments, String> {
    let paths = resolve_paths(home_override, project_root_override)?;
    let files = vec![
        inspect_config_file(
            InspectedConfigFileKind::ProjectSettings,
            "Project settings",
            "project",
            paths.project_settings.clone(),
        ),
        inspect_config_file(
            InspectedConfigFileKind::Settings,
            "Global settings",
            "user",
            paths.settings.clone(),
        ),
        inspect_config_file(
            InspectedConfigFileKind::LocalSettings,
            "Local settings",
            "local",
            paths.local_settings.clone(),
        ),
        inspect_config_file(
            InspectedConfigFileKind::Preferences,
            "Preferences",
            "user",
            paths.preferences.clone(),
        ),
    ];

    Ok(InspectedConfigDocuments { paths, files })
}

fn inspect_config_file(
    kind: InspectedConfigFileKind,
    label: &'static str,
    scope: &'static str,
    path: PathBuf,
) -> InspectedConfigFile {
    match std::fs::metadata(&path) {
        Ok(metadata) if !metadata.is_file() => InspectedConfigFile {
            kind,
            label,
            scope,
            path,
            status: InspectedConfigFileStatus::NotFile,
            document: None,
            error: Some("path exists but is not a file".to_owned()),
        },
        Ok(_) => inspect_config_file_contents(kind, label, scope, path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => InspectedConfigFile {
            kind,
            label,
            scope,
            path,
            status: InspectedConfigFileStatus::Missing,
            document: None,
            error: None,
        },
        Err(error) => InspectedConfigFile {
            kind,
            label,
            scope,
            path,
            status: InspectedConfigFileStatus::Unreadable,
            document: None,
            error: Some(format!("failed to inspect file metadata: {error}")),
        },
    }
}

fn inspect_config_file_contents(
    kind: InspectedConfigFileKind,
    label: &'static str,
    scope: &'static str,
    path: PathBuf,
) -> InspectedConfigFile {
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => {
            return InspectedConfigFile {
                kind,
                label,
                scope,
                path,
                status: InspectedConfigFileStatus::Unreadable,
                document: None,
                error: Some(format!("failed to read file: {error}")),
            };
        }
    };

    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(object)) => InspectedConfigFile {
            kind,
            label,
            scope,
            path,
            status: InspectedConfigFileStatus::Valid,
            document: Some(Value::Object(object)),
            error: None,
        },
        Ok(_) => InspectedConfigFile {
            kind,
            label,
            scope,
            path,
            status: InspectedConfigFileStatus::Invalid,
            document: None,
            error: Some("expected top-level JSON object".to_owned()),
        },
        Err(error) => InspectedConfigFile {
            kind,
            label,
            scope,
            path,
            status: InspectedConfigFileStatus::Invalid,
            document: None,
            error: Some(format!("failed to parse JSON: {error}")),
        },
    }
}

pub fn resolve_paths(
    home_override: Option<&Path>,
    project_root_override: Option<&Path>,
) -> Result<SettingsPaths, String> {
    let user_paths = crate::claude_paths::ClaudePaths::resolve(home_override)
        .ok_or_else(|| "Failed to resolve Claude configuration directory".to_owned())?;
    let project_root = if let Some(path) = project_root_override {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| format!("Failed to resolve current directory: {err}"))?
    };

    Ok(SettingsPaths {
        settings: user_paths.config_dir.join(SETTINGS_FILENAME),
        project_settings: project_root.join(CLAUDE_DIR).join(SETTINGS_FILENAME),
        local_settings: project_root.join(CLAUDE_DIR).join(LOCAL_SETTINGS_FILENAME),
        preferences: user_paths.preferences,
    })
}
