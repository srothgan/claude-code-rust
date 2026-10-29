// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::{
    footer::{FooterFieldId, FooterSide},
    settings,
};

fn requested_fields() -> Value {
    // A complete custom layout is an independent persisted-document oracle.
    json!([
        {"id":"model","visible":false,"side":"left","color":"cyan"},
        {"id":"permissionMode","visible":false,"side":"left"},
        {"id":"effort","visible":false,"side":"left"},
        {"id":"ultracode","visible":false,"side":"left"},
        {"id":"fastMode","visible":false,"side":"left"},
        {"id":"runningSubagents","visible":false,"side":"left"},
        {"id":"directory","visible":true,"side":"right","color":"orange"},
        {"id":"gitBranch","visible":false,"side":"right"},
        {"id":"contextRemaining","visible":false,"side":"right"}
    ])
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn assert_no_operation_files(directory: &Path) {
    let names: Vec<_> =
        std::fs::read_dir(directory).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, vec![std::ffi::OsString::from("settings.json")]);
}

#[test]
fn footer_and_updater_writes_preserve_other_owners_and_remove_only_obsolete_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let initial = json!({"presentation":{"future":"initial"},"updates":{"autoInstall":true,"future":42,"skipped_version":"1.0.0"},"unknown":[1,2]});
    std::fs::write(&path, initial.to_string()).unwrap();
    let state = FooterPreferences::load(Some(&path));
    let mut external = initial;
    external["presentation"] =
        json!({"future":"external","copyFullResponse":true,"showStatusInTerminalTab":false});
    std::fs::write(&path, external.to_string()).unwrap();
    let requested = requested_fields();
    let layout = FooterLayout::from_saved(Some(&requested)).unwrap();
    assert!(save(&path, state.revision(), Some(&layout)).1.is_none());
    let mut expected = external;
    expected["presentation"]["footer"] = requested;
    assert_eq!(read(&path), expected);
    let mut updates = settings::AppSettings::default();
    assert!(!updates.updates.auto_install, "stale in-memory consent differs from the document");
    settings::record_update_check_result(
        &mut updates,
        "1.0.0",
        "2.0.0",
        "https://example.invalid/release-2",
        500,
    );
    settings::save_global_settings(&path, &updates).unwrap();
    expected["updates"].as_object_mut().unwrap().remove("skipped_version");
    expected["updates"]["last_result"] = json!({"checked_at_unix_secs":500,"current_version":"1.0.0","latest_version":"2.0.0","release_url":"https://example.invalid/release-2","source":"github_release"});
    assert_eq!(read(&path), expected);
    assert_no_operation_files(directory.path());
}

#[test]
fn resetting_an_invalid_leaf_preserves_siblings_and_reloads_visible_defaults() {
    for siblings in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut initial = json!({"presentation":{"footer":false},"retain":17});
        if siblings {
            initial["presentation"]["future"] = json!("keep");
        }
        std::fs::write(&path, initial.to_string()).unwrap();
        let before = FooterPreferences::load(Some(&path));
        assert!(before.error.is_some());
        assert!(save(&path, before.revision(), None).1.is_none());
        let saved = read(&path);
        assert!(saved.pointer("/presentation/footer").is_none());
        assert_eq!(saved["retain"], 17);
        if siblings {
            assert_eq!(saved["presentation"]["future"], "keep");
        }
        let reloaded = FooterPreferences::load(Some(&path));
        assert!(reloaded.error.is_none());
        assert!(reloaded.saved().is_none());
        let layout = reloaded.layout();
        for (id, side) in [
            (FooterFieldId::Model, FooterSide::Left),
            (FooterFieldId::Directory, FooterSide::Right),
        ] {
            let field = layout.fields().iter().find(|f| f.id == id).unwrap();
            assert!(field.visible);
            assert_eq!(field.side, side);
            assert_eq!(field.color, None);
        }
        assert_no_operation_files(directory.path());
    }
}

#[test]
fn detected_intervening_writes_preserve_siblings_or_require_an_explicit_footer_retry() {
    for conflict in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let original = json!({"presentation":{"footer":[],"future":"before"},"root":[1,2]});
        std::fs::write(&path, original.to_string()).unwrap();
        let state = FooterPreferences::load(Some(&path));
        let mut external = original;
        external["presentation"]["future"] = json!("external editor");
        external["unknown"] = json!({"keep":true});
        if conflict {
            external["presentation"]["footer"] =
                json!([{"id":"model","visible":true,"side":"right","color":"pink"}]);
        }
        let external_bytes = serde_json::to_vec(&external).unwrap();
        let checkpoint_path = path.clone();
        let checkpoint_bytes = external_bytes.clone();
        let observed = std::rc::Rc::new(std::cell::Cell::new(false));
        let signal = observed.clone();
        let requested = requested_fields();
        let layout = FooterLayout::from_saved(Some(&requested)).unwrap();
        let (revision, error) = crate::json_file::with_before_replacement(
            &path,
            move || {
                std::fs::write(&checkpoint_path, checkpoint_bytes).unwrap();
                signal.set(true);
            },
            || save(&path, state.revision(), Some(&layout)),
        );
        assert!(observed.get(), "the edit occurs at the real replacement boundary");
        assert_no_operation_files(directory.path());
        if conflict {
            assert!(error.is_some());
            assert_eq!(std::fs::read(&path).unwrap(), external_bytes);
            let retry = save(&path, revision.as_deref().unwrap(), Some(&layout));
            assert!(retry.1.is_none(), "returned conflict revision authorizes deliberate retry");
        } else {
            assert!(error.is_none());
        }
        let mut expected = external;
        expected["presentation"]["footer"] = requested;
        assert_eq!(read(&path), expected);
        assert!(FooterPreferences::load(Some(&path)).error.is_none());
        assert_no_operation_files(directory.path());
    }
}
