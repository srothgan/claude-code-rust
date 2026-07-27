// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::{Cli, DiagnosticsPreset};
use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::fmt;
use std::fs::{File, OpenOptions, create_dir_all, metadata, read_dir, remove_file, rename};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use time::{OffsetDateTime, format_description::well_known::Rfc3339, macros::format_description};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::registry::LookupSpan;

pub mod targets {
    pub const APP_AUTH: &str = "app.auth";
    pub const APP_CACHE: &str = "app.cache";
    pub const APP_CONFIG: &str = "app.config";
    pub const APP_COMMAND: &str = "app.command";
    pub const APP_FILE_INDEX: &str = "app.file_index";
    pub const APP_INPUT: &str = "app.input";
    pub const APP_LIFECYCLE: &str = "app.lifecycle";
    pub const APP_NETWORK: &str = "app.network";
    pub const APP_PASTE: &str = "app.paste";
    pub const APP_PERF: &str = "app.perf";
    pub const APP_PERMISSION: &str = "app.permission";
    pub const APP_RENDER: &str = "app.render";
    pub const APP_SESSION: &str = "app.session";
    pub const APP_TOOL: &str = "app.tool";
    pub const APP_UPDATE: &str = "app.update";
    pub const BRIDGE_LIFECYCLE: &str = "bridge.lifecycle";
    pub const BRIDGE_MCP: &str = "bridge.mcp";
    pub const BRIDGE_PERMISSION: &str = "bridge.permission";
    pub const BRIDGE_PROTOCOL: &str = "bridge.protocol";
    pub const BRIDGE_SDK: &str = "bridge.sdk";
}

const BRIDGE_LOG_SCHEMA: &str = "claude-rs-log/v1";
const BASELINE_LOG_SCHEMA: &str = "claude-rs-baseline/v1";
const BRIDGE_LINE_PREVIEW_LIMIT: usize = 240;
const DEFAULT_LOG_DIR: &str = "claude-code-rust";
const DEFAULT_LOG_FILE_NAME: &str = "claude-rs.log";
const DEFAULT_RUNTIME_LOG_SUBDIR: &str = "runtime";
const DEFAULT_PERF_LOG_SUBDIR: &str = "perf";
const DEFAULT_RUNTIME_LOG_PREFIX: &str = "claude-rs";
const DEFAULT_PERF_LOG_PREFIX: &str = "claude-rs-perf";
const RUNTIME_LOG_EXTENSION: &str = "log";
const PERF_LOG_EXTENSION: &str = "jsonl";
const LOG_ROTATION_MAX_BYTES: u64 = 10 * 1024 * 1024;
const LOG_ROTATION_MAX_FILES: usize = 5;
const LOG_RETENTION_MAX_BYTES: u64 = 256 * 1024 * 1024;
const LOG_RETENTION_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const LOG_RETENTION_MAX_FILES: usize = 100;
const LOG_RETENTION_MIN_FILES: usize = 10;
const DEFAULT_BASELINE_FILTER: &str = "warn,app.lifecycle=info";
static BRIDGE_DIAGNOSTICS_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsPaths {
    pub root_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub legacy_log_path: PathBuf,
    pub perf_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLogFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size: u64,
}

pub struct LoggingRuntime {
    _guard: Option<WorkerGuard>,
}

impl LoggingRuntime {
    pub fn init(cli: &Cli) -> anyhow::Result<Self> {
        let detailed_diagnostics = detailed_diagnostics_requested(cli);
        match Self::try_init(cli, detailed_diagnostics) {
            Ok(runtime) => Ok(runtime),
            Err(_) if !detailed_diagnostics => {
                BRIDGE_DIAGNOSTICS_ENABLED.store(false, Ordering::Relaxed);
                Ok(Self { _guard: None })
            }
            Err(error) => Err(error),
        }
    }

    fn try_init(cli: &Cli, detailed_diagnostics: bool) -> anyhow::Result<Self> {
        let log_path = resolve_log_path(cli)?.context("runtime log path was not resolved")?;
        let directives = build_filter_directives(cli);
        let filter = tracing_subscriber::EnvFilter::try_new(directives.as_str())
            .map_err(|e| anyhow::anyhow!("invalid tracing filter `{directives}`: {e}"))?;
        let writer = RollingFileWriter::new(
            &log_path.path,
            log_path.open_mode,
            LOG_ROTATION_MAX_BYTES,
            LOG_ROTATION_MAX_FILES,
        )?;
        let (non_blocking, guard) = tracing_appender::non_blocking(writer);

        if detailed_diagnostics {
            tracing_subscriber::fmt()
                .json()
                .flatten_event(true)
                .with_env_filter(filter)
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_file(true)
                .with_line_number(true)
                .with_target(true)
                .try_init()
                .map_err(|e| anyhow::anyhow!("failed to initialize tracing subscriber: {e}"))?;
        } else {
            tracing_subscriber::fmt()
                .event_format(BaselineEventFormatter)
                .with_env_filter(filter)
                .with_writer(non_blocking)
                .try_init()
                .map_err(|e| anyhow::anyhow!("failed to initialize tracing subscriber: {e}"))?;
        }

        tracing::info!(
            target: targets::APP_LIFECYCLE,
            event_name = "logging_initialized",
            message = "tracing subscriber initialized",
            log_file = %log_path.path.display(),
            log_path_source = log_path.source.as_str(),
            log_filter = %directives,
            log_open_mode = log_path.open_mode.as_str(),
            log_rotation_max_bytes = LOG_ROTATION_MAX_BYTES,
            log_rotation_max_files = LOG_ROTATION_MAX_FILES,
            log_retention_max_bytes = LOG_RETENTION_MAX_BYTES,
            log_retention_max_age_seconds = LOG_RETENTION_MAX_AGE.as_secs(),
            log_retention_max_files = LOG_RETENTION_MAX_FILES,
            log_retention_min_files = LOG_RETENTION_MIN_FILES,
            detailed_diagnostics,
            version = env!("CARGO_PKG_VERSION"),
        );
        if let Some(retention) = &log_path.retention {
            match enforce_log_retention(retention, Some(&log_path.path)) {
                Ok(report) => {
                    if report.removed_files > 0 {
                        tracing::info!(
                            target: targets::APP_LIFECYCLE,
                            event_name = "log_retention_applied",
                            message = "old diagnostic log files removed",
                            removed_files = report.removed_files,
                            removed_bytes = report.removed_bytes,
                            retention_dir = %retention.directory.display(),
                            retention_prefix = retention.prefix,
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        target: targets::APP_LIFECYCLE,
                        event_name = "log_retention_failed",
                        message = "failed to apply diagnostic log retention",
                        outcome = "failure",
                        error_message = %error,
                        retention_dir = %retention.directory.display(),
                        retention_prefix = retention.prefix,
                    );
                }
            }
        }
        BRIDGE_DIAGNOSTICS_ENABLED.store(detailed_diagnostics, Ordering::Relaxed);

        Ok(Self { _guard: Some(guard) })
    }
}

#[must_use]
pub fn bridge_diagnostics_enabled() -> bool {
    BRIDGE_DIAGNOSTICS_ENABLED.load(Ordering::Relaxed)
}

fn build_filter_directives(cli: &Cli) -> String {
    let detailed_diagnostics = detailed_diagnostics_requested(cli);
    let mut directives = cli
        .log_filter
        .clone()
        .or_else(|| {
            cli.diagnostics_preset
                .as_ref()
                .map(DiagnosticsPreset::filter_directives)
                .map(str::to_owned)
        })
        .or_else(|| std::env::var("RUST_LOG").ok())
        .unwrap_or_else(|| {
            if detailed_diagnostics {
                "info".to_owned()
            } else {
                DEFAULT_BASELINE_FILTER.to_owned()
            }
        });
    if detailed_diagnostics && !directives.contains("tui_markdown=") {
        directives.push_str(",tui_markdown=info");
    }
    directives
}

fn detailed_diagnostics_requested(cli: &Cli) -> bool {
    cli.enable_logs
        || cli.diagnostics_preset.is_some()
        || cli.log_file.is_some()
        || cli.log_filter.is_some()
        || cli.log_append
        || std::env::var_os("RUST_LOG").is_some()
}

#[derive(Debug, Clone, Copy)]
struct BaselineEventFormatter;

impl<S, N> FormatEvent<S, N> for BaselineEventFormatter
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let timestamp = OffsetDateTime::now_utc().format(&Rfc3339).map_err(|_| fmt::Error)?;
        let mut fields = Map::new();
        event.record(&mut BaselineFieldVisitor { fields: &mut fields });

        if !fields.contains_key("event_name") {
            fields.remove("message");
        }

        let mut record = Map::new();
        record.insert("schema".to_owned(), Value::String(BASELINE_LOG_SCHEMA.to_owned()));
        record.insert("timestamp".to_owned(), Value::String(timestamp));
        record.insert("level".to_owned(), Value::String(metadata.level().as_str().to_owned()));
        record.extend(fields);
        record.insert("target".to_owned(), Value::String(metadata.target().to_owned()));
        if let Some(filename) = metadata.file() {
            record.insert("filename".to_owned(), Value::String(filename.to_owned()));
        }
        if let Some(line_number) = metadata.line() {
            record.insert("line_number".to_owned(), Value::from(line_number));
        }

        let line = serde_json::to_string(&record).map_err(|_| fmt::Error)?;
        writer.write_str(&line)?;
        writer.write_char('\n')
    }
}

struct BaselineFieldVisitor<'a> {
    fields: &'a mut Map<String, Value>,
}

impl BaselineFieldVisitor<'_> {
    fn record(&mut self, field: &Field, value: Value) {
        if baseline_field_allowed(field.name()) {
            self.fields.insert(field.name().to_owned(), value);
        }
    }
}

impl Visit for BaselineFieldVisitor<'_> {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record(field, Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record(field, Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, Value::String(value.to_owned()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record(field, Value::String(format!("{value:?}")));
    }
}

fn baseline_field_allowed(name: &str) -> bool {
    matches!(
        name,
        "event_name"
            | "message"
            | "outcome"
            | "reason"
            | "error_kind"
            | "error_code"
            | "duration_ms"
            | "count"
            | "size_bytes"
            | "exit_code"
            | "version"
            | "log_path_source"
            | "log_filter"
            | "log_open_mode"
            | "log_rotation_max_bytes"
            | "log_rotation_max_files"
            | "log_retention_max_bytes"
            | "log_retention_max_age_seconds"
            | "log_retention_max_files"
            | "log_retention_min_files"
            | "detailed_diagnostics"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogPathSource {
    Explicit,
    Default,
    DefaultLegacyAppend,
}

impl LogPathSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Default => "default",
            Self::DefaultLegacyAppend => "default_legacy_append",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogOpenMode {
    CreateNew,
    Truncate,
    Append,
}

impl LogOpenMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::CreateNew => "create_new",
            Self::Truncate => "truncate",
            Self::Append => "append",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedLogPath {
    path: PathBuf,
    source: LogPathSource,
    open_mode: LogOpenMode,
    retention: Option<LogRetentionTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LogRetentionTarget {
    directory: PathBuf,
    prefix: &'static str,
    extension: &'static str,
    policy: LogRetentionPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LogRetentionPolicy {
    max_total_bytes: u64,
    max_age: Duration,
    max_files: usize,
    min_files: usize,
}

impl Default for LogRetentionPolicy {
    fn default() -> Self {
        Self {
            max_total_bytes: LOG_RETENTION_MAX_BYTES,
            max_age: LOG_RETENTION_MAX_AGE,
            max_files: LOG_RETENTION_MAX_FILES,
            min_files: LOG_RETENTION_MIN_FILES,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct LogRetentionReport {
    removed_files: usize,
    removed_bytes: u64,
}

fn resolve_log_path(cli: &Cli) -> anyhow::Result<Option<ResolvedLogPath>> {
    if let Some(path) = cli.log_file.clone() {
        return Ok(Some(ResolvedLogPath {
            path,
            source: LogPathSource::Explicit,
            open_mode: if cli.log_append { LogOpenMode::Append } else { LogOpenMode::Truncate },
            retention: None,
        }));
    }
    if cli.log_append {
        return Ok(Some(ResolvedLogPath {
            path: default_legacy_log_path()?,
            source: LogPathSource::DefaultLegacyAppend,
            open_mode: LogOpenMode::Append,
            retention: None,
        }));
    }
    let directory = default_runtime_log_dir()?;
    let path = generated_log_path(&directory, DEFAULT_RUNTIME_LOG_PREFIX, RUNTIME_LOG_EXTENSION);
    Ok(Some(ResolvedLogPath {
        path,
        source: LogPathSource::Default,
        open_mode: LogOpenMode::CreateNew,
        retention: Some(LogRetentionTarget {
            directory,
            prefix: DEFAULT_RUNTIME_LOG_PREFIX,
            extension: RUNTIME_LOG_EXTENSION,
            policy: LogRetentionPolicy::default(),
        }),
    }))
}

pub fn default_legacy_log_path() -> anyhow::Result<PathBuf> {
    let base_dir = default_diagnostics_dir()?;
    Ok(base_dir.join(DEFAULT_LOG_FILE_NAME))
}

pub fn resolve_perf_path(cli: &Cli) -> anyhow::Result<Option<PathBuf>> {
    if let Some(path) = cli.perf_log.clone() {
        return Ok(Some(path));
    }
    if !perf_enabled_without_explicit_path(cli) {
        return Ok(None);
    }
    Ok(Some(generated_log_path(
        &default_perf_log_dir()?,
        DEFAULT_PERF_LOG_PREFIX,
        PERF_LOG_EXTENSION,
    )))
}

pub fn default_runtime_log_dir() -> anyhow::Result<PathBuf> {
    Ok(default_diagnostics_dir()?.join(DEFAULT_RUNTIME_LOG_SUBDIR))
}

pub fn default_perf_log_dir() -> anyhow::Result<PathBuf> {
    Ok(default_diagnostics_dir()?.join(DEFAULT_PERF_LOG_SUBDIR))
}

pub fn default_diagnostics_paths() -> anyhow::Result<DiagnosticsPaths> {
    let root_dir = default_diagnostics_dir()?;
    Ok(DiagnosticsPaths {
        runtime_dir: root_dir.join(DEFAULT_RUNTIME_LOG_SUBDIR),
        legacy_log_path: root_dir.join(DEFAULT_LOG_FILE_NAME),
        perf_dir: root_dir.join(DEFAULT_PERF_LOG_SUBDIR),
        root_dir,
    })
}

pub fn list_managed_runtime_logs() -> anyhow::Result<Vec<ManagedLogFile>> {
    list_managed_runtime_logs_in(&default_runtime_log_dir()?)
}

pub fn list_managed_runtime_logs_in(directory: &Path) -> anyhow::Result<Vec<ManagedLogFile>> {
    let mut logs = Vec::new();
    let Ok(entries) = read_dir(directory) else {
        return Ok(logs);
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_managed_runtime_log_file(name) {
            continue;
        }
        let metadata = metadata(&path)?;
        if !metadata.is_file() {
            continue;
        }
        logs.push(ManagedLogFile {
            path,
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: metadata.len(),
        });
    }

    logs.sort_by(|left, right| {
        right.modified.cmp(&left.modified).then_with(|| right.path.cmp(&left.path))
    });
    Ok(logs)
}

pub fn latest_default_log_path() -> anyhow::Result<Option<PathBuf>> {
    let paths = default_diagnostics_paths()?;
    latest_log_path_in(&paths.runtime_dir, &paths.legacy_log_path)
}

pub fn latest_log_path_in(
    runtime_dir: &Path,
    legacy_log_path: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    if let Some(log) = list_managed_runtime_logs_in(runtime_dir)?.into_iter().next() {
        return Ok(Some(log.path));
    }
    if legacy_log_path.is_file() {
        return Ok(Some(legacy_log_path.to_path_buf()));
    }
    Ok(None)
}

fn generated_log_path(directory: &Path, prefix: &str, extension: &str) -> PathBuf {
    directory.join(generated_log_file_name(
        prefix,
        extension,
        OffsetDateTime::now_utc(),
        std::process::id(),
        &short_run_id(),
    ))
}

fn generated_log_file_name(
    prefix: &str,
    extension: &str,
    timestamp: OffsetDateTime,
    process_id: u32,
    run_id: &str,
) -> String {
    let timestamp = timestamp
        .format(format_description!("[year][month][day]T[hour][minute][second]Z"))
        .unwrap_or_else(|_| "19700101T000000Z".to_owned());
    format!("{prefix}-{timestamp}-p{process_id}-r{run_id}.{extension}")
}

fn short_run_id() -> String {
    uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect()
}

fn perf_enabled_without_explicit_path(cli: &Cli) -> bool {
    cli.enable_perf || cli.perf_append
}

pub fn default_diagnostics_dir() -> anyhow::Result<PathBuf> {
    if let Some(dir) = dirs::data_local_dir() {
        return Ok(dir.join(DEFAULT_LOG_DIR).join("logs"));
    }
    if let Some(dir) = dirs::cache_dir() {
        return Ok(dir.join(DEFAULT_LOG_DIR).join("logs"));
    }
    if let Some(home) = dirs::home_dir() {
        return Ok(home.join(format!(".{DEFAULT_LOG_DIR}")).join("logs"));
    }
    Ok(std::env::current_dir()
        .context("failed to resolve current directory for default diagnostics path")?
        .join(format!(".{DEFAULT_LOG_DIR}"))
        .join("logs"))
}

#[derive(Debug)]
struct RollingFileWriter {
    base_path: PathBuf,
    max_bytes: u64,
    max_files: usize,
    file: BufWriter<File>,
    current_size: u64,
}

impl RollingFileWriter {
    fn new(
        path: &Path,
        open_mode: LogOpenMode,
        max_bytes: u64,
        max_files: usize,
    ) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)
                .with_context(|| format!("failed to create log directory {}", parent.display()))?;
        }
        if open_mode == LogOpenMode::Append {
            let current_size = metadata(path).map_or(0, |m| m.len());
            if current_size >= max_bytes {
                rotate_file_window(path, max_files)?;
                let file = open_log_file(path, LogOpenMode::Truncate)?;
                return Ok(Self {
                    base_path: path.to_path_buf(),
                    max_bytes,
                    max_files,
                    file: BufWriter::new(file),
                    current_size: 0,
                });
            }
            let file = open_log_file(path, LogOpenMode::Append)?;
            return Ok(Self {
                base_path: path.to_path_buf(),
                max_bytes,
                max_files,
                file: BufWriter::new(file),
                current_size,
            });
        }

        if open_mode == LogOpenMode::Truncate {
            clear_rotated_files(path, max_files)?;
        }
        let file = open_log_file(path, open_mode)?;
        Ok(Self {
            base_path: path.to_path_buf(),
            max_bytes,
            max_files,
            file: BufWriter::new(file),
            current_size: 0,
        })
    }

    fn rotate_if_needed(&mut self, incoming_len: usize) -> std::io::Result<()> {
        let incoming = u64::try_from(incoming_len).unwrap_or(u64::MAX);
        if self.current_size == 0 || self.current_size.saturating_add(incoming) <= self.max_bytes {
            return Ok(());
        }
        self.file.flush()?;
        rotate_file_window(&self.base_path, self.max_files)?;
        self.file = BufWriter::new(open_log_file(&self.base_path, LogOpenMode::Truncate)?);
        self.current_size = 0;
        Ok(())
    }
}

impl Write for RollingFileWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.rotate_if_needed(buf.len())?;
        let written = self.file.write(buf)?;
        self.current_size =
            self.current_size.saturating_add(u64::try_from(written).unwrap_or(u64::MAX));
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn open_log_file(path: &Path, open_mode: LogOpenMode) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true);
    match open_mode {
        LogOpenMode::CreateNew => {
            options.create_new(true);
        }
        LogOpenMode::Truncate => {
            options.create(true).truncate(true);
        }
        LogOpenMode::Append => {
            options.create(true).append(true);
        }
    }
    options.open(path)
}

fn rotate_file_window(base_path: &Path, max_files: usize) -> std::io::Result<()> {
    if max_files == 0 {
        if base_path.exists() {
            remove_file(base_path)?;
        }
        return Ok(());
    }

    let oldest = rotated_log_path(base_path, max_files);
    if oldest.exists() {
        remove_file(&oldest)?;
    }

    for index in (1..max_files).rev() {
        let from = rotated_log_path(base_path, index);
        if from.exists() {
            let to = rotated_log_path(base_path, index + 1);
            if to.exists() {
                remove_file(&to)?;
            }
            rename(&from, &to)?;
        }
    }

    if base_path.exists() {
        let first = rotated_log_path(base_path, 1);
        if first.exists() {
            remove_file(&first)?;
        }
        rename(base_path, first)?;
    }

    Ok(())
}

fn clear_rotated_files(base_path: &Path, max_files: usize) -> std::io::Result<()> {
    for index in 1..=max_files {
        let rotated = rotated_log_path(base_path, index);
        if rotated.exists() {
            remove_file(rotated)?;
        }
    }
    Ok(())
}

fn rotated_log_path(base_path: &Path, index: usize) -> PathBuf {
    let suffix = format!(".{index}");
    if let Some(name) = base_path.file_name().and_then(|name| name.to_str()) {
        base_path.with_file_name(format!("{name}{suffix}"))
    } else {
        let mut path = base_path.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    }
}

#[derive(Debug)]
struct RetentionCandidate {
    path: PathBuf,
    modified: SystemTime,
    size: u64,
}

fn enforce_log_retention(
    target: &LogRetentionTarget,
    protected_path: Option<&Path>,
) -> std::io::Result<LogRetentionReport> {
    enforce_log_retention_at(target, protected_path, SystemTime::now())
}

fn enforce_log_retention_at(
    target: &LogRetentionTarget,
    protected_path: Option<&Path>,
    now: SystemTime,
) -> std::io::Result<LogRetentionReport> {
    let Ok(entries) = read_dir(&target.directory) else {
        return Ok(LogRetentionReport::default());
    };

    let protected_path = protected_path.map(Path::to_path_buf);
    let mut candidates = Vec::new();
    let mut protected_size = 0;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if protected_path.as_ref().is_some_and(|protected| *protected == path) {
            protected_size = metadata(&path).map_or(0, |meta| meta.len());
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_managed_log_file(name, target.prefix, target.extension) {
            continue;
        }
        let metadata = metadata(&path)?;
        if !metadata.is_file() {
            continue;
        }
        candidates.push(RetentionCandidate {
            path,
            modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: metadata.len(),
        });
    }

    candidates.sort_by(|left, right| {
        left.modified.cmp(&right.modified).then_with(|| left.path.cmp(&right.path))
    });

    let mut report = LogRetentionReport::default();
    let mut total_size =
        protected_size + candidates.iter().map(|candidate| candidate.size).sum::<u64>();
    let mut remaining_files = candidates.len() + usize::from(protected_path.is_some());

    let mut retained = Vec::new();
    for candidate in candidates {
        let expired =
            now.duration_since(candidate.modified).is_ok_and(|age| age > target.policy.max_age);
        let over_file_limit = remaining_files > target.policy.max_files;
        if (expired || over_file_limit) && remaining_files > target.policy.min_files {
            remove_retention_candidate(&candidate, &mut report, &mut total_size)?;
            remaining_files = remaining_files.saturating_sub(1);
        } else {
            retained.push(candidate);
        }
    }

    for candidate in retained {
        if total_size <= target.policy.max_total_bytes || remaining_files <= target.policy.min_files
        {
            break;
        }
        remove_retention_candidate(&candidate, &mut report, &mut total_size)?;
        remaining_files = remaining_files.saturating_sub(1);
    }

    Ok(report)
}

fn remove_retention_candidate(
    candidate: &RetentionCandidate,
    report: &mut LogRetentionReport,
    total_size: &mut u64,
) -> std::io::Result<()> {
    match remove_file(&candidate.path) {
        Ok(()) => {
            report.removed_files += 1;
            report.removed_bytes = report.removed_bytes.saturating_add(candidate.size);
            *total_size = total_size.saturating_sub(candidate.size);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_managed_log_file(name: &str, prefix: &str, extension: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('-')) else {
        return false;
    };
    let extension_suffix = format!(".{extension}");
    if rest.ends_with(&extension_suffix) {
        return true;
    }
    rest.rsplit_once('.').is_some_and(|(base, suffix)| {
        base.ends_with(&extension_suffix) && suffix.chars().all(|ch| ch.is_ascii_digit())
    })
}

pub fn is_managed_runtime_log_file(name: &str) -> bool {
    is_managed_log_file(name, DEFAULT_RUNTIME_LOG_PREFIX, RUNTIME_LOG_EXTENSION)
}

pub fn emit_bridge_stderr_line(line: &str) {
    if let Some(record) = BridgeDiagnosticRecord::parse(line) {
        record.emit();
        return;
    }
    let preview = preview_text(line, BRIDGE_LINE_PREVIEW_LIMIT);
    let line_chars = line.chars().count();
    tracing::warn!(
        target: targets::BRIDGE_SDK,
        event_name = "bridge_stderr_unstructured",
        message = "unstructured bridge stderr line received",
        outcome = "unexpected",
        preview = %preview,
        preview_chars = preview.chars().count(),
        line_chars,
    );
}

fn preview_text(input: &str, limit: usize) -> String {
    let mut preview = String::new();
    for (index, ch) in input.chars().enumerate() {
        if index >= limit {
            preview.push_str("...");
            return preview;
        }
        preview.push(ch);
    }
    preview
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BridgeDiagnosticLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Debug, Deserialize)]
struct BridgeDiagnosticRecord {
    schema: String,
    level: BridgeDiagnosticLevel,
    target: String,
    event_name: String,
    message: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    command_id: Option<String>,
    #[serde(default)]
    terminal_id: Option<String>,
    #[serde(default)]
    error_kind: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    count: Option<u64>,
    #[serde(default)]
    size_bytes: Option<u64>,
    #[serde(default)]
    fields: Map<String, Value>,
}

impl BridgeDiagnosticRecord {
    fn parse(line: &str) -> Option<Self> {
        let record: Self = serde_json::from_str(line).ok()?;
        (record.schema == BRIDGE_LOG_SCHEMA).then_some(record)
    }

    fn fields_json(&self) -> String {
        serde_json::to_string(&self.fields).unwrap_or_else(|_| "{}".to_owned())
    }

    fn outcome(&self) -> &str {
        self.outcome.as_deref().unwrap_or("")
    }

    fn timestamp(&self) -> &str {
        self.timestamp.as_deref().unwrap_or("")
    }

    fn session_id(&self) -> &str {
        self.session_id.as_deref().unwrap_or("")
    }

    fn request_id(&self) -> &str {
        self.request_id.as_deref().unwrap_or("")
    }

    fn tool_call_id(&self) -> &str {
        self.tool_call_id.as_deref().unwrap_or("")
    }

    fn command_id(&self) -> &str {
        self.command_id.as_deref().unwrap_or("")
    }

    fn terminal_id(&self) -> &str {
        self.terminal_id.as_deref().unwrap_or("")
    }

    fn error_kind(&self) -> &str {
        self.error_kind.as_deref().unwrap_or("")
    }

    fn error_code(&self) -> &str {
        self.error_code.as_deref().unwrap_or("")
    }

    fn emit(&self) {
        let fields_json = self.fields_json();
        macro_rules! emit_for_target {
            ($target:expr, $log:ident) => {
                tracing::$log!(
                    target: $target,
                    event_name = %self.event_name,
                    message = %self.message,
                    outcome = %self.outcome(),
                    bridge_timestamp = %self.timestamp(),
                    bridge_target = %self.target,
                    session_id = %self.session_id(),
                    request_id = %self.request_id(),
                    tool_call_id = %self.tool_call_id(),
                    command_id = %self.command_id(),
                    terminal_id = %self.terminal_id(),
                    error_kind = %self.error_kind(),
                    error_code = %self.error_code(),
                    duration_ms = self.duration_ms.unwrap_or_default(),
                    count = self.count.unwrap_or_default(),
                    size_bytes = self.size_bytes.unwrap_or_default(),
                    fields_json = %fields_json,
                )
            };
        }

        macro_rules! emit_for_level {
            ($target:expr) => {
                match self.level {
                    BridgeDiagnosticLevel::Error => emit_for_target!($target, error),
                    BridgeDiagnosticLevel::Warn => emit_for_target!($target, warn),
                    BridgeDiagnosticLevel::Info => emit_for_target!($target, info),
                    BridgeDiagnosticLevel::Debug => emit_for_target!($target, debug),
                    BridgeDiagnosticLevel::Trace => emit_for_target!($target, trace),
                }
            };
        }

        match self.target.as_str() {
            targets::APP_LIFECYCLE => emit_for_level!(targets::APP_LIFECYCLE),
            targets::APP_AUTH => emit_for_level!(targets::APP_AUTH),
            targets::APP_CACHE => emit_for_level!(targets::APP_CACHE),
            targets::APP_CONFIG => emit_for_level!(targets::APP_CONFIG),
            targets::APP_COMMAND => emit_for_level!(targets::APP_COMMAND),
            targets::APP_FILE_INDEX => emit_for_level!(targets::APP_FILE_INDEX),
            targets::APP_INPUT => emit_for_level!(targets::APP_INPUT),
            targets::APP_PERMISSION => emit_for_level!(targets::APP_PERMISSION),
            targets::APP_PASTE => emit_for_level!(targets::APP_PASTE),
            targets::APP_PERF => emit_for_level!(targets::APP_PERF),
            targets::APP_RENDER => emit_for_level!(targets::APP_RENDER),
            targets::APP_SESSION => emit_for_level!(targets::APP_SESSION),
            targets::APP_TOOL => emit_for_level!(targets::APP_TOOL),
            targets::APP_NETWORK => emit_for_level!(targets::APP_NETWORK),
            targets::APP_UPDATE => emit_for_level!(targets::APP_UPDATE),
            targets::BRIDGE_LIFECYCLE => emit_for_level!(targets::BRIDGE_LIFECYCLE),
            targets::BRIDGE_MCP => emit_for_level!(targets::BRIDGE_MCP),
            targets::BRIDGE_PERMISSION => emit_for_level!(targets::BRIDGE_PERMISSION),
            targets::BRIDGE_PROTOCOL => emit_for_level!(targets::BRIDGE_PROTOCOL),
            targets::BRIDGE_SDK => emit_for_level!(targets::BRIDGE_SDK),
            _ => emit_for_level!(targets::BRIDGE_SDK),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BaselineEventFormatter, BridgeDiagnosticRecord, LogOpenMode, LogPathSource,
        LogRetentionPolicy, LogRetentionTarget, RollingFileWriter, baseline_field_allowed,
        clear_rotated_files, enforce_log_retention_at, generated_log_file_name,
        is_managed_log_file, list_managed_runtime_logs_in, preview_text, resolve_log_path,
        resolve_perf_path, rotated_log_path,
    };
    use crate::{Cli, DiagnosticsPreset};
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};
    use tempfile::tempdir;
    use time::macros::datetime;

    #[derive(Clone)]
    struct SharedLogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    struct LogWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for SharedLogWriter {
        type Writer = LogWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            LogWriter(std::sync::Arc::clone(&self.0))
        }
    }

    impl std::io::Write for LogWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn parses_structured_bridge_diagnostic() {
        let line = r#"{"schema":"claude-rs-log/v1","timestamp":"2026-04-08T12:00:00Z","level":"warn","target":"bridge.sdk","event_name":"sdk_spawn_failed","message":"spawn failed","session_id":"session-1","fields":{"preview":"node"}}"#;
        let record = BridgeDiagnosticRecord::parse(line).expect("structured bridge log");

        assert_eq!(record.target, "bridge.sdk");
        assert_eq!(record.event_name, "sdk_spawn_failed");
        assert_eq!(record.message, "spawn failed");
        assert_eq!(record.session_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn preview_truncates_with_ellipsis() {
        let preview = preview_text("abcdefgh", 5);
        assert_eq!(preview, "abcde...");
    }

    #[test]
    fn baseline_fields_exclude_sensitive_diagnostic_payloads() {
        for allowed in [
            "event_name",
            "message",
            "outcome",
            "error_kind",
            "error_code",
            "duration_ms",
            "version",
        ] {
            assert!(baseline_field_allowed(allowed), "{allowed} should be retained");
        }
        for excluded in [
            "session_id",
            "request_id",
            "tool_call_id",
            "prompt",
            "content",
            "command",
            "path",
            "preview",
            "error",
            "error_message",
        ] {
            assert!(!baseline_field_allowed(excluded), "{excluded} should be excluded");
        }
    }

    #[test]
    fn baseline_formatter_writes_structured_metadata_without_sensitive_fields() {
        let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .event_format(BaselineEventFormatter)
            .with_writer(SharedLogWriter(std::sync::Arc::clone(&buffer)))
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                target: "app.session",
                event_name = "session_operation_failed",
                message = "session operation failed",
                outcome = "failure",
                error_code = "timeout",
                duration_ms = 250_u64,
                session_id = "session-secret",
                path = "C:/private/project",
                error_message = "token=secret",
            );
        });

        let output =
            String::from_utf8(buffer.lock().expect("log buffer lock").clone()).expect("utf8 log");
        let record: serde_json::Value = serde_json::from_str(output.trim()).expect("json log");
        assert_eq!(record["event_name"], "session_operation_failed");
        assert_eq!(record["schema"], "claude-rs-baseline/v1");
        assert_eq!(record["message"], "session operation failed");
        assert_eq!(record["outcome"], "failure");
        assert_eq!(record["error_code"], "timeout");
        assert_eq!(record["duration_ms"], 250);
        assert_eq!(record["target"], "app.session");
        assert!(record.get("timestamp").is_some());
        assert!(record.get("session_id").is_none());
        assert!(record.get("path").is_none());
        assert!(record.get("error_message").is_none());
        assert!(!output.contains("session-secret"));
        assert!(!output.contains("private/project"));
        assert!(!output.contains("token=secret"));
    }

    #[test]
    fn resolve_log_path_uses_default_for_always_on_baseline() {
        let cli = Cli {
            command: None,
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        };

        let resolved = resolve_log_path(&cli).expect("resolve succeeds").expect("path exists");
        assert_eq!(resolved.source, LogPathSource::Default);
        assert_eq!(resolved.open_mode, LogOpenMode::CreateNew);
        assert!(resolved.retention.is_some());
    }

    #[test]
    fn resolve_log_path_uses_explicit_path_when_provided() {
        let cli = Cli {
            command: None,
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: Some(PathBuf::from("custom.log")),
            log_filter: None,
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        };

        let resolved = resolve_log_path(&cli).expect("resolve succeeds").expect("path exists");
        assert_eq!(resolved.path, PathBuf::from("custom.log"));
        assert_eq!(resolved.source.as_str(), "explicit");
        assert_eq!(resolved.open_mode, LogOpenMode::Truncate);
        assert!(resolved.retention.is_none());
    }

    #[test]
    fn resolve_log_path_uses_default_when_filter_enables_logging() {
        let cli = Cli {
            command: None,
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: Some("app.render=trace".to_owned()),
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        };

        let resolved = resolve_log_path(&cli).expect("resolve succeeds").expect("path exists");
        assert_eq!(resolved.source.as_str(), "default");
        assert_eq!(resolved.open_mode, LogOpenMode::CreateNew);
        assert!(resolved.retention.is_some());
        let path = resolved.path.to_string_lossy().replace('\\', "/");
        assert!(path.contains("claude-code-rust/logs/runtime/claude-rs-"));
        assert!(
            resolved
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("log"))
        );
        assert!(!path.ends_with("claude-rs.log"));
    }

    #[test]
    fn resolve_log_path_uses_default_when_enable_logs_is_set() {
        let cli = Cli {
            command: None,
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: true,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        };

        let resolved = resolve_log_path(&cli).expect("resolve succeeds").expect("path exists");
        assert_eq!(resolved.source.as_str(), "default");
        assert_eq!(resolved.open_mode, LogOpenMode::CreateNew);
    }

    #[test]
    fn resolve_log_path_uses_default_when_preset_is_set() {
        let cli = Cli {
            command: None,
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: Some(DiagnosticsPreset::Session),
            log_file: None,
            log_filter: None,
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        };

        let resolved = resolve_log_path(&cli).expect("resolve succeeds").expect("path exists");
        assert_eq!(resolved.source.as_str(), "default");
        assert_eq!(resolved.open_mode, LogOpenMode::CreateNew);
    }

    #[test]
    fn resolve_log_path_keeps_legacy_default_for_append_without_explicit_path() {
        let cli = Cli {
            command: None,
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: true,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        };

        let resolved = resolve_log_path(&cli).expect("resolve succeeds").expect("path exists");
        assert_eq!(resolved.source, LogPathSource::DefaultLegacyAppend);
        assert_eq!(resolved.open_mode, LogOpenMode::Append);
        assert!(resolved.retention.is_none());
        let path = resolved.path.to_string_lossy().replace('\\', "/");
        assert!(path.ends_with("claude-code-rust/logs/claude-rs.log"));
    }

    #[test]
    fn resolve_perf_path_uses_default_when_enable_perf_is_set() {
        let cli = Cli {
            command: None,
            no_update_check: false,
            dir: None,
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: false,
            enable_perf: true,
            perf_log: None,
            perf_append: false,
        };

        let resolved = resolve_perf_path(&cli).expect("resolve succeeds").expect("path exists");
        let path = resolved.to_string_lossy().replace('\\', "/");
        assert!(path.contains("claude-code-rust/logs/perf/claude-rs-perf-"));
        assert!(
            resolved
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        );
    }

    #[test]
    fn generated_default_log_name_is_timestamped_and_filesystem_safe() {
        let name = generated_log_file_name(
            "claude-rs",
            "log",
            datetime!(2026-06-14 7:59:24 UTC),
            12345,
            "8f3a2c1",
        );

        assert_eq!(name, "claude-rs-20260614T075924Z-p12345-r8f3a2c1.log");
        assert!(!name.contains(':'));
    }

    #[test]
    fn rolling_writer_rotates_by_size() {
        let dir = tempdir().expect("temp dir");
        let base = dir.path().join("runtime.log");
        let mut writer =
            RollingFileWriter::new(&base, LogOpenMode::Truncate, 10, 2).expect("writer");

        writer.write_all(b"12345").expect("first write");
        writer.write_all(b"67890").expect("second write");
        writer.write_all(b"abc").expect("rotation write");
        writer.flush().expect("flush");

        let current = fs::read_to_string(&base).expect("current log");
        let rotated = fs::read_to_string(rotated_log_path(&base, 1)).expect("rotated log");

        assert_eq!(current, "abc");
        assert_eq!(rotated, "1234567890");
    }

    #[test]
    fn rolling_writer_append_rotates_full_startup_file_without_clearing_window() {
        let dir = tempdir().expect("temp dir");
        let base = dir.path().join("runtime.log");
        fs::write(&base, "1234567890").expect("write base");
        fs::write(rotated_log_path(&base, 1), "previous").expect("write rotated");

        let mut writer = RollingFileWriter::new(&base, LogOpenMode::Append, 10, 2).expect("writer");
        writer.write_all(b"abc").expect("write new base");
        writer.flush().expect("flush");

        assert_eq!(fs::read_to_string(&base).expect("current log"), "abc");
        assert_eq!(
            fs::read_to_string(rotated_log_path(&base, 1)).expect("first rotated"),
            "1234567890"
        );
        assert_eq!(
            fs::read_to_string(rotated_log_path(&base, 2)).expect("second rotated"),
            "previous"
        );
    }

    #[test]
    fn clear_rotated_files_removes_existing_window() {
        let dir = tempdir().expect("temp dir");
        let base = dir.path().join("runtime.log");
        fs::write(rotated_log_path(&base, 1), "a").expect("write first");
        fs::write(rotated_log_path(&base, 2), "b").expect("write second");

        clear_rotated_files(&base, 2).expect("clear rotated files");

        assert!(!rotated_log_path(&base, 1).exists());
        assert!(!rotated_log_path(&base, 2).exists());
    }

    #[test]
    fn managed_log_file_detection_is_strict() {
        assert!(is_managed_log_file("claude-rs-20260614T075924Z-p1-rabc.log", "claude-rs", "log"));
        assert!(is_managed_log_file(
            "claude-rs-20260614T075924Z-p1-rabc.log.2",
            "claude-rs",
            "log"
        ));
        assert!(!is_managed_log_file("claude-rs.log", "claude-rs", "log"));
        assert!(!is_managed_log_file("other-20260614T075924Z-p1-rabc.log", "claude-rs", "log"));
        assert!(!is_managed_log_file("claude-rs-20260614T075924Z-p1-rabc.txt", "claude-rs", "log"));
    }

    #[test]
    fn retention_removes_only_managed_logs_and_preserves_protected_file() {
        let dir = tempdir().expect("temp dir");
        let protected = dir.path().join("claude-rs-20260614T080000Z-p1-ractive.log");
        let removable = dir.path().join("claude-rs-20260614T070000Z-p1-rold.log");
        let legacy = dir.path().join("claude-rs.log");
        fs::write(&protected, "active").expect("write protected");
        fs::write(&removable, "old").expect("write removable");
        fs::write(&legacy, "legacy").expect("write legacy");
        let target = LogRetentionTarget {
            directory: dir.path().to_path_buf(),
            prefix: "claude-rs",
            extension: "log",
            policy: LogRetentionPolicy {
                max_total_bytes: 1,
                max_age: Duration::from_secs(60),
                max_files: 10,
                min_files: 1,
            },
        };

        let report = enforce_log_retention_at(&target, Some(&protected), SystemTime::now())
            .expect("retention succeeds");

        assert_eq!(report.removed_files, 1);
        assert!(protected.exists());
        assert!(!removable.exists());
        assert!(legacy.exists());
    }

    #[test]
    fn retention_caps_managed_log_file_count() {
        let dir = tempdir().expect("temp dir");
        for index in 0..5 {
            let path =
                dir.path().join(format!("claude-rs-20260614T07000{index}Z-p1-rrun{index}.log"));
            fs::write(path, "log").expect("write managed log");
        }
        let target = LogRetentionTarget {
            directory: dir.path().to_path_buf(),
            prefix: "claude-rs",
            extension: "log",
            policy: LogRetentionPolicy {
                max_total_bytes: u64::MAX,
                max_age: Duration::MAX,
                max_files: 3,
                min_files: 1,
            },
        };

        let report =
            enforce_log_retention_at(&target, None, SystemTime::now()).expect("retention succeeds");

        assert_eq!(report.removed_files, 2);
        assert_eq!(list_managed_runtime_logs_in(dir.path()).expect("list logs").len(), 3);
    }
}
