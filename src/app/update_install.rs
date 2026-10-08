// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Simon Peter Rothgang

//! Update installs after exit: in the terminal when chosen in the update window,
//! in a detached worker process for automatic updates.

use super::{App, AppStatus, SurfaceMode, SystemSeverity, settings};
use crate::install_method::InstallMethod;
use crate::{UpdateWorkerArgs, UpdateWorkerMethod};
use std::ffi::OsString;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

#[cfg(not(target_os = "windows"))]
const UNIX_INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.sh";
#[cfg(target_os = "windows")]
const WINDOWS_INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/srothgan/claude-code-rust/main/scripts/install/install.ps1";

const WORKER_SUBCOMMAND: &str = "update-worker";
const WORKER_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WORKER_LOG_FILE: &str = "claude-rs-update.log";
const WORKER_LOCK_FILE: &str = "claude-rs-update.lock";

/// Where installer processes read and write, and how long they may run.
enum InstallIo<'a> {
    /// The user's terminal, without a time limit.
    Terminal,
    /// The worker log; the process tree is stopped at the deadline.
    Log { file: &'a File, deadline: Instant },
}

impl InstallIo<'_> {
    fn run(&self, command: &mut Command) -> Result<ExitStatus, String> {
        let program = command.get_program().to_string_lossy().into_owned();
        let start_error = |error: std::io::Error| format!("failed to start {program}: {error}");
        match self {
            Self::Terminal => command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .map_err(start_error),
            Self::Log { file, deadline } => {
                let stdout = file.try_clone().map_err(start_error)?;
                let stderr = file.try_clone().map_err(start_error)?;
                command.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
                let mut child = own_process_tree(command).spawn().map_err(start_error)?;
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => return Ok(status),
                        Ok(None) if Instant::now() < *deadline => {
                            std::thread::sleep(WORKER_POLL_INTERVAL);
                        }
                        Ok(None) => {
                            kill_process_tree(&mut child);
                            return Err(format!(
                                "{program} exceeded the update time limit and was stopped"
                            ));
                        }
                        Err(error) => {
                            kill_process_tree(&mut child);
                            return Err(format!("failed to wait for {program}: {error}"));
                        }
                    }
                }
            }
        }
    }
}

/// Install in the user's terminal and return the process exit code.
pub fn run_update_install(app: &App, latest_version: &str, method: &InstallMethod) -> i32 {
    let result = install(latest_version, method, &InstallIo::Terminal);
    let failure = failure_message(method, latest_version, &result);
    report_outcome_to_terminal(app, failure);
    match result {
        Ok(status) if status.success() => 0,
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => 1,
    }
}

/// Hand the install to a detached worker so the shell prompt returns immediately.
pub fn start_background_update_install(app: &App, latest_version: &str, method: &InstallMethod) {
    match spawn_worker(latest_version, method) {
        Ok(()) => eprintln!("Updating claude-rs to v{latest_version} in the background."),
        Err(error) => {
            report_outcome_to_terminal(app, failure_message(method, latest_version, &Err(error)));
        }
    }
}

/// Entry point of the hidden worker subcommand.
pub fn run_update_worker(args: &UpdateWorkerArgs) -> i32 {
    let Some(paths) = WorkerPaths::resolve() else {
        return 1;
    };
    let method = worker_method(args);
    let outcome = run_worker(&paths, &args.version, &method, WORKER_INSTALL_TIMEOUT, |io| {
        install(&args.version, &method, io)
    });
    i32::from(outcome == WorkerOutcome::Failed)
}

/// Report a failed automatic install once the launch transcript is settled.
pub(super) fn maybe_emit_failure_notice(app: &mut App) {
    if app.surface_mode != SurfaceMode::Chat
        || !matches!(app.status, AppStatus::Ready)
        || !app.startup.launch_completed()
    {
        return;
    }
    let Some(error) = app.update_install_failure.take() else { return };
    super::events::push_system_message_with_severity(
        app,
        Some(SystemSeverity::Warning),
        &format!("Automatic update failed and will be retried after the next normal exit. {error}"),
    );
}

fn report_outcome_to_terminal(app: &App, failure: Option<String>) {
    if let Some(message) = &failure {
        eprintln!("{message}");
    }
    if let Some(path) = app.global_settings_path.as_deref()
        && let Err(error) = settings::record_install_outcome(path, failure)
    {
        eprintln!("Failed to update app settings after install: {error}");
    }
}

fn failure_message(
    method: &InstallMethod,
    latest_version: &str,
    result: &Result<ExitStatus, String>,
) -> Option<String> {
    let method_label = method.label();
    match result {
        Ok(status) if status.success() => None,
        Ok(status) => Some(format!(
            "{method_label} update install for v{latest_version} exited with status {status}."
        )),
        Err(error) => Some(format!(
            "Failed to run {method_label} update install for v{latest_version}: {error}"
        )),
    }
}

struct WorkerPaths {
    settings: PathBuf,
    log: PathBuf,
    lock: PathBuf,
}

impl WorkerPaths {
    fn resolve() -> Option<Self> {
        let settings = settings::global_settings_path()?;
        let diagnostics_dir = crate::logging::default_diagnostics_dir().ok()?;
        Some(Self {
            settings,
            log: diagnostics_dir.join(WORKER_LOG_FILE),
            lock: diagnostics_dir.join(WORKER_LOCK_FILE),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerOutcome {
    Installed,
    Failed,
    /// Another worker owns the install; this one changed nothing.
    AlreadyRunning,
}

fn run_worker(
    paths: &WorkerPaths,
    latest_version: &str,
    method: &InstallMethod,
    timeout: Duration,
    install: impl FnOnce(&InstallIo<'_>) -> Result<ExitStatus, String>,
) -> WorkerOutcome {
    let _lock = match lock_worker(&paths.lock) {
        Ok(lock) => lock,
        Err(fs4::TryLockError::WouldBlock) => return WorkerOutcome::AlreadyRunning,
        Err(fs4::TryLockError::Error(error)) => {
            let error = format!("cannot lock {}: {error}", paths.lock.display());
            return finish_worker(paths, None, method, latest_version, &Err(error));
        }
    };
    let log = match File::create(&paths.log) {
        Ok(log) => log,
        Err(error) => {
            let error = format!("cannot create {}: {error}", paths.log.display());
            return finish_worker(paths, None, method, latest_version, &Err(error));
        }
    };
    let _ = writeln!(&log, "Installing claude-rs v{latest_version} ({} install).", method.label());
    let result = install(&InstallIo::Log { file: &log, deadline: Instant::now() + timeout });
    finish_worker(paths, Some(&log), method, latest_version, &result)
}

fn finish_worker(
    paths: &WorkerPaths,
    log: Option<&File>,
    method: &InstallMethod,
    latest_version: &str,
    result: &Result<ExitStatus, String>,
) -> WorkerOutcome {
    let failure = failure_message(method, latest_version, result)
        .map(|message| format!("{message} Log: {}", paths.log.display()));
    let outcome = if failure.is_some() { WorkerOutcome::Failed } else { WorkerOutcome::Installed };
    if let Some(mut log) = log {
        let _ = writeln!(log, "{}", failure.as_deref().unwrap_or("Update installed."));
    }
    if let Err(error) = settings::record_install_outcome(&paths.settings, failure)
        && let Some(mut log) = log
    {
        let _ = writeln!(log, "Failed to update app settings after install: {error}");
    }
    outcome
}

/// The lock is released by the OS when the worker exits, however it exits.
fn lock_worker(path: &Path) -> Result<File, fs4::TryLockError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::options().create(true).truncate(false).write(true).open(path)?;
    fs4::FileExt::try_lock(&file)?;
    Ok(file)
}

fn spawn_worker(latest_version: &str, method: &InstallMethod) -> Result<(), String> {
    let args = worker_args(latest_version, method)
        .ok_or_else(|| "no update install method was selected".to_owned())?;
    let exe = std::env::current_exe()
        .map_err(|error| format!("failed to locate the claude-rs executable: {error}"))?;
    spawn_detached(Command::new(&exe).args(args))
        .map(drop)
        .map_err(|error| format!("failed to start {}: {error}", exe.display()))
}

fn worker_args(latest_version: &str, method: &InstallMethod) -> Option<Vec<OsString>> {
    let mut args: Vec<OsString> =
        vec![WORKER_SUBCOMMAND.into(), latest_version.into(), "--method".into()];
    match method {
        InstallMethod::Npm => args.push("npm".into()),
        InstallMethod::Script { install_dir } => {
            args.push("script".into());
            if let Some(install_dir) = install_dir {
                args.push("--install-dir".into());
                args.push(install_dir.into());
            }
        }
        InstallMethod::Unknown => return None,
    }
    Some(args)
}

fn worker_method(args: &UpdateWorkerArgs) -> InstallMethod {
    match args.method {
        UpdateWorkerMethod::Npm => InstallMethod::Npm,
        UpdateWorkerMethod::Script => {
            InstallMethod::Script { install_dir: args.install_dir.clone() }
        }
    }
}

/// Start `command` without a terminal, console window, or session shared with this process.
fn spawn_detached(command: &mut Command) -> std::io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(std::env::temp_dir());
    detach(command).spawn()
}

#[cfg(windows)]
fn detach(command: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt as _;
    // A hidden console rather than none: npm.cmd and powershell.exe would open their own window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP)
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn detach(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: the hook runs between fork and exec and only calls `setsid`, which is
    // async-signal-safe and touches no memory owned by this process.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
    }
}

#[cfg(windows)]
fn own_process_tree(command: &mut Command) -> &mut Command {
    command
}

#[cfg(unix)]
fn own_process_tree(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0)
}

#[cfg(windows)]
fn kill_process_tree(child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn kill_process_tree(child: &mut Child) {
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `killpg` takes plain integers. The group was created for this child by
        // `own_process_tree`, and the child is not reaped yet, so the id is not reused.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn install(
    latest_version: &str,
    method: &InstallMethod,
    io: &InstallIo<'_>,
) -> Result<ExitStatus, String> {
    match method {
        InstallMethod::Npm => run_npm_update(latest_version, io),
        InstallMethod::Script { install_dir } => {
            run_script_update(latest_version, install_dir.as_deref(), io)
        }
        InstallMethod::Unknown => Err("no update install method was selected".to_owned()),
    }
}

fn run_npm_update(latest_version: &str, io: &InstallIo<'_>) -> Result<ExitStatus, String> {
    let npm = resolve_npm().map_err(|error| format!("failed to resolve npm: {error}"))?;
    io.run(Command::new(&npm).args([
        "install",
        "-g",
        &format!("claude-code-rust@{latest_version}"),
    ]))
}

#[cfg(not(target_os = "windows"))]
fn run_script_update(
    latest_version: &str,
    install_dir: Option<&Path>,
    io: &InstallIo<'_>,
) -> Result<ExitStatus, String> {
    let installer = download_unix_installer(io)?;
    let mut command = Command::new("sh");
    command
        .arg(&installer.script)
        .args(["--release", latest_version, "--yes", "--keep-npm"])
        .env_remove("CLAUDE_RS_RELEASE")
        .env_remove("CLAUDE_RS_INSTALL_DIR")
        .env_remove("CLAUDE_RS_BIN_DIR")
        .env_remove("CLAUDE_RS_NO_MODIFY_PATH")
        .env_remove("CLAUDE_RS_REMOVE_NPM")
        .env_remove("CLAUDE_RS_RUN")
        .env_remove("CLAUDE_RS_UNINSTALL")
        .env_remove("CLAUDE_RS_UPDATE")
        .env_remove("CLAUDE_RS_VERIFY")
        .env("CLAUDE_RS_NON_INTERACTIVE", "1");
    if let Some(install_dir) = install_dir {
        command.arg("--update").arg("--install-dir").arg(install_dir);
    }
    io.run(&mut command)
}

#[cfg(target_os = "windows")]
fn run_script_update(
    latest_version: &str,
    install_dir: Option<&Path>,
    io: &InstallIo<'_>,
) -> Result<ExitStatus, String> {
    let powershell = resolve_powershell()?;
    let mut command = Command::new(&powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!(
                "$ProgressPreference='SilentlyContinue'; Invoke-Expression (Invoke-RestMethod -Uri '{WINDOWS_INSTALLER_URL}')"
            ),
        ])
        .env_remove("CLAUDE_RS_INSTALL_DIR")
        .env_remove("CLAUDE_RS_NO_MODIFY_PATH")
        .env_remove("CLAUDE_RS_REMOVE_NPM")
        .env_remove("CLAUDE_RS_RUN")
        .env_remove("CLAUDE_RS_UNINSTALL")
        .env_remove("CLAUDE_RS_UPDATE")
        .env_remove("CLAUDE_RS_UPDATE_PARENT_PID")
        .env_remove("CLAUDE_RS_VERIFY")
        .env("CLAUDE_RS_RELEASE", latest_version)
        .env("CLAUDE_RS_NON_INTERACTIVE", "1")
        .env("CLAUDE_RS_KEEP_NPM", "1");
    if let Some(install_dir) = install_dir {
        command
            .env("CLAUDE_RS_INSTALL_DIR", install_dir)
            .env("CLAUDE_RS_UPDATE", "1")
            .env("CLAUDE_RS_UPDATE_PARENT_PID", std::process::id().to_string());
    }
    io.run(&mut command)
}

#[cfg(not(target_os = "windows"))]
struct DownloadedInstaller {
    root: PathBuf,
    script: PathBuf,
}

#[cfg(not(target_os = "windows"))]
impl Drop for DownloadedInstaller {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(not(target_os = "windows"))]
fn download_unix_installer(io: &InstallIo<'_>) -> Result<DownloadedInstaller, String> {
    let root = create_update_temp_dir()?;
    let script = root.join("install.sh");
    let installer = DownloadedInstaller { root, script };
    let (downloader, args): (PathBuf, Vec<OsString>) = if let Ok(curl) = which::which("curl") {
        (
            curl,
            vec![
                "-fsSL".into(),
                UNIX_INSTALLER_URL.into(),
                "-o".into(),
                installer.script.as_os_str().to_owned(),
            ],
        )
    } else if let Ok(wget) = which::which("wget") {
        (
            wget,
            vec![
                "-q".into(),
                "-O".into(),
                installer.script.as_os_str().to_owned(),
                UNIX_INSTALLER_URL.into(),
            ],
        )
    } else {
        return Err("neither curl nor wget was found in PATH".to_owned());
    };

    let status = io.run(Command::new(&downloader).args(args))?;
    if !status.success() {
        return Err(format!("installer download exited with status {status}"));
    }
    if !installer.script.is_file() {
        return Err("installer download did not create install.sh".to_owned());
    }
    Ok(installer)
}

#[cfg(not(target_os = "windows"))]
fn create_update_temp_dir() -> Result<PathBuf, String> {
    let base = std::env::temp_dir();
    for attempt in 0..100_u32 {
        let candidate = base.join(format!("claude-rs-update-{}-{attempt}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!("failed to create {}: {error}", candidate.display()));
            }
        }
    }
    Err("could not allocate a temporary update directory".to_owned())
}

fn resolve_npm() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let candidates = ["npm.cmd", "npm"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["npm"];

    candidates
        .iter()
        .find_map(|candidate| which::which(candidate).ok())
        .ok_or_else(|| format!("none of {} were found in PATH", candidates.join(", ")))
}

#[cfg(target_os = "windows")]
fn resolve_powershell() -> Result<PathBuf, String> {
    ["powershell.exe", "pwsh.exe"]
        .iter()
        .find_map(|candidate| which::which(candidate).ok())
        .ok_or_else(|| "neither powershell.exe nor pwsh.exe was found in PATH".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{MessageBlock, MessageRole};

    const VERSION: &str = "9.9.9";
    #[cfg(windows)]
    const OUTLIVES_THE_TEST: &str = "ping -n 120 127.0.0.1 >nul";
    #[cfg(unix)]
    const OUTLIVES_THE_TEST: &str = "sleep 120";

    fn shell(script: &str) -> Command {
        #[cfg(windows)]
        let (program, flag) = ("cmd", "/C");
        #[cfg(unix)]
        let (program, flag) = ("sh", "-c");
        let mut command = Command::new(program);
        command.args([flag, script]);
        command
    }

    /// Blocks until the file named by `RELEASE` exists, then creates the file named by `MARKER`.
    fn gated_command() -> Command {
        #[cfg(windows)]
        let (program, flags, script) = (
            "powershell.exe",
            ["-NoProfile", "-Command"],
            "while (-not (Test-Path -LiteralPath $env:RELEASE)) { Start-Sleep -Milliseconds 20 }; Set-Content -LiteralPath $env:MARKER -Value done",
        );
        #[cfg(unix)]
        let (program, flags, script) = (
            "sh",
            ["-e", "-c"],
            r#"while [ ! -e "$RELEASE" ]; do sleep 0.02; done; : > "$MARKER""#,
        );
        let mut command = Command::new(program);
        command.args(flags).arg(script);
        command
    }

    fn worker_fixture(settings: &str) -> (tempfile::TempDir, WorkerPaths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = WorkerPaths {
            settings: dir.path().join("settings.json"),
            log: dir.path().join("logs").join(WORKER_LOG_FILE),
            lock: dir.path().join("logs").join(WORKER_LOCK_FILE),
        };
        std::fs::write(&paths.settings, settings).expect("settings fixture");
        (dir, paths)
    }

    fn saved_updates(paths: &WorkerPaths) -> serde_json::Value {
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&paths.settings).expect("read settings"))
                .expect("settings JSON");
        saved["updates"].clone()
    }

    fn run_fake_install(paths: &WorkerPaths, timeout: Duration, script: &str) -> WorkerOutcome {
        run_worker(paths, VERSION, &InstallMethod::Npm, timeout, |io| io.run(&mut shell(script)))
    }

    #[test]
    fn worker_records_a_failed_install_and_a_later_success_clears_it() {
        let (_dir, paths) = worker_fixture(r#"{"updates":{"autoInstall":true}}"#);

        let outcome = run_fake_install(&paths, WORKER_INSTALL_TIMEOUT, "echo boom&& exit 3");

        assert_eq!(outcome, WorkerOutcome::Failed);
        let updates = saved_updates(&paths);
        let error = updates["last_install_error"].as_str().expect("recorded failure");
        assert!(error.contains("v9.9.9"), "names the version: {error}");
        assert!(error.contains('3'), "names the exit status: {error}");
        assert!(error.contains(&paths.log.display().to_string()), "names the log: {error}");
        assert_eq!(updates["autoInstall"], true);
        let log = std::fs::read_to_string(&paths.log).expect("worker log");
        assert!(log.contains("boom"), "installer output is captured in the log: {log}");

        let outcome = run_fake_install(&paths, WORKER_INSTALL_TIMEOUT, "exit 0");

        assert_eq!(outcome, WorkerOutcome::Installed);
        let updates = saved_updates(&paths);
        assert!(updates.get("last_install_error").is_none());
        assert_eq!(updates["autoInstall"], true);
    }

    #[test]
    fn second_worker_changes_nothing_while_another_holds_the_lock() {
        let settings = r#"{"updates":{"autoInstall":true,"last_install_error":"earlier failure"}}"#;
        let (_dir, paths) = worker_fixture(settings);
        std::fs::create_dir_all(paths.lock.parent().expect("lock directory")).expect("log dir");
        std::fs::write(&paths.log, "first worker output").expect("first worker log");
        let first_worker = File::create(&paths.lock).expect("lock file");
        fs4::FileExt::try_lock(&first_worker).expect("first worker owns the lock");
        let mut installer_ran = false;

        let outcome =
            run_worker(&paths, VERSION, &InstallMethod::Npm, WORKER_INSTALL_TIMEOUT, |_| {
                installer_ran = true;
                Err("must not run".to_owned())
            });

        assert_eq!(outcome, WorkerOutcome::AlreadyRunning);
        assert!(!installer_ran);
        assert_eq!(std::fs::read_to_string(&paths.settings).expect("settings"), settings);
        assert_eq!(std::fs::read_to_string(&paths.log).expect("log"), "first worker output");
    }

    #[test]
    fn worker_stops_an_installer_that_exceeds_the_time_limit() {
        let (_dir, paths) = worker_fixture("{}");
        let started = Instant::now();

        let outcome = run_fake_install(&paths, Duration::from_millis(300), OUTLIVES_THE_TEST);

        assert!(started.elapsed() < Duration::from_secs(60), "the installer was not stopped");
        assert_eq!(outcome, WorkerOutcome::Failed);
        let updates = saved_updates(&paths);
        let error = updates["last_install_error"].as_str().expect("recorded failure");
        assert!(error.contains("time limit"), "{error}");
    }

    #[test]
    fn detached_spawn_returns_while_the_command_is_still_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let release = dir.path().join("release");
        let marker = dir.path().join("marker");
        let mut command = gated_command();
        command.env("RELEASE", &release).env("MARKER", &marker);

        let mut child = spawn_detached(&mut command).expect("spawn");

        assert!(!marker.exists(), "spawning must not wait for the command");
        #[cfg(unix)]
        assert_leads_its_own_session(&child);
        std::fs::write(&release, "").expect("release the command");
        assert!(child.wait().expect("wait").success());
        assert!(marker.exists(), "the detached command ran to completion");
    }

    #[cfg(unix)]
    #[allow(unsafe_code)]
    fn assert_leads_its_own_session(child: &Child) {
        let pid = libc::pid_t::try_from(child.id()).expect("pid");
        // SAFETY: `getsid` takes a plain integer and the child is still running.
        let session = unsafe { libc::getsid(pid) };
        assert_eq!(session, pid, "the command must not share the terminal session");
    }

    #[test]
    fn worker_arguments_parse_back_to_the_requested_install() {
        use clap::Parser as _;
        let methods = [
            InstallMethod::Npm,
            InstallMethod::Script { install_dir: None },
            InstallMethod::Script { install_dir: Some(PathBuf::from("install dir")) },
        ];
        for method in methods {
            let args = worker_args(VERSION, &method).expect("worker arguments");
            let cli = crate::Cli::try_parse_from(
                std::iter::once(OsString::from("claude-rs")).chain(args),
            )
            .expect("worker command line");
            cli.validate().expect("valid worker command line");
            let Some(crate::Command::UpdateWorker(parsed)) = cli.command else {
                panic!("expected the worker subcommand");
            };
            assert_eq!(parsed.version, VERSION);
            assert_eq!(worker_method(&parsed), method);
        }
        assert!(worker_args(VERSION, &InstallMethod::Unknown).is_none());
    }

    #[test]
    fn failure_notice_waits_for_the_launch_transcript_and_is_shown_once() {
        use clap::Parser as _;
        let cli = crate::Cli::try_parse_from(["claude-rs"]).expect("CLI");
        let mut app = App::test_default();
        app.startup = crate::app::state::StartupState::from_cli(&cli);
        app.update_install_failure = Some("npm update install for v9.9.9 failed.".to_owned());
        let warnings = |app: &App| {
            app.transcript
                .messages
                .iter()
                .filter(|message| {
                    matches!(message.role, MessageRole::System(Some(SystemSeverity::Warning)))
                })
                .filter_map(|message| match message.blocks.first() {
                    Some(MessageBlock::Text(block)) => Some(block.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        maybe_emit_failure_notice(&mut app);
        assert!(warnings(&app).is_empty(), "the launch transcript is not settled yet");

        app.startup.complete_launch();
        maybe_emit_failure_notice(&mut app);
        maybe_emit_failure_notice(&mut app);

        let warnings = warnings(&app);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("npm update install for v9.9.9 failed."), "{warnings:?}");
    }
}
