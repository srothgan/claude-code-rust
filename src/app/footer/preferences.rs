// SPDX-License-Identifier: Apache-2.0

use super::{FooterLayout, SETTING_ID};
use crate::agent::settings::{
    SavedSetting, ScopedSetting, SettingDescriptor, SettingKind, SettingProvenance,
    SettingsApplication, SettingsCategory, SettingsScope, SettingsSnapshot, SettingsSource,
};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingSave {
    pub(crate) request_id: String,
    pub(crate) layout: Option<FooterLayout>,
}

/// Session-independent authority for the saved footer. Snapshots are derived views.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FooterPreferences {
    saved: Option<FooterLayout>,
    revision: String,
    pub(crate) error: Option<String>,
    pub(crate) pending: Option<PendingSave>,
}

impl FooterPreferences {
    pub(crate) fn load(path: Option<&Path>) -> Self {
        let loaded = path.map_or_else(|| Ok((json!({}), None)), read_document);
        match loaded {
            Ok((document, _)) => {
                let value = document.pointer("/presentation/footer");
                let parsed = FooterLayout::from_saved(value);
                Self {
                    revision: revision(value),
                    error: parsed.as_ref().err().map(|error| format!("Invalid footer: {error}")),
                    saved: value.and_then(|_| parsed.ok()),
                    pending: None,
                }
            }
            Err(error) => Self { error: Some(error), ..Self::default() },
        }
    }

    pub(crate) fn layout(&self) -> FooterLayout {
        self.saved.clone().unwrap_or_default()
    }

    pub(crate) fn saved(&self) -> Option<&FooterLayout> {
        self.saved.as_ref()
    }

    pub(crate) fn summary(&self) -> &'static str {
        if self.error.is_some() {
            "Invalid layout"
        } else if self.saved.is_some() {
            "Custom layout"
        } else {
            "Default layout"
        }
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn complete(
        &mut self,
        request_id: &str,
        revision: Option<String>,
        saved: bool,
    ) -> bool {
        if self.pending.as_ref().is_none_or(|pending| pending.request_id != request_id) {
            return false;
        }
        let Some(pending) = self.pending.take() else {
            return false;
        };
        if let Some(revision) = revision {
            self.revision = revision;
        }
        if saved {
            self.saved.clone_from(&pending.layout);
            self.error = None;
        }
        true
    }

    /// Add only a derived UI entry; SDK snapshots never supply footer semantics.
    pub(crate) fn project(&self, snapshot: &mut SettingsSnapshot, path: Option<&Path>) {
        if !snapshot.categories.iter().any(|category| category.id == "general") {
            snapshot.categories.insert(
                0,
                SettingsCategory {
                    id: "general".into(),
                    label: "General".into(),
                    short_label: "General".into(),
                },
            );
        }
        snapshot.catalog.retain(|setting| setting.id != SETTING_ID);
        snapshot.catalog.push(SettingDescriptor {
            id: SETTING_ID.into(), label: "Footer".into(),
            description: "Choose visible footer fields, their alignment, order, and colors. Permission and authentication notices remain at the far right. Applies immediately.".into(),
            category: "general".into(), editor: None,
            key_path: vec!["presentation".into(), "footer".into()], kind: SettingKind::Json,
            options: vec![], writable_scopes: if path.is_some() { vec![SettingsScope::User] } else { vec![] },
            allows_custom: true, reset: "Reset restores the default footer.".into(),
            application: SettingsApplication::Host, unavailable: None,
        });
        snapshot.catalog.sort_by_key(|setting| setting.label.to_lowercase());
        let value = self.saved.as_ref().and_then(|layout| serde_json::to_value(layout).ok());
        snapshot.values.retain(|setting| setting.id != SETTING_ID);
        snapshot.values.push(SavedSetting {
            id: SETTING_ID.into(),
            value: value.clone(),
            contributors: if value.is_some() { vec!["user".into()] } else { vec![] },
            policy_restricted: false,
        });
        for scope in [SettingsScope::User, SettingsScope::Project, SettingsScope::Local] {
            if !snapshot.sources.iter().any(|source| source.scope == scope) {
                snapshot.sources.push(SettingsSource {
                    scope,
                    path: path.map_or_else(String::new, |path| path.display().to_string()),
                    status: "valid".into(),
                    error: None,
                    values: vec![],
                });
            }
            if let Some(source) = snapshot.sources.iter_mut().find(|source| source.scope == scope) {
                source.values.retain(|setting| setting.id != SETTING_ID);
                source.values.push(ScopedSetting {
                    id: SETTING_ID.into(),
                    revision: self.revision.clone(),
                    value: (scope == SettingsScope::User).then(|| value.clone()).flatten(),
                });
            }
        }
        snapshot.provenance.insert(
            SETTING_ID.into(),
            SettingProvenance {
                source: "user".into(),
                path: path.map(|path| path.display().to_string()),
                policy_origin: None,
            },
        );
    }
}

fn revision(value: Option<&Value>) -> String {
    value.map_or_else(|| "unset".into(), Value::to_string)
}

fn read_document(path: &Path) -> Result<(Value, Option<Vec<u8>>), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("App settings path must be a regular file.".into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((json!({}), None)),
        Err(error) => return Err(format!("Cannot read app settings: {error}")),
        _ => {}
    }
    let raw = std::fs::read(path).map_err(|error| format!("Cannot read app settings: {error}"))?;
    let document: Value =
        serde_json::from_slice(&raw).map_err(|error| format!("Invalid app settings: {error}"))?;
    if !document.is_object() {
        return Err("App settings must be a JSON object.".into());
    }
    Ok((document, Some(raw)))
}

/// Lock and reread the document; compare only the edited leaf and preserve all siblings.
pub(crate) fn save(
    path: &Path,
    expected_revision: &str,
    layout: Option<&FooterLayout>,
) -> (Option<String>, Option<String>) {
    let result = (|| {
        let _lock = crate::json_file::lock(path)
            .map_err(|error| format!("Cannot lock app settings: {error}"))?;
        for _ in 0..3 {
            let (mut document, raw) = read_document(path)?;
            let current = revision(document.pointer("/presentation/footer"));
            if current != expected_revision {
                return Ok((
                    Some(current),
                    Some("The saved footer changed outside this app.".into()),
                ));
            }
            let next =
                layout.map(serde_json::to_value).transpose().map_err(|error| error.to_string())?;
            if revision(next.as_ref()) == current {
                return Ok((Some(current), None));
            }
            let root = document.as_object_mut().ok_or("App settings must be an object.")?;
            if let Some(next) = &next {
                let presentation = root.entry("presentation").or_insert_with(|| json!({}));
                let presentation = presentation
                    .as_object_mut()
                    .ok_or("App presentation settings must be an object.")?;
                presentation.insert("footer".into(), next.clone());
            } else if let Some(presentation) = root.get_mut("presentation") {
                let presentation = presentation
                    .as_object_mut()
                    .ok_or("App presentation settings must be an object.")?;
                presentation.remove("footer");
                if presentation.is_empty() {
                    root.remove("presentation");
                }
            }
            if crate::json_file::replace_if_unchanged(path, &document, raw.as_deref())
                .map_err(|error| format!("Cannot save footer preferences: {error}"))?
            {
                return Ok((Some(revision(next.as_ref())), None));
            }
        }
        Err::<_, String>("App settings changed during saving.".into())
    })();
    result.unwrap_or_else(|error| (None, Some(error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_preserves_external_siblings_and_updater_saves_preserve_the_footer() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"presentation":{"future":"initial"}}"#).expect("fixture");
        let initial = FooterPreferences::load(Some(&path));
        std::fs::write(&path, r#"{"presentation":{"future":"external"},"updates":{"autoInstall":true},"unknown":[1,2]}"#).expect("external sibling edit");
        let layout = FooterLayout::from_saved(Some(&json!([
            {"id":"directory","visible":true,"side":"right","color":"orange"}
        ])))
        .expect("layout");
        let (_, error) = save(&path, initial.revision(), Some(&layout));
        assert!(error.is_none(), "unrelated edits are not footer conflicts");
        let mut updates = crate::app::AppSettings::default();
        crate::app::settings::record_skip_version(&mut updates, "0.99.0");
        crate::app::settings::save_global_settings(&path, &updates).expect("updater save");
        let saved: Value =
            serde_json::from_slice(&std::fs::read(path).expect("read")).expect("JSON");
        assert_eq!(saved["presentation"]["future"], "external");
        let directory = saved["presentation"]["footer"]
            .as_array()
            .expect("footer")
            .iter()
            .find(|field| field["id"] == "directory")
            .expect("directory field");
        assert_eq!(
            *directory,
            json!({"id":"directory","visible":true,"side":"right","color":"orange"})
        );
        assert_eq!(saved["updates"]["autoInstall"], true);
        assert_eq!(saved["updates"]["skipped_version"], "0.99.0");
        assert_eq!(saved["unknown"], json!([1, 2]));
    }

    #[test]
    fn invalid_or_nonregular_app_documents_cannot_be_overwritten() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        for content in ["{broken", "[]", r#"{"presentation":false}"#] {
            std::fs::write(&path, content).expect("fixture");
            let state = FooterPreferences::load(Some(&path));
            let (_, error) = save(&path, state.revision(), Some(&FooterLayout::default()));
            assert!(error.is_some());
            assert_eq!(std::fs::read_to_string(&path).expect("read"), content);
        }
        let (_, error) = save(directory.path(), "unset", Some(&FooterLayout::default()));
        assert!(error.is_some());
        assert!(directory.path().is_dir());
    }

    #[test]
    fn reset_repairs_an_invalid_footer_and_removes_an_empty_presentation_container() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"presentation":{"footer":false},"retain":17}"#).expect("fixture");
        let state = FooterPreferences::load(Some(&path));
        assert!(state.error.is_some());
        let (_, error) = save(&path, state.revision(), None);
        assert!(error.is_none());
        let saved: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("JSON");
        assert_eq!(saved, json!({"retain":17}));
        assert!(FooterPreferences::load(Some(&path)).saved().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_settings_target_is_rejected_without_touching_its_destination() {
        let directory = tempfile::tempdir().expect("directory");
        let target = directory.path().join("target.json");
        let link = directory.path().join("settings.json");
        std::fs::write(&target, r#"{"retain":"secret"}"#).expect("fixture");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        assert!(FooterPreferences::load(Some(&link)).error.is_some());
        assert!(save(&link, "unset", Some(&FooterLayout::default())).1.is_some());
        assert_eq!(std::fs::read_to_string(&target).expect("target"), r#"{"retain":"secret"}"#);
        assert!(std::fs::symlink_metadata(&link).expect("link").file_type().is_symlink());
    }
}
