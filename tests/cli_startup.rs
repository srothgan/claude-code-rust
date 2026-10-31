// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use claude_code_rust::{Cli, StartupLaunch};
#[cfg(unix)]
use std::process::Command;

#[path = "support/startup_audit.rs"]
mod audit_tests;

#[test]
fn interactive_startup_flags_normalize_to_one_launch() {
    let cases = [
        (vec!["claude-rs"], StartupLaunch::NewSession, None),
        (
            vec!["claude-rs", "explain this project"],
            StartupLaunch::NewSession,
            Some("explain this project"),
        ),
        (vec!["claude-rs", "--resume"], StartupLaunch::SessionPicker, None),
        (
            vec!["claude-rs", "--resume", "--", "initial prompt"],
            StartupLaunch::SessionPicker,
            Some("initial prompt"),
        ),
        (
            vec!["claude-rs", "--resume=session-1", "initial prompt"],
            StartupLaunch::ResumeSession { session_id: "session-1".into() },
            Some("initial prompt"),
        ),
        (
            vec!["claude-rs", "-r", "session-1", "continue reviewing"],
            StartupLaunch::ResumeSession { session_id: "session-1".into() },
            Some("continue reviewing"),
        ),
        (
            vec!["claude-rs", "resume", "session-1", "continue reviewing"],
            StartupLaunch::ResumeSession { session_id: "session-1".into() },
            Some("continue reviewing"),
        ),
        (vec!["claude-rs", "resume"], StartupLaunch::SessionPicker, None),
        (
            vec!["claude-rs", "-c", "continue reviewing"],
            StartupLaunch::ContinueSession,
            Some("continue reviewing"),
        ),
    ];
    for (args, launch, prompt) in cases {
        let cli = Cli::try_parse_from(&args).expect("parse startup");
        cli.validate().expect("valid launch");
        assert_eq!(cli.startup_launch(), launch, "{args:?}");
        assert_eq!(cli.initial_prompt(), prompt, "{args:?}");
    }
}

#[test]
fn session_options_support_existing_resume_syntax() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("docs")).unwrap();
    let options = [
        "--model",
        "opus",
        "--effort",
        "max",
        "--permission-mode",
        "plan",
        "--agent",
        "reviewer",
        "--add-dir",
        "docs",
        "--name",
        "Experiment",
    ];
    for before in [true, false] {
        let mut args = vec!["claude-rs", "--dir", project.path().to_str().unwrap()];
        if before {
            args.extend(options);
        }
        args.extend(["resume", "session-1", "review prompt"]);
        if !before {
            args.extend(options);
        }
        let cli = Cli::try_parse_from(args).unwrap();
        cli.validate().unwrap();
        assert_eq!(
            cli.startup_launch(),
            StartupLaunch::ResumeSession { session_id: "session-1".into() }
        );
        assert_eq!(cli.initial_prompt(), Some("review prompt"));
        assert_eq!(cli.session_options.model.as_deref(), Some("opus"));
        assert_eq!(
            cli.session_options.effort,
            Some(claude_code_rust::agent::model::EffortLevel::Max)
        );
        assert_eq!(cli.session_options.permission_mode.unwrap().as_stored(), "plan");
        assert_eq!(cli.session_options.agent.as_deref(), Some("reviewer"));
        assert_eq!(cli.session_options.name.as_deref(), Some("Experiment"));
        assert_eq!(
            cli.resolve_session_config().unwrap().additional_directories,
            [project.path().join("docs").to_str().unwrap()]
        );
    }
}

/// Runs the real binary with a scratch profile and returns it with its output.
#[cfg(unix)]
#[allow(clippy::expect_used)]
fn run_with_scratch_profile(
    args: &[&std::ffi::OsStr],
    cwd: &std::path::Path,
) -> (tempfile::TempDir, std::process::Output) {
    let profile = tempfile::tempdir().expect("profile directory");
    let output = Command::new(env!("CARGO_BIN_EXE_claude-rs"))
        .args(args)
        .current_dir(cwd)
        .env("CLAUDE_CONFIG_DIR", profile.path())
        .output()
        .expect("run claude-rs");
    (profile, output)
}

#[test]
fn launch_paths_that_exist_are_accepted_for_every_interactive_launch() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let script = scratch.path().join("bridge.js");
    std::fs::write(&script, "").expect("script");
    let paths: [&std::ffi::OsStr; 4] =
        ["-C".as_ref(), scratch.path().as_os_str(), "--bridge-script".as_ref(), script.as_os_str()];
    for selection in [&[][..], &["--resume"][..], &["--continue"][..], &["resume", "session-1"][..]]
    {
        let mut args: Vec<&std::ffi::OsStr> = vec!["claude-rs".as_ref()];
        if selection.first() == Some(&"resume") {
            args.extend(paths);
            args.extend(selection.iter().map(std::ffi::OsStr::new));
        } else {
            args.extend(selection.iter().map(std::ffi::OsStr::new));
            args.extend(paths);
        }
        let cli = Cli::try_parse_from(&args).expect("parse");
        cli.validate().unwrap_or_else(|error| panic!("{args:?}: {error}"));
    }
}
/// Sets `path`'s mode for the length of a test and opens it again on drop, so a failed
/// assertion still leaves the scratch directory removable.
#[cfg(unix)]
struct ModeGuard(std::path::PathBuf);

#[cfg(unix)]
impl ModeGuard {
    #[allow(clippy::expect_used)]
    fn set(path: &std::path::Path, mode: u32) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
        Self(path.to_path_buf())
    }
}

#[cfg(unix)]
impl Drop for ModeGuard {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

/// Root passes every permission check, which leaves nothing to refuse: such a run skips
/// explicitly rather than passing without asserting.
/// The probe opens `behind_lock`, a path the test locked away, rather than repeating the
/// check under test, so a broken check fails instead of being skipped.
#[cfg(unix)]
fn permissions_enforced(behind_lock: &std::path::Path) -> bool {
    let enforced = std::fs::File::open(behind_lock)
        .is_err_and(|err| err.kind() == std::io::ErrorKind::PermissionDenied);
    if !enforced {
        eprintln!("skipped: permissions are not enforced for this user (root?)");
    }
    enforced
}

#[cfg(unix)]
#[test]
fn a_dir_needs_both_read_and_search_permission() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let make = |name: &str| {
        let dir = scratch.path().join(name);
        std::fs::create_dir(&dir).expect("dir");
        std::fs::write(dir.join("inside.txt"), "").expect("file inside");
        dir
    };
    // Read without search lists the names but cannot enter the directory or open
    // anything in it; search without read enters it but cannot list it.
    let no_search = make("read-only");
    let no_read = make("search-only");
    let both = make("read-and-search");
    let _guards = [
        ModeGuard::set(&no_search, 0o600),
        ModeGuard::set(&no_read, 0o300),
        ModeGuard::set(&both, 0o500),
    ];
    if !permissions_enforced(&no_search.join("inside.txt")) {
        return;
    }
    for (dir, reason) in [(&no_search, "cannot be entered"), (&no_read, "cannot be read")] {
        let (profile, output) =
            run_with_scratch_profile(&["--dir".as_ref(), dir.as_os_str()], scratch.path());
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{}: {error}", dir.display());
        assert!(error.contains("--dir") && error.contains(reason), "{error}");
        assert!(error.contains(&dir.display().to_string()), "{error}");
        assert_eq!(std::fs::read_dir(profile.path()).expect("profile").count(), 0);
    }
    let cli = Cli::try_parse_from(["claude-rs".as_ref(), "-C".as_ref(), both.as_os_str()])
        .expect("parse");
    cli.validate().unwrap_or_else(|error| panic!("read and search: {error}"));
}

#[cfg(unix)]
#[test]
fn a_dir_behind_an_unsearchable_parent_is_reported_as_inaccessible_not_missing() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let parent = scratch.path().join("locked-parent");
    std::fs::create_dir_all(parent.join("project")).expect("project");
    let _guard = ModeGuard::set(&parent, 0o000);
    if !permissions_enforced(&parent.join("project")) {
        return;
    }
    let project = parent.join("project");
    let error = Cli::try_parse_from(["claude-rs".as_ref(), "-C".as_ref(), project.as_os_str()])
        .expect("parse")
        .validate()
        .expect_err("dir behind a locked parent")
        .to_string();
    assert!(error.contains("cannot be accessed"), "{error}");
    assert!(!error.contains("does not exist"), "{error}");
}

#[tokio::test]
async fn explicit_logging_failure_exits_one_before_bridge_startup_and_preserves_input() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("inert bridge.js");
    std::fs::write(&script, b"throw new Error('must never run');").unwrap();
    let blocker = root.path().join("blocker");
    std::fs::write(&blocker, b"keep blocker").unwrap();
    let target = blocker.join("explicit.log");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_claude-rs"));
    command
        .current_dir(root.path())
        .arg("--bridge-script")
        .arg(&script)
        .arg("--log-file")
        .arg(&target)
        .arg("--no-update-check")
        .env_remove("RUST_LOG")
        .env("CLAUDE_CONFIG_DIR", root.path().join("profile"))
        .env("CLAUDE_RS_CONFIG_DIR", root.path().join("app-profile"))
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(20), command.output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("failed to create log directory"));
    assert!(error.contains(blocker.to_str().unwrap()));
    assert!(!error.contains("claude-rs resume"));
    assert_eq!(std::fs::read(blocker).unwrap(), b"keep blocker");
    assert_eq!(std::fs::read(script).unwrap(), b"throw new Error('must never run');");
    assert!(!root.path().join("app-profile").exists());
    assert!(!root.path().join("profile").exists());
}

// Default diagnostics lookup honors XDG_DATA_HOME on Linux. Other platforms must
// use an independently verified native data-directory isolation mechanism.
#[cfg(all(target_os = "linux", debug_assertions))]
#[tokio::test]
async fn wrapped_startup_errors_keep_their_typed_exit_and_report_without_starting_a_session() {
    for (missing_runtime, code, category) in
        [(true, 20, "environment"), (false, 21, "bridge_spawn")]
    {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let script = root.path().join("inert bridge.js");
        std::fs::write(&script, b"throw new Error('must never run');").unwrap();
        let missing =
            root.path().join(if missing_runtime { "missing-runtime" } else { "missing-script.js" });
        let mut probe = tokio::process::Command::new(env!("CARGO_BIN_EXE_claude-rs"));
        probe
            .args(["logs", "--path"])
            .current_dir(&project)
            .env("XDG_DATA_HOME", &data)
            .env("HOME", root.path().join("home"))
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let selected = tokio::time::timeout(std::time::Duration::from_secs(20), probe.output())
            .await
            .unwrap()
            .unwrap();
        assert!(selected.status.success());
        assert_eq!(
            String::from_utf8(selected.stdout).unwrap(),
            format!("{}\n", data.join("claude-code-rust/logs/runtime").display())
        );
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_claude-rs"));
        command
            .current_dir(&project)
            .args(["--no-update-check", "--log-file"])
            .arg(root.path().join("explicit.log"))
            .env("XDG_DATA_HOME", &data)
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("HOME", root.path().join("home"))
            .env("CLAUDE_CONFIG_DIR", root.path().join("profile"))
            .env("CLAUDE_RS_CONFIG_DIR", root.path().join("app-profile"))
            .env_remove("RUST_LOG")
            .env_remove("CLAUDE_RS_AGENT_BRIDGE")
            .env("CLAUDE_RS_AGENT_BRIDGE_RUNTIME", &missing)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        if missing_runtime {
            command.arg("--bridge-script").arg(&script);
        } else {
            command.env("CLAUDE_RS_AGENT_BRIDGE", &missing);
        }
        let output = tokio::time::timeout(std::time::Duration::from_secs(20), command.output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output.status.code(), Some(code));
        assert!(output.stdout.is_empty());
        let text = String::from_utf8(output.stderr).unwrap();
        let field = |name: &str| {
            text.lines().find_map(|line| line.trim().strip_prefix(&format!("{name}: ")))
        };
        assert_eq!(field("category"), Some(category));
        assert_eq!(field("exit_code"), Some(code.to_string().as_str()));
        assert_eq!(field("next_step"), Some("claude-rs doctor --strict"));
        assert!(field("detail").unwrap().contains(missing.to_str().unwrap()));
        assert!(!text.contains("claude-rs resume"));
        assert!(!root.path().join("app-profile").exists());
        assert!(!root.path().join("profile").exists());
        assert_eq!(std::fs::read(script).unwrap(), b"throw new Error('must never run');");
    }
}
