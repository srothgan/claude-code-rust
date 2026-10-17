// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use claude_code_rust::{Cli, StartupLaunch};
use std::process::Command;

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
    let cli = Cli::try_parse_from([
        "claude-rs",
        "resume",
        "session-1",
        "--model",
        "opus",
        "--effort",
        "max",
        "--permission-mode",
        "plan",
        "--agent",
        "reviewer",
    ])
    .expect("parse options");
    cli.validate().expect("valid launch");
    assert_eq!(cli.session_options.model.as_deref(), Some("opus"));
    assert_eq!(cli.session_options.effort, Some(claude_code_rust::agent::model::EffortLevel::Max));
    assert_eq!(cli.session_options.permission_mode.unwrap().as_stored(), "plan");
    assert_eq!(cli.session_options.agent.as_deref(), Some("reviewer"));
}

#[test]
fn contradictory_or_invalid_startup_arguments_are_rejected() {
    for args in [
        vec!["claude-rs", "--continue", "--resume", "session-1"],
        vec!["claude-rs", "--resume", "session-1", "resume"],
        vec!["claude-rs", "--continue", "doctor"],
        vec!["claude-rs", "doctor", "--model", "opus"],
        vec!["claude-rs", "--effort", "invalid"],
        vec!["claude-rs", "--permission-mode", "invalid"],
        vec!["claude-rs", "--model", ""],
        vec!["claude-rs", "--agent", ""],
        vec!["claude-rs", "--name", ""],
        vec!["claude-rs", "--fork-session"],
        vec!["claude-rs", "--fork-session", "explain this project"],
        vec!["claude-rs", "doctor", "--add-dir", "src"],
        vec!["claude-rs", "doctor", "--system-prompt", "a"],
        vec!["claude-rs", "--system-prompt", "a", "--system-prompt-file", "prompt.md"],
        vec!["claude-rs", "--append-system-prompt", "a", "--append-system-prompt-file", "a.md"],
    ] {
        assert!(
            Cli::try_parse_from(&args).and_then(|cli| cli.validate()).is_err(),
            "accepted {args:?}"
        );
    }
}

#[test]
fn executable_generates_every_shell_without_a_bridge() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let output = Command::new(env!("CARGO_BIN_EXE_claude-rs"))
            .args(["--bridge-script", "missing-bridge.js", "completions", shell])
            .output()
            .expect("run completions");
        assert!(output.status.success(), "{shell}: {}", String::from_utf8_lossy(&output.stderr));
        let script = String::from_utf8(output.stdout).expect("UTF-8 script");
        assert!(script.contains("claude-rs"), "{shell}");
        assert!(script.contains("permission-mode"), "{shell}");
        assert!(script.contains("model"), "{shell}");
    }
}

#[test]
fn executable_rejects_startup_conflicts_before_resolving_the_bridge() {
    for args in [
        vec!["--continue", "--resume", "session-1"],
        vec!["doctor", "--model", "opus"],
        vec!["--effort", "invalid"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_claude-rs"))
            .args(["--bridge-script", "missing-bridge.js"])
            .args(&args)
            .output()
            .expect("run invalid arguments");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("error:"), "{error}");
        assert!(!error.contains("missing-bridge.js"), "runtime resolved the bridge: {error}");
    }
}

#[test]
fn executable_rejects_unusable_launch_configuration_before_anything_starts() {
    let project = tempfile::tempdir().expect("project");
    std::fs::write(project.path().join("broken.json"), "{ \"a\": ").expect("broken settings");
    std::fs::write(project.path().join("empty.md"), "").expect("empty prompt");
    // A usable script, so the up-front `--bridge-script` check passes and each flag below
    // is the only thing wrong; the absent log shows nothing started.
    let bridge = project.path().join("bridge.js");
    std::fs::write(&bridge, "").expect("bridge script");
    let log = project.path().join("runtime.log");
    for (args, expected) in [
        (vec!["--add-dir", "not-there"], "--add-dir: "),
        (vec!["--settings", "missing.json"], "--settings: cannot read "),
        (vec!["--settings", "broken.json"], "is not valid JSON"),
        (vec!["--mcp-config", "{\"servers\":{}}"], "must contain an \"mcpServers\" object"),
        (vec!["--system-prompt-file", "empty.md"], "--system-prompt-file: "),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_claude-rs"))
            .arg("--bridge-script")
            .arg(&bridge)
            .arg("--log-file")
            .arg(&log)
            .arg("-C")
            .arg(project.path())
            .args(&args)
            .output()
            .expect("run with an unusable flag");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(expected), "{args:?}: {error}");
        assert!(!log.exists(), "{args:?}: the launch started logging");
    }
}

#[test]
fn executable_generates_manuals_without_a_bridge() {
    let dir = tempfile::tempdir().expect("manual directory");
    let output_dir = dir.path().join("nested/man1");
    let output = Command::new(env!("CARGO_BIN_EXE_claude-rs"))
        .args(["--bridge-script", "missing-bridge.js", "man"])
        .arg(&output_dir)
        .output()
        .expect("run man generator");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let manual = std::fs::read_to_string(output_dir.join("claude-rs.1")).expect("root manual");
    for flag in ["model", "effort", "permission", "continue", "resume"] {
        assert!(manual.contains(flag), "missing {flag}");
    }
    assert!(output_dir.join("claude-rs-config-show.1").is_file());
    assert!(output_dir.join("claude-rs-completions.1").is_file());
    assert!(!output_dir.join("claude-rs-man.1").exists());
}

/// Runs the real binary with a scratch profile and returns it with its output.
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
fn executable_refuses_a_bad_dir_or_bridge_script_before_startup() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let file = scratch.path().join("notes.txt");
    std::fs::write(&file, "").expect("file");
    let missing_dir = scratch.path().join("missing-dir");
    let missing_script = scratch.path().join("missing-bridge.js");
    let scratch_dir = scratch.path().to_path_buf();
    let cases = [
        ("--dir", &missing_dir, "directory does not exist"),
        ("--dir", &file, "not a directory"),
        ("--bridge-script", &missing_script, "bridge script does not exist"),
        ("--bridge-script", &scratch_dir, "bridge script is not a file"),
    ];
    for (flag, path, reason) in cases {
        let (profile, output) =
            run_with_scratch_profile(&[flag.as_ref(), path.as_os_str()], scratch.path());
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{flag} {}: {error}", path.display());
        assert!(output.stdout.is_empty(), "{flag}: {}", String::from_utf8_lossy(&output.stdout));
        assert!(error.contains(flag), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(error.contains(reason), "{error}");
        // The trust prompt and settings are never reached.
        let saved = std::fs::read_dir(profile.path()).expect("profile").count();
        assert_eq!(saved, 0, "{flag} left files in the profile");
    }
}

#[test]
fn resume_with_a_bad_path_is_refused_like_a_plain_launch() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let missing_dir = scratch.path().join("missing-dir");
    let missing_script = scratch.path().join("missing-bridge.js");
    for (flag, path) in [("--dir", &missing_dir), ("--bridge-script", &missing_script)] {
        let (profile, output) = run_with_scratch_profile(
            &[flag.as_ref(), path.as_os_str(), "resume".as_ref(), "session-1".as_ref()],
            scratch.path(),
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{flag}: {error}");
        assert!(error.contains(flag) && error.contains(&path.display().to_string()), "{error}");
        assert_eq!(std::fs::read_dir(profile.path()).expect("profile").count(), 0, "{flag}");
    }
}

#[test]
fn executable_resolves_a_relative_dir_against_the_current_directory() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    std::fs::create_dir(scratch.path().join("project")).expect("project");
    // `project` exists next to the working directory, so only the script is wrong.
    let (_profile, output) = run_with_scratch_profile(
        &["-C".as_ref(), "project".as_ref(), "--bridge-script".as_ref(), "missing.js".as_ref()],
        scratch.path(),
    );
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{error}");
    assert!(error.contains("--bridge-script"), "{error}");
    assert!(!error.contains("--dir"), "{error}");

    let (_profile, output) =
        run_with_scratch_profile(&["-C".as_ref(), "elsewhere".as_ref()], scratch.path());
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{error}");
    assert!(error.contains("--dir"), "{error}");
}

/// Runs `claude-rs <args> --json` from `cwd` and parses its report.
#[allow(clippy::expect_used)]
fn json_report(args: &[&str], cwd: &std::path::Path) -> serde_json::Value {
    let mut full: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    full.push("--json".as_ref());
    let (_profile, output) = run_with_scratch_profile(&full, cwd);
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("json report")
}

#[test]
fn config_and_doctor_read_the_directory_a_relative_dir_names() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let project = scratch.path().join("project");
    std::fs::create_dir_all(project.join("sub")).expect("project");
    // Without `--dir` the root is the process directory, which `--dir .` and a dotted
    // relative path name too.
    let root = json_report(&["config", "path"], &project)["project_root"].clone();
    assert!(root.as_str().is_some_and(|root| std::path::Path::new(root).is_absolute()), "{root}");
    let local_settings = std::path::Path::new(root.as_str().expect("root"))
        .join(".claude")
        .join("settings.local.json");
    for dir in [".", "sub/.."] {
        let config = json_report(&["--dir", dir, "config", "path"], &project);
        assert_eq!(config["project_root"], root, "--dir {dir}");
        let doctor = json_report(&["--dir", dir, "doctor"], &project);
        let local = doctor["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .find(|check| check["id"] == "config_local_settings")
            .expect("local settings check");
        assert_eq!(
            local["details"]["path"].as_str().map(std::path::Path::new),
            Some(local_settings.as_path()),
            "--dir {dir}"
        );
    }
}

#[test]
fn doctor_still_reports_a_bad_bridge_script_instead_of_refusing_it() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let missing = scratch.path().join("missing-bridge.js");
    let (_profile, output) = run_with_scratch_profile(
        &["--bridge-script".as_ref(), missing.as_os_str(), "doctor".as_ref(), "--json".as_ref()],
        scratch.path(),
    );
    assert_ne!(output.status.code(), Some(2), "{}", String::from_utf8_lossy(&output.stderr));
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(report.contains("bridge script does not exist"), "{report}");
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

/// The `bridge_script` check of `claude-rs [--bridge-script <script>] doctor --json`.
#[cfg(unix)]
#[allow(clippy::expect_used)]
fn doctor_bridge_script_check(
    script: &std::path::Path,
    cwd: &std::path::Path,
) -> serde_json::Value {
    let (_profile, output) = run_with_scratch_profile(
        &["--bridge-script".as_ref(), script.as_os_str(), "doctor".as_ref(), "--json".as_ref()],
        cwd,
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("doctor json");
    report["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["id"] == "bridge_script")
        .cloned()
        .expect("bridge_script check")
}

#[cfg(unix)]
#[test]
fn startup_and_doctor_judge_an_unusable_bridge_script_alike() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let unreadable = scratch.path().join("unreadable.js");
    std::fs::write(&unreadable, "").expect("script");
    let parent = scratch.path().join("locked-parent");
    std::fs::create_dir(&parent).expect("parent");
    let behind_parent = parent.join("bridge.js");
    std::fs::write(&behind_parent, "").expect("script behind parent");
    let _guards = [ModeGuard::set(&unreadable, 0o000), ModeGuard::set(&parent, 0o000)];
    if !permissions_enforced(&behind_parent) {
        return;
    }
    for (script, reason) in
        [(&unreadable, "cannot be read"), (&behind_parent, "cannot be accessed")]
    {
        let check = doctor_bridge_script_check(script, scratch.path());
        let verdict = check["message"].as_str().expect("doctor message");
        assert_eq!(check["status"], "fail", "{}: {check}", script.display());
        assert!(verdict.contains(reason) && !verdict.contains("does not exist"), "{verdict}");

        let (_profile, output) = run_with_scratch_profile(
            &["--bridge-script".as_ref(), script.as_os_str()],
            scratch.path(),
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{}: {error}", script.display());
        assert!(error.contains("--bridge-script") && error.contains(verdict), "{error}");
    }
}
