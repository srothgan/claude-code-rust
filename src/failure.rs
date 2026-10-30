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
    install_panic_hook_with_paths(crate::logging::default_diagnostics_paths);
}

fn install_panic_hook_with_paths(
    resolve_paths: impl Fn() -> anyhow::Result<crate::logging::DiagnosticsPaths> + Send + Sync + 'static,
) {
    std::panic::set_hook(Box::new(move |panic_info| {
        if !should_report_panic() {
            return;
        }

        let paths = resolve_paths();
        let metadata = CrashMetadata::from_panic_info(
            panic_info,
            latest_log_path_display(paths.as_ref().ok()),
        );
        if let Err(error) = paths.and_then(|paths| write_last_crash_metadata(&metadata, &paths)) {
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

pub fn write_app_error_report_with_detail(
    writer: &mut impl std::io::Write,
    error: &AppError,
    detail: Option<&str>,
) -> std::io::Result<()> {
    write_app_error_report_with_paths(
        writer,
        error,
        detail,
        crate::logging::default_diagnostics_paths,
    )
}

fn write_app_error_report_with_paths(
    writer: &mut impl std::io::Write,
    error: &AppError,
    detail: Option<&str>,
    resolve_paths: impl FnOnce() -> anyhow::Result<crate::logging::DiagnosticsPaths>,
) -> std::io::Result<()> {
    let latest_log = latest_log_path_display(resolve_paths().ok().as_ref());
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

fn write_last_crash_metadata(
    metadata: &CrashMetadata,
    paths: &crate::logging::DiagnosticsPaths,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.root_dir)?;
    let path = last_crash_metadata_path(paths);
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

fn latest_log_path_display(paths: Option<&crate::logging::DiagnosticsPaths>) -> Option<String> {
    let paths = paths?;
    crate::logging::latest_log_path_in(&paths.runtime_dir, &paths.legacy_log_path)
        .ok()
        .flatten()
        .map(|path| path.display().to_string())
}

impl CrashMetadata {
    fn from_panic_info(panic_info: &PanicHookInfo<'_>, latest_log_path: Option<String>) -> Self {
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
            latest_log_path,
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
#[path = "failure_audit_tests.rs"]
mod audit_tests;
