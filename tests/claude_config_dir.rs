// SPDX-License-Identifier: Apache-2.0

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;

struct ProfileFixture {
    root: tempfile::TempDir,
    project: PathBuf,
    profile: PathBuf,
    home: PathBuf,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl ProfileFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project with spaces");
        let profile = root.path().join("selected profile");
        let home = root.path().join("default home");
        for path in [&project.join(".claude"), &profile, &home.join(".claude")] {
            std::fs::create_dir_all(path).unwrap();
        }
        Self { root, project, profile, home }
    }

    fn files(&self) -> [(&'static str, &'static str, &'static str, PathBuf); 4] {
        [
            ("settings", "settings", "user", self.profile.join("settings.json")),
            ("preferences", "preferences", "user", self.profile.join(".claude.json")),
            (
                "project_settings",
                "project-settings",
                "project",
                self.project.join(".claude").join("settings.json"),
            ),
            (
                "local_settings",
                "local-settings",
                "local",
                self.project.join(".claude").join("settings.local.json"),
            ),
        ]
    }

    fn seed(&self) {
        for (kind, _, _, path) in self.files() {
            std::fs::write(path, serde_json::to_vec(&json!({"model":kind,"enabled":false,"env":{"apiKey":"selected-profile-secret","SAFE":"visible"}})).unwrap()).unwrap();
        }
        std::fs::write(self.profile.join(".credentials.json"), b"private credential file").unwrap();
        std::fs::write(self.project.join("unrelated.txt"), b"private project document").unwrap();
        self.seed_default();
    }

    fn seed_default(&self) {
        std::fs::write(
            self.home.join(".claude").join("settings.json"),
            br#"{"model":"wrong-default-model"}"#,
        )
        .unwrap();
        std::fs::write(self.home.join(".claude.json"), br#"{"theme":"wrong-default-theme"}"#)
            .unwrap();
    }

    async fn run(&self, args: &[&str], profile: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_claude-rs"));
        command
            .current_dir(&self.project)
            .arg("--bridge-script")
            .arg(self.root.path().join("absent bridge.js"))
            .arg("--log-file")
            .arg(self.root.path().join("unrequested.log"))
            .args(args)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CLAUDE_RS_CONFIG_DIR", self.root.path().join("app-profile"))
            .env_remove("CLAUDE_CODE_PROJECT_DIR_NAME")
            .env("NO_COLOR", "1")
            .env_remove("FORCE_COLOR")
            .env("PRIVATE_CONFIG_TEST_SENTINEL", "private environment value")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(profile) = profile {
            command.env("CLAUDE_CONFIG_DIR", profile);
        } else {
            command.env_remove("CLAUDE_CONFIG_DIR");
        }
        tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .expect("bounded config command")
            .unwrap()
    }

    async fn selected(&self, args: &[&str]) -> Output {
        self.run(args, Some(&self.profile)).await
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        snapshot(self.root.path())
    }

    fn assert_report(&self, report: &Value, documents: bool) {
        for (kind, _, scope, path) in self.files() {
            let file = record(report, kind);
            assert_path(&file["path"], &path);
            assert_eq!(file["scope"], scope);
            assert_eq!(file["status"], "valid");
            if documents {
                assert_eq!(
                    file["document"],
                    json!({"model":kind,"enabled":false,"env":{"apiKey":"[redacted]","SAFE":"visible"}})
                );
            } else {
                assert!(file.get("document").is_none());
            }
        }
        let text = report.to_string();
        for secret in [
            "selected-profile-secret",
            "private credential file",
            "private project document",
            "private environment value",
            "wrong-default-model",
            "wrong-default-theme",
        ] {
            assert!(!text.contains(secret), "leaked {secret}");
        }
    }
}

#[allow(clippy::unwrap_used)]
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut result = BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.insert(path.clone(), None);
            result.extend(snapshot(&path));
        } else {
            result.insert(path.clone(), Some(std::fs::read(path).unwrap()));
        }
    }
    result
}

#[allow(clippy::unwrap_used)]
fn record<'a>(report: &'a Value, kind: &str) -> &'a Value {
    report["files"].as_array().unwrap().iter().find(|file| file["kind"] == kind).unwrap()
}

#[allow(clippy::unwrap_used)]
fn assert_path(actual: &Value, expected: &Path) {
    // Process cwd is physical on macOS; existing fixture paths can be compared canonically.
    assert_eq!(
        std::fs::canonicalize(actual.as_str().unwrap()).unwrap(),
        expected.canonicalize().unwrap()
    );
}

#[allow(clippy::unwrap_used)]
fn success(output: &Output) -> &str {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    std::str::from_utf8(&output.stdout).unwrap()
}

#[tokio::test]
async fn config_commands_inspect_only_the_selected_profile() {
    let f = ProfileFixture::new();
    f.seed();
    let before = f.snapshot();
    for args in [&["config"][..], &["config", "path"][..]] {
        let output = f.selected(args).await;
        let human = success(&output);
        for (label, kind) in [
            ("Global settings", "settings"),
            ("Preferences", "preferences"),
            ("Project settings", "project_settings"),
            ("Local settings", "local_settings"),
        ] {
            let line = human
                .lines()
                .find(|line| line.ends_with(label) && line.starts_with("  [FILE]"))
                .unwrap();
            let region = human.split_once(line).unwrap().1.split("\n  [").next().unwrap();
            let (_, _, scope, path) =
                f.files().into_iter().find(|(k, _, _, _)| *k == kind).unwrap();
            let shown =
                region.lines().find_map(|line| line.trim().strip_prefix("- Path: ")).unwrap();
            assert_path(&Value::String(shown.into()), &path);
            assert!(region.contains(&format!("Scope: {scope}")));
        }
    }
    for (_, selector, _, path) in f.files() {
        let output = f.selected(&["config", "path", "--which", selector]).await;
        let expected = if selector == "project-settings" || selector == "local-settings" {
            {
                let root = if cfg!(windows) {
                    f.project.clone()
                } else {
                    f.project.canonicalize().unwrap()
                };
                root.join(path.strip_prefix(&f.project).unwrap())
            }
        } else {
            path
        };
        assert_eq!(success(&output), format!("{}\n", expected.display()));
    }
    for (args, documents) in [
        (&["config", "path", "--json"][..], false),
        (&["config", "show", "--json"][..], true),
        (&["config", "export"][..], true),
    ] {
        let output = f.selected(args).await;
        let report: Value = serde_json::from_str(success(&output)).unwrap();
        f.assert_report(&report, documents);
        if args[1] == "export" {
            assert_eq!(report["schema"], "claude-rs-config-export/v1");
        }
    }
    assert_eq!(f.snapshot(), before);
}

#[tokio::test]
async fn selected_profile_export_writes_requested_artifact_and_refuses_overwrite() {
    let f = ProfileFixture::new();
    f.seed();
    let before = f.snapshot();
    let target = f.root.path().join("selected export.json");
    let args = ["config", "export", "--output", target.to_str().unwrap()];
    let output = f.selected(&args).await;
    success(&output);
    let bytes = std::fs::read(&target).unwrap();
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(report["schema"], "claude-rs-config-export/v1");
    f.assert_report(&report, true);
    let output = f.selected(&args).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(target.to_str().unwrap()));
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    let mut after = f.snapshot();
    after.remove(&target);
    assert_eq!(after, before);
}

#[tokio::test]
async fn a_missing_override_profile_does_not_fall_back_or_create_files() {
    let mut f = ProfileFixture::new();
    f.profile = f.root.path().join("missing profile");
    f.seed_default();
    std::fs::write(
        f.project.join(".claude").join("settings.local.json"),
        br#"{"model":"usable-local"}"#,
    )
    .unwrap();
    let before = f.snapshot();
    let output = f.selected(&["config", "show", "--json"]).await;
    let report: Value = serde_json::from_str(success(&output)).unwrap();
    for (kind, name) in [("settings", "settings.json"), ("preferences", ".claude.json")] {
        let file = record(&report, kind);
        assert_eq!(file["path"], f.profile.join(name).to_str().unwrap());
        assert_eq!(file["status"], "missing");
        assert!(file.get("document").is_none());
    }
    let local = record(&report, "local_settings");
    assert_path(&local["path"], &f.project.join(".claude").join("settings.local.json"));
    assert_eq!(local["document"], json!({"model":"usable-local"}));
    assert_eq!(record(&report, "project_settings")["status"], "missing");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("wrong-default"));
    assert!(!f.profile.exists());
    assert_eq!(f.snapshot(), before);
}

// HOME redirects the native home lookup on Unix. USERPROFILE does not redirect
// Windows Known Folders: portable default-layout policy stays in ClaudePaths tests.
#[cfg(unix)]
#[tokio::test]
async fn empty_and_unset_overrides_preserve_the_default_cli_layout() {
    let f = ProfileFixture::new();
    f.seed_default();
    let before = f.snapshot();
    for profile in [None, Some(Path::new(""))] {
        let output = f.run(&["config", "show", "--json"], profile).await;
        let report: Value = serde_json::from_str(success(&output)).unwrap();
        let settings = record(&report, "settings");
        assert_eq!(
            settings["path"],
            f.home.join(".claude").join("settings.json").to_str().unwrap()
        );
        assert_eq!(settings["document"], json!({"model":"wrong-default-model"}));
        let preferences = record(&report, "preferences");
        assert_eq!(preferences["path"], f.home.join(".claude.json").to_str().unwrap());
        assert_eq!(preferences["document"], json!({"theme":"wrong-default-theme"}));
    }
    assert_eq!(f.snapshot(), before);
}
