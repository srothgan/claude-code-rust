// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::{Value, json};

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn candidate(latest: &str, url: &str, error: Option<&str>) -> UpdatePrompt {
    UpdatePrompt {
        current_version: "1.0.0".into(),
        latest_version: latest.into(),
        release_url: url.into(),
        last_error: error.map(str::to_owned),
    }
}

#[test]
fn profile_selection_respects_override_fallback_and_unavailable_locations() {
    for (profile, system, expected) in [
        (Some("profile"), Some("system"), Some("profile/settings.json")),
        (Some("profile"), None, Some("profile/settings.json")),
        (Some(""), Some("system"), Some("system/claude-code-rust/settings.json")),
        (None, Some("system"), Some("system/claude-code-rust/settings.json")),
        (Some(""), None, None),
        (None, None, None),
    ] {
        assert_eq!(
            settings_path_from(profile.map(std::ffi::OsStr::new), system.map(Path::new)),
            expected.map(PathBuf::from)
        );
    }
}

#[test]
fn manual_candidates_require_a_newer_release_and_retain_the_actual_payload() {
    let mut settings = AppSettings::default();
    assert_eq!(update_prompt_candidate(&settings, "1.0.0", 100), None);
    for (version, eligible) in
        [("2.0.0", true), ("1.0.0", false), ("0.9.0", false), ("invalid", false)]
    {
        for url in ["https://example.invalid/custom-release", "", "   "] {
            record_update_check_result(&mut settings, "0.8.0", version, url, 50);
            settings.updates.last_install_error =
                Some("installer could not replace executable".into());
            let expected_url = if url.trim().is_empty() {
                "https://github.com/srothgan/claude-code-rust/releases/tag/v2.0.0"
            } else {
                url
            };
            assert_eq!(
                update_prompt_candidate(&settings, "1.0.0", 100),
                eligible.then(|| candidate(
                    "2.0.0",
                    expected_url,
                    Some("installer could not replace executable")
                ))
            );
        }
    }
    record_update_check_result(&mut settings, "1.0.0", "2.0.0", "https://example.invalid/new", 200);
    settings.updates.last_install_error = None;
    assert_eq!(
        update_prompt_candidate(&settings, "1.0.0", 201),
        Some(candidate("2.0.0", "https://example.invalid/new", None))
    );
    settings.updates.auto_install = true;
    assert_eq!(update_prompt_candidate(&settings, "1.0.0", 201), None);
}

#[test]
fn temporary_skip_expires_at_six_hours_and_version_skip_releases_a_later_version() {
    let mut settings = AppSettings::default();
    record_update_check_result(&mut settings, "1.0.0", "2.0.0", "release-2", 100);
    record_skip_now(&mut settings, 1_000);
    for (now, eligible) in [(22_599, false), (22_600, true), (22_601, true)] {
        assert_eq!(
            update_prompt_candidate(&settings, "1.0.0", now),
            eligible.then(|| candidate("2.0.0", "release-2", None))
        );
    }
    record_skip_now(&mut settings, 2_000);
    record_skip_version(&mut settings, "2.0.0");
    assert_eq!(update_prompt_candidate(&settings, "1.0.0", 2_001), None);
    record_update_check_result(&mut settings, "1.0.0", "3.0.0", "release-3", 2_002);
    assert_eq!(
        update_prompt_candidate(&settings, "1.0.0", 2_003),
        Some(candidate("3.0.0", "release-3", None))
    );
}

#[test]
fn locked_updater_save_changes_no_bytes_and_explicit_retry_preserves_the_owner_edit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let original=b"{\"presentation\":{\"copyFullResponse\":false},\"updates\":{\"autoInstall\":true},\"sentinel\":17}\n";
    std::fs::write(&path, original).unwrap();
    let owner = crate::json_file::lock(&path).unwrap();
    let mut settings = AppSettings::default();
    record_skip_version(&mut settings, "2.0.0");
    assert!(save_global_settings(&path, &settings).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let external = json!({"presentation":{"copyFullResponse":true,"future":"owner edit"},"updates":{"autoInstall":true},"sentinel":17});
    std::fs::write(&path, external.to_string()).unwrap();
    drop(owner);
    save_global_settings(&path, &settings).unwrap();
    let mut expected = external;
    expected["updates"]["skipped_version"] = json!("2.0.0");
    assert_eq!(read(&path), expected);
}

#[test]
fn malformed_updater_documents_are_unchanged_until_an_explicit_repaired_retry() {
    for bytes in ["{broken\n", "[]\n", r#"{"updates":false,"sentinel":17}"#] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        std::fs::write(&path, bytes).unwrap();
        let mut settings = AppSettings::default();
        record_skip_version(&mut settings, "2.0.0");
        assert!(save_global_settings(&path, &settings).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        let repaired = json!({"updates":{"autoInstall":true,"future":"keep"},"presentation":{"copyFullResponse":true},"sentinel":17});
        std::fs::write(&path, repaired.to_string()).unwrap();
        save_global_settings(&path, &settings).unwrap();
        let mut expected = repaired;
        expected["updates"]["skipped_version"] = json!("2.0.0");
        assert_eq!(read(&path), expected);
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            1,
            "failed saves release their locks and temporary files"
        );
    }
}

#[test]
fn caught_up_checks_clear_obsolete_suppression_and_failure_before_a_later_release() {
    for latest in ["1.9.0", "2.0.0", "2.1.0"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let original = json!({"updates":{"autoInstall":true,"skip_until_unix_secs":100_000,"skipped_version":"2.1.0","last_install_error":"old install failed","future":42},"presentation":{"copyFullResponse":true},"sentinel":[1,2]});
        std::fs::write(&path, original.to_string()).unwrap();
        let mut settings = load_from_path(&path).unwrap();
        record_update_check_result(&mut settings, "2.0.0", latest, "checked-release", 1_000);
        save_global_settings(&path, &settings).unwrap();
        let mut expected = original;
        expected["updates"]["last_result"] = json!({"checked_at_unix_secs":1_000,"current_version":"2.0.0","latest_version":latest,"release_url":"checked-release","source":"github_release"});
        if latest != "2.1.0" {
            for key in ["skip_until_unix_secs", "skipped_version", "last_install_error"] {
                expected["updates"].as_object_mut().unwrap().remove(key);
            }
        }
        assert_eq!(read(&path), expected);
        let reloaded = load_from_path(&path).unwrap();
        assert_eq!(
            automatic_install_failure(&reloaded, "2.0.0"),
            if latest == "2.1.0" { Some("old install failed") } else { None }
        );
        // A deliberate config-owner consent change enables the manual prompt workflow.
        expected["updates"]["autoInstall"] = json!(false);
        std::fs::write(&path, expected.to_string()).unwrap();
        let mut settings = load_from_path(&path).unwrap();
        record_update_check_result(&mut settings, "2.0.0", "3.0.0", "release-3", 1_001);
        save_global_settings(&path, &settings).unwrap();
        let reloaded = load_from_path(&path).unwrap();
        let prompt = update_prompt_candidate(&reloaded, "2.0.0", 1_002);
        assert_eq!(
            prompt,
            if latest == "2.1.0" {
                None
            } else {
                Some(UpdatePrompt {
                    current_version: "2.0.0".into(),
                    latest_version: "3.0.0".into(),
                    release_url: "release-3".into(),
                    last_error: None,
                })
            }
        );
    }
}
