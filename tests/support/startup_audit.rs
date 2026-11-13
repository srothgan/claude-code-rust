// SPDX-License-Identifier: Apache-2.0

use clap::{Parser, error::ErrorKind};
use claude_code_rust::Cli;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Fixture {
    root: tempfile::TempDir,
    profile: PathBuf,
    app_profile: PathBuf,
    script: PathBuf,
    log: PathBuf,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        let app_profile = root.path().join("app-profile");
        for dir in [&profile, &app_profile] {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(dir.join("settings.json"), b"{\"env\":{\"PRESERVE\":\"sentinel\"}}")
                .unwrap();
        }
        let script = root.path().join("inert.js");
        std::fs::write(&script, b"throw new Error('must not start');").unwrap();
        let log = root.path().join("runtime.log");
        Self { root, profile, app_profile, script, log }
    }

    fn command(&self, script: &Path) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_claude-rs"));
        cmd.current_dir(self.root.path())
            .arg("--bridge-script")
            .arg(script)
            .arg("--log-file")
            .arg(&self.log)
            .arg("--no-update-check")
            .env("CLAUDE_CONFIG_DIR", &self.profile)
            .env("CLAUDE_RS_CONFIG_DIR", &self.app_profile)
            .env_remove("RUST_LOG")
            .env_remove("CLAUDE_RS_AGENT_BRIDGE")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        for (key, child) in [
            ("HOME", "home"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
        ] {
            cmd.env(key, self.root.path().join(child));
        }
        cmd
    }

    async fn run(&self, script: &Path, args: &[&OsStr]) -> std::process::Output {
        let mut cmd = self.command(script);
        cmd.args(args);
        tokio::time::timeout(Duration::from_secs(20), cmd.output())
            .await
            .expect("bounded CLI fixture")
            .unwrap()
    }

    fn assert_preserved(&self, occupied_log: bool) {
        for dir in [&self.profile, &self.app_profile] {
            assert_eq!(
                std::fs::read(dir.join("settings.json")).unwrap(),
                b"{\"env\":{\"PRESERVE\":\"sentinel\"}}"
            );
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1, "no startup writes");
        }
        if occupied_log {
            assert_eq!(std::fs::read(&self.log).unwrap(), b"prior log bytes");
        } else {
            assert!(!self.log.exists());
        }
        assert_eq!(std::fs::read(&self.script).unwrap(), b"throw new Error('must not start');");
    }
}

#[test]
fn invalid_launch_values_identify_the_option_and_accept_corrected_inputs() {
    for (flag, bad, good, kind) in [
        ("--effort", "invalid", "high", ErrorKind::InvalidValue),
        ("--permission-mode", "invalid", "plan", ErrorKind::InvalidValue),
        ("--model", "", "opus", ErrorKind::InvalidValue),
        ("--agent", "", "reviewer", ErrorKind::InvalidValue),
        ("--name", "", "Review", ErrorKind::ValueValidation),
    ] {
        let error = Cli::try_parse_from(["claude-rs", flag, bad]).unwrap_err();
        assert_eq!(error.kind(), kind, "{flag}: {error}");
        assert!(error.to_string().contains(flag));
        Cli::try_parse_from(["claude-rs", flag, good]).unwrap().validate().unwrap();
    }
    for (args, targets, corrected) in [
        (
            vec!["--continue", "--resume", "session-1"],
            ["--continue", "--resume"],
            vec!["--resume", "session-1"],
        ),
        (
            vec!["--resume", "session-1", "resume"],
            ["interactive startup arguments", "subcommand"],
            vec!["resume", "session-1"],
        ),
    ] {
        let error = Cli::try_parse_from(["claude-rs"].into_iter().chain(args))
            .and_then(|cli| cli.validate())
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        for target in targets {
            assert!(error.to_string().contains(target));
        }
        Cli::try_parse_from(["claude-rs"].into_iter().chain(corrected))
            .unwrap()
            .validate()
            .unwrap();
    }
}

#[tokio::test]
async fn each_completion_registers_the_requested_shell_without_startup_writes() {
    for (shell, registration, option) in [
        ("bash", "complete -F ", "--permission-mode"),
        ("zsh", "#compdef claude-rs", "--permission-mode"),
        ("fish", "complete -c claude-rs", "-l permission-mode"),
        (
            "powershell",
            "Register-ArgumentCompleter -Native -CommandName 'claude-rs'",
            "--permission-mode",
        ),
        ("elvish", "set edit:completion:arg-completer[claude-rs]", "--permission-mode"),
    ] {
        let fixture = Fixture::new();
        let missing = fixture.root.path().join("missing-bridge.js");
        let output = fixture.run(&missing, &["completions".as_ref(), shell.as_ref()]).await;
        assert!(output.status.success(), "{shell}: {}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stderr.is_empty());
        let script = String::from_utf8(output.stdout).unwrap();
        assert!(script.contains(registration) && script.contains(option), "{shell}");
        assert!(
            script.lines().any(|line| line.contains(registration) && line.contains("claude-rs"))
        );
        fixture.assert_preserved(false);
    }
}

#[allow(clippy::unwrap_used)]
fn section<'a>(manual: &'a str, name: &str) -> &'a str {
    manual.split_once(&format!(".SH {name}\n")).unwrap().1.split(".SH ").next().unwrap()
}

#[tokio::test]
async fn manuals_identify_their_command_and_options_without_startup_writes() {
    let fixture = Fixture::new();
    let directory = fixture.root.path().join("nested/man1");
    let output = fixture
        .run(&fixture.root.path().join("missing.js"), &["man".as_ref(), directory.as_os_str()])
        .await;
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    for (file, command, option) in [
        ("claude-rs.1", "claude-rs", "--permission-mode"),
        ("claude-rs-config-show.1", "claude-rs config show", "--json"),
        ("claude-rs-completions.1", "claude-rs completions", "SHELL"),
    ] {
        let manual = std::fs::read_to_string(directory.join(file)).unwrap().replace("\\-", "-");
        assert!(
            manual
                .lines()
                .any(|line| line.starts_with(&format!(".TH {} 1 ", file.trim_end_matches(".1"))))
        );
        assert!(section(&manual, "NAME").starts_with(&command.replace(' ', "-")));
        assert!(section(&manual, "SYNOPSIS").contains(command));
        assert!(section(&manual, "SYNOPSIS").contains(option));
        assert!(section(&manual, "OPTIONS").contains(option));
    }
    assert!(!directory.join("claude-rs-man.1").exists());
    fixture.assert_preserved(false);
}

#[tokio::test]
async fn preflight_reports_the_failing_stage_before_bridge_resolution_or_file_mutation() {
    for occupied in [false, true] {
        let fixture = Fixture::new();
        if occupied {
            std::fs::write(&fixture.log, b"prior log bytes").unwrap();
        }
        std::fs::write(fixture.root.path().join("broken.json"), b"{\"a\":").unwrap();
        std::fs::write(fixture.root.path().join("empty.md"), b"").unwrap();
        for (args, expected, unusable_bridge) in [
            (vec!["--continue", "--resume", "session-1"], vec!["--continue", "--resume"], true),
            (
                vec!["doctor", "--model", "opus"],
                vec!["interactive startup arguments", "subcommand"],
                true,
            ),
            (vec!["--effort", "invalid"], vec!["--effort", "invalid"], true),
            (vec!["--add-dir", "not-there"], vec!["--add-dir", "not-there"], false),
            (
                vec!["--settings", "missing.json"],
                vec!["--settings", "cannot read", "missing.json"],
                false,
            ),
            (
                vec!["--settings", "broken.json"],
                vec!["--settings", "not valid JSON", "broken.json"],
                false,
            ),
            (vec!["--mcp-config", "{\"servers\":{}}"], vec!["--mcp-config", "mcpServers"], false),
            (
                vec!["--system-prompt-file", "empty.md"],
                vec!["--system-prompt-file", "empty.md"],
                false,
            ),
        ] {
            let missing = fixture.root.path().join("missing-bridge.js");
            let output = fixture
                .run(
                    if unusable_bridge { &missing } else { &fixture.script },
                    &args.iter().map(OsStr::new).collect::<Vec<_>>(),
                )
                .await;
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            let error = String::from_utf8(output.stderr).unwrap();
            for cause in expected {
                assert!(error.contains(cause), "{args:?}: {error}");
            }
            if unusable_bridge {
                assert!(!error.contains("missing-bridge.js"));
            }
            fixture.assert_preserved(occupied);
        }
    }
}

#[tokio::test]
async fn path_refusals_preserve_targets_causes_and_files_for_new_and_resumed_launches() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.log, b"prior log bytes").unwrap();
    let wrong_kind = fixture.root.path().join("not-directory");
    std::fs::write(&wrong_kind, b"preserve wrong-kind file").unwrap();
    std::fs::create_dir(fixture.root.path().join("project")).unwrap();
    for resume in [false, true] {
        for (flag, path, reason) in [
            ("--dir", fixture.root.path().join("missing-dir"), "directory does not exist"),
            ("--dir", wrong_kind.clone(), "not a directory"),
            (
                "--bridge-script",
                fixture.root.path().join("missing.js"),
                "bridge script does not exist",
            ),
            ("--bridge-script", fixture.root.path().to_path_buf(), "bridge script is not a file"),
        ] {
            let mut args = vec![];
            let script = if flag == "--bridge-script" { &path } else { &fixture.script };
            if flag == "--dir" {
                args.extend([OsStr::new(flag), path.as_os_str()]);
            }
            if resume {
                args.extend([OsStr::new("resume"), OsStr::new("session-1")]);
            }
            let output = fixture.run(script, &args).await;
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(
                error.contains(flag)
                    && error.contains(reason)
                    && error.contains(path.to_str().unwrap()),
                "{error}"
            );
            fixture.assert_preserved(true);
        }
    }
    for (relative, flag, cause) in
        [("project", "--bridge-script", "missing.js"), ("elsewhere", "--dir", "elsewhere")]
    {
        let output = fixture
            .run(&fixture.root.path().join("missing.js"), &["-C".as_ref(), relative.as_ref()])
            .await;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(flag) && error.contains(cause), "{error}");
        if relative == "project" {
            assert!(!error.contains("--dir"));
        }
        fixture.assert_preserved(true);
    }
    assert_eq!(std::fs::read(wrong_kind).unwrap(), b"preserve wrong-kind file");
}

#[cfg(unix)]
#[tokio::test]
async fn startup_and_doctor_independently_identify_script_access_failures() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let locked = fixture.root.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    let behind_parent = locked.join("bridge.js");
    let unreadable = fixture.root.path().join("unreadable.js");
    for file in [&unreadable, &behind_parent] {
        std::fs::write(file, b"inert").unwrap();
    }
    let runtime = fixture.root.path().join("runtime.sh");
    std::fs::write(
        &runtime,
        b"#!/bin/sh\n[ \"$#\" = 1 ] && [ \"$1\" = --version ] || exit 9\nprintf '1.2.3\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _guards =
        [super::ModeGuard::set(&locked, 0o000), super::ModeGuard::set(&unreadable, 0o000)];
    if !super::permissions_enforced(&behind_parent) {
        return;
    }
    for (script, reason) in
        [(&unreadable, "cannot be read"), (&behind_parent, "cannot be accessed")]
    {
        let launch = fixture.run(script, &[]).await;
        assert_eq!(launch.status.code(), Some(2));
        assert!(launch.stdout.is_empty());
        let error = String::from_utf8(launch.stderr).unwrap();
        assert!(
            error.contains("--bridge-script")
                && error.contains(script.to_str().unwrap())
                && error.contains(reason)
        );
        assert!(!error.contains("does not exist"));
        for strict in [false, true] {
            let mut command = fixture.command(script);
            command.args(["doctor", "--json"]).env("CLAUDE_RS_AGENT_BRIDGE_RUNTIME", &runtime);
            if strict {
                command.arg("--strict");
            }
            let output = tokio::time::timeout(Duration::from_secs(20), command.output())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(output.status.code(), Some(i32::from(strict)));
            assert!(output.stderr.is_empty());
            let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let checks = report["checks"].as_array().unwrap();
            let check = checks.iter().find(|row| row["id"] == "bridge_script").unwrap();
            assert_eq!(check["status"], "fail");
            assert_eq!(check["details"]["explicit_bridge_script"], script.to_str().unwrap());
            let cause = check["message"].as_str().unwrap();
            assert!(cause.contains(reason) && cause.contains(script.to_str().unwrap()));
            assert!(!cause.contains("does not exist"));
            let probe = checks.iter().find(|row| row["id"] == "bridge_runtime_version").unwrap();
            assert_eq!(probe["status"], "pass");
            assert_eq!(probe["details"]["version"], "1.2.3");
        }
        fixture.assert_preserved(false);
    }
}
