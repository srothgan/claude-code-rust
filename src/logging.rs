// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::{Cli, DiagnosticsPreset};
use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::fs::{File, OpenOptions, create_dir_all, metadata, remove_file, rename};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing_appender::non_blocking::WorkerGuard;

pub mod targets {
    pub const APP_AUTH: &str = "app.auth";
    pub const APP_CACHE: &str = "app.cache";
    pub const APP_CONFIG: &str = "app.config";
    pub const APP_COMMAND: &str = "app.command";
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
const BRIDGE_LINE_PREVIEW_LIMIT: usize = 240;
const DEFAULT_LOG_DIR: &str = "claude-code-rust";
const DEFAULT_LOG_FILE_NAME: &str = "claude-rs.log";
const DEFAULT_PERF_FILE_NAME: &str = "claude-rs-perf.log";
const LOG_ROTATION_MAX_BYTES: u64 = 10 * 1024 * 1024;
const LOG_ROTATION_MAX_FILES: usize = 5;
static BRIDGE_DIAGNOSTICS_ENABLED: AtomicBool = AtomicBool::new(false);

pub struct LoggingRuntime {
    _guard: Option<WorkerGuard>,
}

impl LoggingRuntime {
    pub fn init(cli: &Cli) -> anyhow::Result<Self> {
        let Some(log_path) = resolve_log_path(cli)? else {
            BRIDGE_DIAGNOSTICS_ENABLED.store(false, Ordering::Relaxed);
            return Ok(Self { _guard: None });
        };

        let directives = build_filter_directives(cli);
        let filter = tracing_subscriber::EnvFilter::try_new(directives.as_str())
            .map_err(|e| anyhow::anyhow!("invalid tracing filter `{directives}`: {e}"))?;
        let writer = RollingFileWriter::new(
            &log_path.path,
            cli.log_append,
            LOG_ROTATION_MAX_BYTES,
            LOG_ROTATION_MAX_FILES,
        )?;
        let (non_blocking, guard) = tracing_appender::non_blocking(writer);

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

        tracing::info!(
            target: targets::APP_LIFECYCLE,
            event_name = "logging_initialized",
            message = "tracing subscriber initialized",
            log_file = %log_path.path.display(),
            log_path_source = log_path.source.as_str(),
            log_filter = %directives,
            log_append = cli.log_append,
            log_rotation_max_bytes = LOG_ROTATION_MAX_BYTES,
            log_rotation_max_files = LOG_ROTATION_MAX_FILES,
            version = env!("CARGO_PKG_VERSION"),
        );
        BRIDGE_DIAGNOSTICS_ENABLED.store(true, Ordering::Relaxed);

        Ok(Self { _guard: Some(guard) })
    }
}

#[must_use]
pub fn bridge_diagnostics_enabled() -> bool {
    BRIDGE_DIAGNOSTICS_ENABLED.load(Ordering::Relaxed)
}

fn build_filter_directives(cli: &Cli) -> String {
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
        .unwrap_or_else(|| "info".to_owned());
    if !directives.contains("tui_markdown=") {
        directives.push_str(",tui_markdown=info");
    }
    directives
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogPathSource {
    Explicit,
    Default,
}

impl LogPathSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedLogPath {
    path: PathBuf,
    source: LogPathSource,
}

fn resolve_log_path(cli: &Cli) -> anyhow::Result<Option<ResolvedLogPath>> {
    if let Some(path) = cli.log_file.clone() {
        return Ok(Some(ResolvedLogPath { path, source: LogPathSource::Explicit }));
    }
    if !logging_enabled_without_explicit_path(cli) {
        return Ok(None);
    }
    let path = default_log_path()?;
    Ok(Some(ResolvedLogPath { path, source: LogPathSource::Default }))
}

fn logging_enabled_without_explicit_path(cli: &Cli) -> bool {
    cli.enable_logs
        || cli.diagnostics_preset.is_some()
        || cli.log_filter.is_some()
        || cli.log_append
        || std::env::var_os("RUST_LOG").is_some()
}

fn default_log_path() -> anyhow::Result<PathBuf> {
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
    Ok(Some(default_diagnostics_dir()?.join(DEFAULT_PERF_FILE_NAME)))
}

fn perf_enabled_without_explicit_path(cli: &Cli) -> bool {
    cli.enable_perf || cli.perf_append
}

fn default_diagnostics_dir() -> anyhow::Result<PathBuf> {
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
    fn new(path: &Path, append: bool, max_bytes: u64, max_files: usize) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)
                .with_context(|| format!("failed to create log directory {}", parent.display()))?;
        }
        if append {
            let current_size = metadata(path).map_or(0, |m| m.len());
            if current_size >= max_bytes {
                rotate_file_window(path, max_files)?;
                return Self::new(path, false, max_bytes, max_files);
            }
            let file = open_log_file(path, true)?;
            return Ok(Self {
                base_path: path.to_path_buf(),
                max_bytes,
                max_files,
                file: BufWriter::new(file),
                current_size,
            });
        }

        clear_rotated_files(path, max_files)?;
        let file = open_log_file(path, false)?;
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
        self.file = BufWriter::new(open_log_file(&self.base_path, false)?);
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

fn open_log_file(path: &Path, append: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
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
        BridgeDiagnosticRecord, RollingFileWriter, clear_rotated_files, preview_text,
        resolve_log_path, resolve_perf_path, rotated_log_path,
    };
    use crate::{Cli, DiagnosticsPreset};
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::tempdir;

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
        let path = resolved.path.to_string_lossy().replace('\\', "/");
        assert!(path.ends_with("claude-code-rust/logs/claude-rs.log"));
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
        assert!(path.ends_with("claude-code-rust/logs/claude-rs-perf.log"));
    }

    #[test]
    fn rolling_writer_rotates_by_size() {
        let dir = tempdir().expect("temp dir");
        let base = dir.path().join("runtime.log");
        let mut writer = RollingFileWriter::new(&base, false, 10, 2).expect("writer");

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
    fn clear_rotated_files_removes_existing_window() {
        let dir = tempdir().expect("temp dir");
        let base = dir.path().join("runtime.log");
        fs::write(rotated_log_path(&base, 1), "a").expect("write first");
        fs::write(rotated_log_path(&base, 2), "b").expect("write second");

        clear_rotated_files(&base, 2).expect("clear rotated files");

        assert!(!rotated_log_path(&base, 1).exists());
        assert!(!rotated_log_path(&base, 2).exists());
    }
}
