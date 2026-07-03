// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::cli::redaction;
use crate::error::AppError;
use serde::Serialize;
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::thread;
use time::{OffsetDateTime, macros::format_description};

pub const LAST_CRASH_FILE_NAME: &str = "last-crash.json";

thread_local! {
    static EXPECTED_PANIC_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CrashMetadata {
    schema: &'static str,
    kind: &'static str,
    created_at: String,
    version: &'static str,
    platform: PlatformMetadata,
    message: String,
    location: Option<String>,
    latest_log_path: Option<String>,
    diagnostics_bundle_command: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct PlatformMetadata {
    os: &'static str,
    arch: &'static str,
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        if !should_report_panic() {
            return;
        }

        let metadata = CrashMetadata::from_panic_info(panic_info);
        if let Err(error) = write_last_crash_metadata(&metadata) {
            tracing::warn!(
                target: crate::logging::targets::APP_LIFECYCLE,
                event_name = "last_crash_metadata_write_failed",
                message = "failed to write last crash metadata",
                outcome = "failure",
                error_message = %error,
            );
        }

        let mut stderr = std::io::stderr().lock();
        let _ = write_panic_report(&mut stderr, &metadata);
    }));
}

pub(crate) fn catch_expected_panic<F, R>(operation: F) -> thread::Result<R>
where
    F: FnOnce() -> R,
{
    let _guard = ExpectedPanicGuard::enter();
    panic::catch_unwind(AssertUnwindSafe(operation))
}

pub fn write_app_error_report(
    writer: &mut impl std::io::Write,
    error: &AppError,
) -> std::io::Result<()> {
    write_app_error_report_with_detail(writer, error, None)
}

pub fn write_app_error_report_with_detail(
    writer: &mut impl std::io::Write,
    error: &AppError,
    detail: Option<&str>,
) -> std::io::Result<()> {
    let latest_log = latest_log_path_display();
    writeln!(writer, "{}", error.report_title())?;
    writeln!(writer, "  category: {}", error.category_tag())?;
    writeln!(writer, "  message: {}", error.user_message())?;
    if let Some(detail) = detail {
        writeln!(writer, "  detail: {}", redaction::redact_line(detail))?;
    }
    writeln!(writer, "  exit_code: {}", error.exit_code())?;
    writeln!(writer, "  version: {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(writer, "  platform: {}/{}", std::env::consts::OS, std::env::consts::ARCH)?;
    match latest_log {
        Some(path) => writeln!(writer, "  latest_log: {path}")?,
        None => writeln!(writer, "  latest_log: not found")?,
    }
    writeln!(writer, "  next_step: {}", error.recommended_command())?;
    writeln!(
        writer,
        "  issue_guidance: paste this report with `claude-rs logs --bundle --yes` output if the problem persists"
    )
}

pub fn last_crash_metadata_path(paths: &crate::logging::DiagnosticsPaths) -> PathBuf {
    paths.root_dir.join(LAST_CRASH_FILE_NAME)
}

pub fn read_last_crash_metadata(paths: &crate::logging::DiagnosticsPaths) -> Option<String> {
    read_last_crash_metadata_from_path(&last_crash_metadata_path(paths))
}

pub fn read_last_crash_metadata_from_path(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(redaction::redact_text(&text))
}

fn write_panic_report(
    writer: &mut impl std::io::Write,
    metadata: &CrashMetadata,
) -> std::io::Result<()> {
    writeln!(writer)?;
    writeln!(writer, "claude-rs crashed unexpectedly")?;
    writeln!(writer, "  category: crash")?;
    writeln!(writer, "  message: {}", metadata.message)?;
    if let Some(location) = &metadata.location {
        writeln!(writer, "  location: {location}")?;
    }
    writeln!(writer, "  version: {}", metadata.version)?;
    writeln!(writer, "  platform: {}/{}", metadata.platform.os, metadata.platform.arch)?;
    match &metadata.latest_log_path {
        Some(path) => writeln!(writer, "  latest_log: {path}")?,
        None => writeln!(writer, "  latest_log: not found")?,
    }
    writeln!(writer, "  next_step: {}", metadata.diagnostics_bundle_command)?;
    writeln!(writer, "  issue_guidance: paste this report and the redacted debug bundle")
}

fn write_last_crash_metadata(metadata: &CrashMetadata) -> anyhow::Result<()> {
    let paths = crate::logging::default_diagnostics_paths()?;
    std::fs::create_dir_all(&paths.root_dir)?;
    let path = last_crash_metadata_path(&paths);
    write_last_crash_metadata_to(metadata, &path)
}

fn write_last_crash_metadata_to(metadata: &CrashMetadata, path: &Path) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(metadata)?;
    std::fs::write(path, format!("{text}\n"))?;
    Ok(())
}

fn should_report_panic() -> bool {
    EXPECTED_PANIC_DEPTH.with(|depth| depth.get() == 0)
}

struct ExpectedPanicGuard;

impl ExpectedPanicGuard {
    fn enter() -> Self {
        EXPECTED_PANIC_DEPTH.with(|depth| depth.set(depth.get().saturating_add(1)));
        Self
    }
}

impl Drop for ExpectedPanicGuard {
    fn drop(&mut self) {
        EXPECTED_PANIC_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

fn latest_log_path_display() -> Option<String> {
    crate::logging::latest_default_log_path().ok().flatten().map(|path| path.display().to_string())
}

impl CrashMetadata {
    fn from_panic_info(panic_info: &PanicHookInfo<'_>) -> Self {
        Self {
            schema: "claude-rs-crash/v1",
            kind: "panic",
            created_at: timestamp_now(),
            version: env!("CARGO_PKG_VERSION"),
            platform: PlatformMetadata { os: std::env::consts::OS, arch: std::env::consts::ARCH },
            message: redaction::redact_line(&panic_message(panic_info)),
            location: panic_info.location().map(|location| {
                format!("{}:{}:{}", location.file(), location.line(), location.column())
            }),
            latest_log_path: latest_log_path_display(),
            diagnostics_bundle_command: "claude-rs logs --bundle --yes",
        }
    }
}

fn panic_message(panic_info: &PanicHookInfo<'_>) -> String {
    if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    if let Some(message) = panic_info.payload().downcast_ref::<String>() {
        return message.clone();
    }
    "panic payload is not a string".to_owned()
}

fn timestamp_now() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z"))
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        CrashMetadata, PlatformMetadata, catch_expected_panic, should_report_panic,
        write_app_error_report_with_detail, write_last_crash_metadata_to,
    };
    use crate::error::AppError;
    use std::panic;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PANIC_HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn app_error_report_is_issue_friendly() {
        let mut output = Vec::new();

        write_app_error_report_with_detail(
            &mut output,
            &AppError::BridgeTimeout,
            Some("ANTHROPIC_API_KEY=sk-ant-secret"),
        )
        .expect("write report");

        let text = String::from_utf8(output).expect("utf8");
        assert!(text.contains("Bridge initialization timeout"));
        assert!(text.contains("category: bridge_timeout"));
        assert!(text.contains("claude-rs doctor --strict"));
        assert!(text.contains("version:"));
        assert!(text.contains("platform:"));
        assert!(text.contains("[redacted]"));
        assert!(!text.contains("sk-ant-secret"));
    }

    #[test]
    fn crash_metadata_file_redacts_secret_payloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("last-crash.json");
        let metadata = CrashMetadata {
            schema: "claude-rs-crash/v1",
            kind: "panic",
            created_at: "2026-07-03T00:00:00Z".to_owned(),
            version: "0",
            platform: PlatformMetadata { os: "test", arch: "test" },
            message: crate::cli::redaction::redact_line("ANTHROPIC_API_KEY=sk-ant-secret"),
            location: Some("src/main.rs:1:1".to_owned()),
            latest_log_path: None,
            diagnostics_bundle_command: "claude-rs logs --bundle --yes",
        };

        write_last_crash_metadata_to(&metadata, &path).expect("write metadata");

        let text = std::fs::read_to_string(path).expect("read metadata");
        assert!(!text.contains("sk-ant-secret"));
        assert!(text.contains("[redacted]"));
    }

    #[test]
    fn expected_caught_panics_do_not_count_as_reportable_crashes() {
        let _guard = PANIC_HOOK_TEST_LOCK.lock().expect("lock panic hook test");
        let original = panic::take_hook();
        let reportable_panic_calls = Arc::new(AtomicUsize::new(0));
        let reportable_panic_calls_for_hook = Arc::clone(&reportable_panic_calls);
        panic::set_hook(Box::new(move |_panic_info| {
            if should_report_panic() {
                reportable_panic_calls_for_hook.fetch_add(1, Ordering::SeqCst);
            }
        }));

        let expected = catch_expected_panic(|| {
            panic!("expected renderer panic");
        });
        assert!(expected.is_err());
        assert_eq!(reportable_panic_calls.load(Ordering::SeqCst), 0);

        let unexpected = panic::catch_unwind(|| {
            panic!("unexpected panic");
        });
        assert!(unexpected.is_err());
        assert_eq!(reportable_panic_calls.load(Ordering::SeqCst), 1);

        let _installed = panic::take_hook();
        panic::set_hook(original);
    }
}
