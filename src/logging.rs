// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::Cli;
use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::fs::OpenOptions;

pub mod targets {
    pub const APP_LIFECYCLE: &str = "app.lifecycle";
    pub const APP_SESSION: &str = "app.session";
    pub const BRIDGE_LIFECYCLE: &str = "bridge.lifecycle";
    pub const BRIDGE_MCP: &str = "bridge.mcp";
    pub const BRIDGE_PERMISSION: &str = "bridge.permission";
    pub const BRIDGE_PROTOCOL: &str = "bridge.protocol";
    pub const BRIDGE_SDK: &str = "bridge.sdk";
}

const BRIDGE_LOG_SCHEMA: &str = "claude-rs-log/v1";
const BRIDGE_LINE_PREVIEW_LIMIT: usize = 240;

pub struct LoggingRuntime {
    _private: (),
}

impl LoggingRuntime {
    pub fn init(cli: &Cli) -> anyhow::Result<Self> {
        let Some(path) = cli.log_file.as_ref() else {
            if std::env::var_os("RUST_LOG").is_some() {
                eprintln!(
                    "RUST_LOG is set, but tracing is disabled without --log-file <PATH>. \
Use --log-file to enable diagnostics."
                );
            }
            return Ok(Self { _private: () });
        };

        let directives = build_filter_directives(cli);
        let filter = tracing_subscriber::EnvFilter::try_new(directives.as_str())
            .map_err(|e| anyhow::anyhow!("invalid tracing filter `{directives}`: {e}"))?;

        let mut options = OpenOptions::new();
        options.create(true).write(true);
        if cli.log_append {
            options.append(true);
        } else {
            options.truncate(true);
        }
        let file = options
            .open(path)
            .with_context(|| format!("failed to open log file {}", path.display()))?;

        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(file)
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
            log_file = %path.display(),
            log_filter = %directives,
            log_append = cli.log_append,
            version = env!("CARGO_PKG_VERSION"),
        );

        Ok(Self { _private: () })
    }
}

fn build_filter_directives(cli: &Cli) -> String {
    let mut directives = cli
        .log_filter
        .clone()
        .or_else(|| std::env::var("RUST_LOG").ok())
        .unwrap_or_else(|| "info".to_owned());
    if !directives.contains("tui_markdown=") {
        directives.push_str(",tui_markdown=info");
    }
    directives
}

pub fn emit_bridge_stderr_line(line: &str) {
    if let Some(record) = BridgeDiagnosticRecord::parse(line) {
        record.emit();
        return;
    }
    emit_legacy_bridge_stderr_line(line);
}

fn emit_legacy_bridge_stderr_line(line: &str) {
    let lowered = line.to_ascii_lowercase();
    let level = if lowered.contains("[sdk error]")
        || lowered.starts_with("error")
        || lowered.contains("panic")
    {
        BridgeDiagnosticLevel::Error
    } else if lowered.contains("[sdk warn]") || lowered.starts_with("warn") {
        BridgeDiagnosticLevel::Warn
    } else {
        BridgeDiagnosticLevel::Debug
    };

    let preview = preview_text(line, BRIDGE_LINE_PREVIEW_LIMIT);
    let line_chars = line.chars().count();
    match level {
        BridgeDiagnosticLevel::Error => tracing::error!(
            target: targets::BRIDGE_SDK,
            event_name = "legacy_bridge_stderr_line",
            message = "legacy bridge stderr line received",
            outcome = "legacy",
            preview = %preview,
            preview_chars = preview.chars().count(),
            line_chars,
        ),
        BridgeDiagnosticLevel::Warn => tracing::warn!(
            target: targets::BRIDGE_SDK,
            event_name = "legacy_bridge_stderr_line",
            message = "legacy bridge stderr line received",
            outcome = "legacy",
            preview = %preview,
            preview_chars = preview.chars().count(),
            line_chars,
        ),
        BridgeDiagnosticLevel::Info => tracing::info!(
            target: targets::BRIDGE_SDK,
            event_name = "legacy_bridge_stderr_line",
            message = "legacy bridge stderr line received",
            outcome = "legacy",
            preview = %preview,
            preview_chars = preview.chars().count(),
            line_chars,
        ),
        BridgeDiagnosticLevel::Debug => tracing::debug!(
            target: targets::BRIDGE_SDK,
            event_name = "legacy_bridge_stderr_line",
            message = "legacy bridge stderr line received",
            outcome = "legacy",
            preview = %preview,
            preview_chars = preview.chars().count(),
            line_chars,
        ),
        BridgeDiagnosticLevel::Trace => tracing::trace!(
            target: targets::BRIDGE_SDK,
            event_name = "legacy_bridge_stderr_line",
            message = "legacy bridge stderr line received",
            outcome = "legacy",
            preview = %preview,
            preview_chars = preview.chars().count(),
            line_chars,
        ),
    }
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
            targets::APP_SESSION => emit_for_level!(targets::APP_SESSION),
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
    use super::{BridgeDiagnosticRecord, preview_text};

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
}
