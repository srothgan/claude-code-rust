// SPDX-License-Identifier: Apache-2.0

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

struct DoctorFixture {
    root: tempfile::TempDir,
    project: PathBuf,
    launch: PathBuf,
    profile: PathBuf,
    app_profile: PathBuf,
    runtime: PathBuf,
    script: PathBuf,
    settings: Vec<(PathBuf, Vec<u8>)>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl DoctorFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("selected-project");
        let launch = root.path().join("launch-directory");
        let profile = root.path().join("selected-profile");
        let app_profile = root.path().join("app-profile");
        for path in
            [&project.join(".claude"), &project.join("sub"), &launch, &profile, &app_profile]
        {
            std::fs::create_dir_all(path).unwrap();
        }
        let settings = [
            (profile.join("settings.json"), br#"{"env":{"PROFILE_SENTINEL":"selected"}}"#.to_vec()),
            (profile.join(".claude.json"), br#"{"theme":"dark"}"#.to_vec()),
            (
                project.join(".claude/settings.json"),
                br#"{"env":{"PROJECT_SENTINEL":"selected"}}"#.to_vec(),
            ),
            (
                project.join(".claude/settings.local.json"),
                br#"{"env":{"LOCAL_SENTINEL":"selected"}}"#.to_vec(),
            ),
        ]
        .into_iter()
        .collect::<Vec<_>>();
        for (path, bytes) in &settings {
            std::fs::write(path, bytes).unwrap();
        }
        let runtime = root.path().join(if cfg!(windows) {
            "runtime fixture.cmd"
        } else {
            "runtime fixture.sh"
        });
        #[cfg(windows)]
        std::fs::write(&runtime,b"@echo off\r\nif not \"%~1\"==\"--version\" exit /b 9\r\nif not \"%~2\"==\"\" exit /b 10\r\necho 1.2.3\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(&runtime,b"#!/bin/sh\n[ \"$#\" = 1 ] && [ \"$1\" = --version ] || exit 9\nprintf '1.2.3\\n'\n").unwrap();
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = root.path().join("healthy bridge.js");
        std::fs::write(&script, b"throw new Error('doctor must never start the bridge');").unwrap();
        Self { root, project, launch, profile, app_profile, runtime, script, settings }
    }

    async fn run(
        &self,
        cwd: &Path,
        dir: &str,
        script: &Path,
        args: &[&str],
    ) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_claude-rs"));
        command
            .current_dir(cwd)
            .arg("--dir")
            .arg(dir)
            .arg("--bridge-script")
            .arg(script)
            .args(args)
            .env("CLAUDE_CONFIG_DIR", &self.profile)
            .env("CLAUDE_RS_CONFIG_DIR", &self.app_profile)
            .env("CLAUDE_RS_AGENT_BRIDGE_RUNTIME", &self.runtime)
            .env_remove("CLAUDE_RS_AGENT_BRIDGE")
            .env_remove("CLAUDE_CODE_PROJECT_DIR_NAME")
            .env("NO_COLOR", "1")
            .env_remove("FORCE_COLOR")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        for (key, directory) in [
            ("HOME", "home"),
            ("USERPROFILE", "home"),
            ("LOCALAPPDATA", "local-data"),
            ("APPDATA", "roaming-data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_DATA_HOME", "local-data"),
            ("XDG_CONFIG_HOME", "config"),
        ] {
            command.env(key, self.root.path().join(directory));
        }
        let output = tokio::time::timeout(Duration::from_secs(20), command.output())
            .await
            .expect("bounded support command")
            .unwrap();
        assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
        output
    }

    fn assert_sources_unchanged(&self) {
        for (path, bytes) in &self.settings {
            assert_eq!(&std::fs::read(path).unwrap(), bytes, "{}", path.display());
        }
        assert!(
            std::fs::read_dir(&self.app_profile).unwrap().next().is_none(),
            "support dispatch must not start the app"
        );
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn check<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).expect("named diagnostic")
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn assert_path(value: &Value, expected: &Path) {
    assert_eq!(
        Path::new(value.as_str().expect("reported path")).canonicalize().unwrap(),
        expected.canonicalize().unwrap()
    );
}

#[tokio::test]
async fn doctor_reports_the_selected_failure_in_each_format_and_strict_changes_only_exit_policy() {
    let fixture = DoctorFixture::new();
    let missing = fixture.root.path().join("missing selected bridge.js");
    for json in [false, true] {
        for strict in [false, true] {
            let mut args = vec!["doctor"];
            if json {
                args.push("--json");
            }
            if strict {
                args.push("--strict");
            }
            let output = fixture.run(&fixture.launch, "../selected-project", &missing, &args).await;
            assert_eq!(
                output.status.code(),
                Some(i32::from(strict)),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            if json {
                let report: Value = serde_json::from_slice(&output.stdout).unwrap();
                let failed = check(&report, "bridge_script");
                assert_eq!(failed["status"], "fail");
                assert_eq!(failed["hard_failure"], true);
                assert_eq!(failed["details"]["explicit_bridge_script"], missing.to_str().unwrap());
                let cause = failed["message"].as_str().unwrap();
                assert!(
                    cause.contains("does not exist") && cause.contains(missing.to_str().unwrap()),
                    "{cause}"
                );
                let runtime = check(&report, "bridge_runtime_version");
                assert_eq!(runtime["status"], "pass");
                assert_eq!(runtime["details"]["version"], "1.2.3");
                assert_path(&runtime["details"]["runtime"], &fixture.runtime);
            } else {
                let text = String::from_utf8(output.stdout).unwrap();
                let row = text
                    .lines()
                    .find(|line| line.contains("Bridge script"))
                    .expect("bridge script diagnostic row");
                assert!(
                    row.contains("FAIL")
                        && row.contains("does not exist")
                        && row.contains(missing.to_str().unwrap()),
                    "{row}"
                );
                let version = text
                    .lines()
                    .find(|line| line.contains("Bridge runtime version"))
                    .expect("runtime version row");
                assert!(version.contains("PASS") && version.contains("1.2.3"), "{version}");
            }
            fixture.assert_sources_unchanged();
        }
    }
    // A warning is diagnostic information, not a hard prerequisite failure.
    let healthy = fixture
        .run(
            &fixture.launch,
            "../selected-project",
            &fixture.script,
            &["doctor", "--json", "--strict"],
        )
        .await;
    assert_eq!(healthy.status.code(), Some(0), "{}", String::from_utf8_lossy(&healthy.stdout));
    let report: Value = serde_json::from_slice(&healthy.stdout).unwrap();
    assert_eq!(check(&report, "bridge_script")["status"], "pass");
    assert_eq!(check(&report, "bridge_runtime_version")["details"]["version"], "1.2.3");
    for (id, suffix) in
        [("runtime_log_dir", "logs/runtime"), ("legacy_log_path", "logs/claude-rs.log")]
    {
        let diagnostic = check(&report, id);
        assert_eq!(diagnostic["status"], "pass");
        let path = Path::new(diagnostic["details"]["path"].as_str().unwrap());
        assert!(path.is_absolute() && path.ends_with(suffix), "{diagnostic}");
    }
    let credentials = check(&report, "claude_credentials");
    assert_eq!(credentials["status"], "warn");
    assert_eq!(credentials["hard_failure"], false);
    assert_eq!(
        credentials["details"]["path"],
        fixture.profile.join(".credentials.json").to_str().unwrap()
    );
    fixture.assert_sources_unchanged();
}

#[tokio::test]
async fn config_and_doctor_associate_paths_with_the_selected_project_and_profile() {
    let fixture = DoctorFixture::new();
    for (cwd, dir) in [
        (&fixture.project, "."),
        (&fixture.launch, "../selected-project"),
        (&fixture.launch, "../selected-project/sub/.."),
    ] {
        let config = fixture.run(cwd, dir, &fixture.script, &["config", "path", "--json"]).await;
        assert!(config.status.success());
        let config: Value = serde_json::from_slice(&config.stdout).unwrap();
        assert_path(&config["project_root"], &fixture.project);
        let doctor = fixture.run(cwd, dir, &fixture.script, &["doctor", "--json"]).await;
        assert!(doctor.status.success());
        let doctor: Value = serde_json::from_slice(&doctor.stdout).unwrap();
        for (kind, diagnostic, expected) in [
            ("settings", "config_settings", fixture.profile.join("settings.json")),
            ("preferences", "config_preferences", fixture.profile.join(".claude.json")),
            (
                "project_settings",
                "config_project_settings",
                fixture.project.join(".claude/settings.json"),
            ),
            (
                "local_settings",
                "config_local_settings",
                fixture.project.join(".claude/settings.local.json"),
            ),
        ] {
            let file = config["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|file| file["kind"] == kind)
                .expect("selected scoped file");
            assert_path(&file["path"], &expected);
            let result = check(&doctor, diagnostic);
            assert_path(&result["details"]["path"], &expected);
            assert_eq!(result["status"], "pass");
            assert_eq!(result["details"]["state"], "file");
        }
        fixture.assert_sources_unchanged();
    }
}
