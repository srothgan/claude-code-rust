// SPDX-License-Identifier: Apache-2.0

use std::process::Command;

#[test]
fn config_commands_inspect_only_the_selected_profile() {
    let temp = tempfile::tempdir().expect("tempdir");
    let profile = temp.path().join("profile with spaces");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&profile).expect("profile");
    std::fs::create_dir_all(project.join(".claude")).expect("project");
    std::fs::write(profile.join("settings.json"), r#"{"model":"isolated-model"}"#)
        .expect("settings");
    std::fs::write(profile.join(".claude.json"), r#"{"theme":"isolated-theme"}"#)
        .expect("preferences");
    std::fs::write(project.join(".claude/settings.local.json"), r#"{"fastMode":false}"#)
        .expect("local");

    let output = Command::new(env!("CARGO_BIN_EXE_claude-rs"))
        .env("CLAUDE_CONFIG_DIR", &profile)
        .args(["config", "show", "--json"])
        .current_dir(&project)
        .output()
        .expect("config show");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout).expect("utf8");
    let json: serde_json::Value = serde_json::from_str(&text).expect("json");
    let files = json["files"].as_array().expect("files");
    let settings = files.iter().find(|file| file["kind"] == "settings").expect("settings file");
    let preferences =
        files.iter().find(|file| file["kind"] == "preferences").expect("preferences file");
    let local = files.iter().find(|file| file["kind"] == "local_settings").expect("local file");
    assert_eq!(settings["path"], profile.join("settings.json").to_string_lossy().as_ref());
    assert_eq!(settings["document"]["model"], "isolated-model");
    assert_eq!(preferences["path"], profile.join(".claude.json").to_string_lossy().as_ref());
    assert_eq!(preferences["document"]["theme"], "isolated-theme");
    assert_eq!(
        local["path"],
        project.join(".claude").join("settings.local.json").to_string_lossy().as_ref()
    );
    assert_eq!(local["document"]["fastMode"], false);
    assert!(profile.join("settings.json").is_file());
    assert!(!profile.join(".claude").exists());
}

#[test]
fn a_missing_override_profile_does_not_fall_back_or_create_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("missing profile");
    let output = Command::new(env!("CARGO_BIN_EXE_claude-rs"))
        .env("CLAUDE_CONFIG_DIR", &missing)
        .args(["config", "show", "--json"])
        .current_dir(temp.path())
        .output()
        .expect("inspect");
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    for file in json["files"].as_array().expect("files") {
        assert_eq!(file["status"], "missing");
        assert!(file.get("document").is_none());
    }
    assert!(!missing.exists());
}

#[test]
fn empty_and_unset_overrides_preserve_the_default_cli_layout() {
    let temp = tempfile::tempdir().expect("tempdir");
    // Windows resolves the home through Known Folders, not USERPROFILE. This
    // command only inspects paths; it does not modify default configuration.
    let expected_home = if cfg!(windows) {
        dirs::home_dir().expect("Windows home")
    } else {
        temp.path().to_path_buf()
    };
    for empty in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_claude-rs"));
        command.env("HOME", temp.path()).env("USERPROFILE", temp.path());
        if empty {
            command.env("CLAUDE_CONFIG_DIR", "");
        } else {
            command.env_remove("CLAUDE_CONFIG_DIR");
        }
        let output = command
            .args(["config", "path", "--json"])
            .current_dir(temp.path())
            .output()
            .expect("config path");
        assert!(output.status.success());
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
        let files = json["files"].as_array().expect("files");
        let settings = files.iter().find(|file| file["kind"] == "settings").expect("settings");
        let preferences =
            files.iter().find(|file| file["kind"] == "preferences").expect("preferences");
        assert_eq!(
            settings["path"],
            expected_home.join(".claude").join("settings.json").to_string_lossy().as_ref()
        );
        assert_eq!(
            preferences["path"],
            expected_home.join(".claude.json").to_string_lossy().as_ref()
        );
    }
}
