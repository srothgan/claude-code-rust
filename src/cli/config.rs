// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::redaction;
use crate::app::config::store::{
    InspectedConfigDocuments, InspectedConfigFile, InspectedConfigFileKind,
    InspectedConfigFileStatus, inspect_read_only,
};
use crate::{
    Cli, ConfigArgs, ConfigCommand, ConfigExportArgs, ConfigFileSelector, ConfigPathArgs,
    ConfigShowArgs,
};
use anyhow::Context as _;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, OpenOptions, create_dir_all};
use std::io::{IsTerminal as _, Write};
use std::path::{Path, PathBuf};
use time::{OffsetDateTime, macros::format_description};

const EXPORT_SCHEMA: &str = "claude-rs-config-export/v1";
const HUMAN_DOCUMENT_PREVIEW_LINES: usize = 24;

pub fn run(
    cli: &Cli,
    args: &ConfigArgs,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> anyhow::Result<i32> {
    match &args.command {
        Some(ConfigCommand::Path(args)) => write_path(cli, args, stdout),
        Some(ConfigCommand::Show(args)) => write_show(cli, args, stdout),
        Some(ConfigCommand::Export(args)) => write_export(cli, args, stdout, stderr),
        None => write_path(cli, &ConfigPathArgs { json: false, which: None }, stdout),
    }
}

fn write_path(cli: &Cli, args: &ConfigPathArgs, stdout: &mut impl Write) -> anyhow::Result<i32> {
    write_path_with_home_override(cli, args, None, stdout)
}

fn write_path_with_home_override(
    cli: &Cli,
    args: &ConfigPathArgs,
    home_override: Option<&Path>,
    stdout: &mut impl Write,
) -> anyhow::Result<i32> {
    let project_root = project_root(cli)?;
    let inspection = inspect_config(cli, &project_root, home_override)?;

    if let Some(which) = args.which {
        let Some(file) = selected_file(&inspection, which) else {
            anyhow::bail!("selected config file was not found in inspection report");
        };
        writeln!(stdout, "{}", file.path.display())?;
        return Ok(0);
    }

    if args.json {
        write_json(stdout, &PathReport::from_inspection(&inspection, &project_root))?;
    } else {
        write_path_human(stdout, &inspection)?;
    }

    Ok(0)
}

fn write_show(cli: &Cli, args: &ConfigShowArgs, stdout: &mut impl Write) -> anyhow::Result<i32> {
    write_show_with_home_override(cli, args, None, stdout)
}

fn write_show_with_home_override(
    cli: &Cli,
    args: &ConfigShowArgs,
    home_override: Option<&Path>,
    stdout: &mut impl Write,
) -> anyhow::Result<i32> {
    let project_root = project_root(cli)?;
    let inspection = inspect_config(cli, &project_root, home_override)?;
    let report = ShowReport::from_inspection(&inspection, &project_root);

    if args.json {
        write_json(stdout, &report)?;
    } else {
        write_show_human(stdout, &report)?;
    }

    Ok(i32::from(report.has_failures()))
}

fn write_export(
    cli: &Cli,
    args: &ConfigExportArgs,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> anyhow::Result<i32> {
    write_export_with_home_override(cli, args, None, stdout, stderr)
}

fn write_export_with_home_override(
    cli: &Cli,
    args: &ConfigExportArgs,
    home_override: Option<&Path>,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> anyhow::Result<i32> {
    let project_root = project_root(cli)?;
    let inspection = inspect_config(cli, &project_root, home_override)?;
    let report = ExportReport::from_inspection(&inspection, &project_root);
    if report.has_failures() {
        writeln!(
            stderr,
            "Cannot export config because one or more existing config files are invalid or unreadable."
        )?;
        return Ok(1);
    }

    let text = json_text(&report)?;
    if let Some(output) = &args.output {
        if output.exists() {
            writeln!(stderr, "Refusing to overwrite existing file: {}", output.display())?;
            return Ok(1);
        }
        write_new_file_atomically(output, &text)?;
        writeln!(stdout, "Exported redacted config: {}", output.display())?;
    } else {
        writeln!(stdout, "{text}")?;
    }

    Ok(0)
}

fn inspect_config(
    cli: &Cli,
    project_root: &Path,
    home_override: Option<&Path>,
) -> anyhow::Result<InspectedConfigDocuments> {
    inspect_read_only(home_override, Some(cli.dir.as_deref().unwrap_or(project_root)))
        .map_err(anyhow::Error::msg)
}

fn project_root(cli: &Cli) -> anyhow::Result<PathBuf> {
    if let Some(path) = &cli.dir {
        Ok(path.clone())
    } else {
        std::env::current_dir().context("failed to resolve current directory")
    }
}

fn write_path_human(
    stdout: &mut impl Write,
    inspection: &InspectedConfigDocuments,
) -> anyhow::Result<()> {
    let style = HumanStyle::detect();
    writeln!(stdout, "{}", style.title("claude-rs config"))?;
    writeln!(stdout, "{} {}", style.detail_label("Summary:"), summary_counts(inspection))?;
    writeln!(stdout)?;
    writeln!(stdout, "{}", style.heading("Locations"))?;
    for file in &inspection.files {
        write_location(stdout, style, file)?;
    }
    writeln!(stdout)?;
    writeln!(stdout, "{}", style.heading("Commands"))?;
    writeln!(
        stdout,
        "{}",
        style.command_block(
            "show",
            "claude-rs config show",
            "Show a concise redacted config summary"
        )
    )?;
    writeln!(
        stdout,
        "{}",
        style.command_block(
            "export",
            "claude-rs config export --output config.json",
            "Write a redacted support export"
        )
    )?;
    Ok(())
}

fn write_show_human(stdout: &mut impl Write, report: &ShowReport) -> anyhow::Result<()> {
    let style = HumanStyle::detect();
    writeln!(stdout, "{}", style.title("claude-rs config"))?;
    writeln!(stdout, "{} {}", style.detail_label("Summary:"), report.summary.as_human())?;
    writeln!(stdout)?;
    writeln!(stdout, "{}", style.heading("Documents"))?;

    for (index, file) in report.files.iter().enumerate() {
        write_show_file(stdout, style, file)?;
        if index + 1 < report.files.len() {
            writeln!(stdout)?;
        }
    }

    Ok(())
}

fn write_show_file(
    stdout: &mut impl Write,
    style: HumanStyle,
    file: &ConfigFileReport,
) -> anyhow::Result<()> {
    writeln!(stdout, "  {} {}", style.status(file.status), file.label)?;
    writeln!(stdout, "      - {} {}", style.detail_label("Scope:"), file.scope)?;
    writeln!(stdout, "      - {} {}", style.detail_label("Path:"), file.path)?;
    writeln!(stdout, "      - {} {}", style.detail_label("State:"), file.status.as_human())?;
    if let Some(error) = &file.error {
        writeln!(stdout, "      - {} {}", style.detail_label("Error:"), error)?;
    }

    if let Some(document) = &file.document {
        write_document_summary(stdout, style, document)?;
    }

    Ok(())
}

fn write_document_summary(
    stdout: &mut impl Write,
    style: HumanStyle,
    document: &Value,
) -> anyhow::Result<()> {
    let text = json_text(document)?;
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() <= HUMAN_DOCUMENT_PREVIEW_LINES {
        writeln!(stdout, "      - {}", style.detail_label("Redacted document:"))?;
        for line in lines {
            writeln!(stdout, "        {line}")?;
        }
        return Ok(());
    }

    let Some(object) = document.as_object() else {
        writeln!(
            stdout,
            "      - {} {} lines, hidden from human output",
            style.detail_label("Redacted document:"),
            lines.len()
        )?;
        return Ok(());
    };

    writeln!(stdout, "      - {} {}", style.detail_label("Top-level keys:"), object.len())?;
    if object.is_empty() {
        return Ok(());
    }

    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    let visible = keys.iter().take(8).map(String::as_str).collect::<Vec<_>>();
    writeln!(stdout, "      - {} {}", style.detail_label("Keys:"), visible.join(", "))?;
    if keys.len() > visible.len() {
        writeln!(
            stdout,
            "      - {} {} more",
            style.detail_label("Hidden keys:"),
            keys.len() - visible.len()
        )?;
    }
    writeln!(
        stdout,
        "      - {} {} lines, hidden from human output",
        style.detail_label("Redacted document:"),
        lines.len()
    )?;
    writeln!(
        stdout,
        "      - {} claude-rs config show --json",
        style.detail_label("Full redacted JSON:")
    )?;
    Ok(())
}

fn write_location(
    stdout: &mut impl Write,
    style: HumanStyle,
    file: &InspectedConfigFile,
) -> std::io::Result<()> {
    writeln!(stdout, "  {} {}", style.status(file.status), file.label)?;
    writeln!(stdout, "      - {} {}", style.detail_label("Scope:"), file.scope)?;
    writeln!(stdout, "      - {} {}", style.detail_label("Path:"), file.path.display())?;
    writeln!(stdout, "      - {} {}", style.detail_label("State:"), file.status.as_human())
}

fn selected_file(
    inspection: &InspectedConfigDocuments,
    selector: ConfigFileSelector,
) -> Option<&InspectedConfigFile> {
    let kind = match selector {
        ConfigFileSelector::Settings => InspectedConfigFileKind::Settings,
        ConfigFileSelector::LocalSettings => InspectedConfigFileKind::LocalSettings,
        ConfigFileSelector::Preferences => InspectedConfigFileKind::Preferences,
    };
    inspection.files.iter().find(|file| file.kind == kind)
}

#[derive(Debug, Serialize)]
struct PathReport {
    version: &'static str,
    project_root: String,
    files: Vec<ConfigFileMetadata>,
}

impl PathReport {
    fn from_inspection(inspection: &InspectedConfigDocuments, project_root: &Path) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            project_root: project_root.display().to_string(),
            files: inspection.files.iter().map(ConfigFileMetadata::from).collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ShowReport {
    version: &'static str,
    project_root: String,
    summary: ConfigSummary,
    files: Vec<ConfigFileReport>,
}

impl ShowReport {
    fn from_inspection(inspection: &InspectedConfigDocuments, project_root: &Path) -> Self {
        let files = inspection.files.iter().map(ConfigFileReport::from).collect::<Vec<_>>();
        Self {
            version: env!("CARGO_PKG_VERSION"),
            project_root: project_root.display().to_string(),
            summary: ConfigSummary::from_files(&files),
            files,
        }
    }

    fn has_failures(&self) -> bool {
        self.files.iter().any(ConfigFileReport::is_failure)
    }
}

#[derive(Debug, Serialize)]
struct ExportReport {
    schema: &'static str,
    created_at: String,
    version: &'static str,
    platform: PlatformReport,
    project_root: String,
    summary: ConfigSummary,
    files: Vec<ConfigFileReport>,
    skipped: Vec<&'static str>,
    redaction: &'static str,
}

impl ExportReport {
    fn from_inspection(inspection: &InspectedConfigDocuments, project_root: &Path) -> Self {
        let files = inspection.files.iter().map(ConfigFileReport::from).collect::<Vec<_>>();
        Self {
            schema: EXPORT_SCHEMA,
            created_at: timestamp_now(),
            version: env!("CARGO_PKG_VERSION"),
            platform: PlatformReport { os: std::env::consts::OS, arch: std::env::consts::ARCH },
            project_root: project_root.display().to_string(),
            summary: ConfigSummary::from_files(&files),
            files,
            skipped: vec![
                "Claude credentials",
                "environment variable dumps",
                "runtime MCP state",
                "plugin inventory",
                "logs",
                "arbitrary project files",
            ],
            redaction: "credential-like keys and bearer/API token values are replaced with [redacted]",
        }
    }

    fn has_failures(&self) -> bool {
        self.files.iter().any(ConfigFileReport::is_failure)
    }
}

#[derive(Debug, Serialize)]
struct PlatformReport {
    os: &'static str,
    arch: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigFileMetadata {
    kind: InspectedConfigFileKind,
    label: &'static str,
    scope: &'static str,
    path: String,
    status: InspectedConfigFileStatus,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl From<&InspectedConfigFile> for ConfigFileMetadata {
    fn from(file: &InspectedConfigFile) -> Self {
        Self {
            kind: file.kind,
            label: file.label,
            scope: file.scope,
            path: file.path.display().to_string(),
            status: file.status,
            state: file.status.as_human(),
            error: file.error.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ConfigFileReport {
    #[serde(flatten)]
    metadata: ConfigFileMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    document: Option<Value>,
}

impl From<&InspectedConfigFile> for ConfigFileReport {
    fn from(file: &InspectedConfigFile) -> Self {
        Self {
            metadata: ConfigFileMetadata::from(file),
            document: file.document.as_ref().map(redacted_document),
        }
    }
}

impl ConfigFileReport {
    fn is_failure(&self) -> bool {
        matches!(
            self.status,
            InspectedConfigFileStatus::Invalid
                | InspectedConfigFileStatus::Unreadable
                | InspectedConfigFileStatus::NotFile
        )
    }

    fn has_document(&self) -> bool {
        self.document.is_some()
    }
}

impl std::ops::Deref for ConfigFileReport {
    type Target = ConfigFileMetadata;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

#[derive(Debug, Serialize)]
struct ConfigSummary {
    found: usize,
    missing: usize,
    invalid: usize,
    documents: usize,
}

impl ConfigSummary {
    fn from_files(files: &[ConfigFileReport]) -> Self {
        Self {
            found: files
                .iter()
                .filter(|file| file.status != InspectedConfigFileStatus::Missing)
                .count(),
            missing: files
                .iter()
                .filter(|file| file.status == InspectedConfigFileStatus::Missing)
                .count(),
            invalid: files.iter().filter(|file| file.is_failure()).count(),
            documents: files.iter().filter(|file| file.has_document()).count(),
        }
    }

    fn as_human(&self) -> String {
        format!("{} files found, {} missing, {} invalid", self.found, self.missing, self.invalid)
    }
}

fn summary_counts(inspection: &InspectedConfigDocuments) -> String {
    let files = inspection.files.iter().map(ConfigFileReport::from).collect::<Vec<_>>();
    ConfigSummary::from_files(&files).as_human()
}

fn redacted_document(document: &Value) -> Value {
    let mut redacted = document.clone();
    redaction::redact_json_value(&mut redacted);
    redacted
}

fn write_json<T: Serialize>(stdout: &mut impl Write, value: &T) -> anyhow::Result<()> {
    writeln!(stdout, "{}", json_text(value)?)?;
    Ok(())
}

fn json_text<T: Serialize>(value: &T) -> anyhow::Result<String> {
    let text = serde_json::to_string_pretty(value)?;
    Ok(redaction::redact_text(&text))
}

fn write_new_file_atomically(path: &Path, text: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent).with_context(|| {
            format!("failed to create config export directory {}", parent.display())
        })?;
    }

    write_new_file(path, text)
}

fn write_new_file(path: &Path, text: &str) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("failed to create config export {}", path.display()))?;

    let write_result = (|| {
        file.write_all(text.as_bytes())
            .with_context(|| format!("failed to write config export {}", path.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("failed to finalize config export {}", path.display()))?;
        file.flush()
            .with_context(|| format!("failed to flush config export {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync config export {}", path.display()))?;
        Ok(())
    })();

    drop(file);

    if let Err(error) = write_result {
        let _ = fs::remove_file(path);
        return Err(error);
    }

    Ok(())
}

fn timestamp_now() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z"))
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

#[derive(Clone, Copy, Debug)]
struct HumanStyle {
    color: bool,
}

impl HumanStyle {
    fn detect() -> Self {
        Self { color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }

    fn title(self, text: &str) -> String {
        if self.color { format!("\x1b[1;36m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn heading(self, text: &str) -> String {
        if self.color { format!("\x1b[1;36m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn detail_label(self, text: &str) -> String {
        if self.color { format!("\x1b[2m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn status(self, status: InspectedConfigFileStatus) -> String {
        let label = format!("[{}]", status.as_tag());
        if !self.color {
            return label;
        }

        match status {
            InspectedConfigFileStatus::Valid => format!("\x1b[32m{label}\x1b[0m"),
            InspectedConfigFileStatus::Missing => format!("\x1b[2m{label}\x1b[0m"),
            InspectedConfigFileStatus::Invalid
            | InspectedConfigFileStatus::Unreadable
            | InspectedConfigFileStatus::NotFile => format!("\x1b[31m{label}\x1b[0m"),
        }
    }

    fn command_block(self, mode: &str, command: &str, purpose: &str) -> String {
        format!("  {} {}\n      {}", self.mode(mode), command, purpose)
    }

    fn mode(self, mode: &str) -> String {
        if self.color { format!("\x1b[2m{mode}\x1b[0m") } else { mode.to_owned() }
    }
}

impl InspectedConfigFileStatus {
    const fn as_tag(self) -> &'static str {
        match self {
            Self::Missing => "MISS",
            Self::Valid => "FILE",
            Self::Invalid => "INVALID",
            Self::Unreadable => "UNREADABLE",
            Self::NotFile => "NOTFILE",
        }
    }

    const fn as_human(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Valid => "valid JSON file",
            Self::Invalid => "invalid JSON",
            Self::Unreadable => "unreadable",
            Self::NotFile => "not a file",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        write_export_with_home_override, write_new_file_atomically, write_path_with_home_override,
        write_show_with_home_override,
    };
    use crate::{Cli, ConfigExportArgs, ConfigFileSelector, ConfigPathArgs, ConfigShowArgs};
    use serde_json::Value;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn path_output_uses_sections_and_status_rows() {
        let dir = tempdir().expect("tempdir");
        let mut stdout = Vec::new();

        let code =
            write_test_path(dir.path(), &ConfigPathArgs { json: false, which: None }, &mut stdout)
                .expect("path");

        assert_eq!(code, 0);
        let output = String::from_utf8(stdout).expect("utf8");
        assert!(output.contains("claude-rs config"));
        assert!(output.contains("Summary: 0 files found, 3 missing, 0 invalid"));
        assert!(output.contains("Locations"));
        assert!(output.contains("[MISS] Global settings"));
        assert!(output.contains("Commands"));
    }

    #[test]
    fn which_path_output_is_script_friendly() {
        let dir = tempdir().expect("tempdir");
        let mut stdout = Vec::new();

        let code = write_test_path(
            dir.path(),
            &ConfigPathArgs { json: true, which: Some(ConfigFileSelector::LocalSettings) },
            &mut stdout,
        )
        .expect("path");

        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(stdout).expect("utf8"),
            format!("{}\n", dir.path().join(".claude").join("settings.local.json").display())
        );
    }

    #[test]
    fn show_json_redacts_nested_secrets() {
        let dir = tempdir().expect("tempdir");
        let settings = dir.path().join(".claude").join("settings.local.json");
        fs::create_dir_all(settings.parent().expect("settings parent")).expect("settings dir");
        fs::write(&settings, r#"{"model":"sonnet","env":{"ANTHROPIC_API_KEY":"sk-ant-secret"}}"#)
            .expect("write settings");
        let mut stdout = Vec::new();

        let code =
            write_test_show(dir.path(), &ConfigShowArgs { json: true }, &mut stdout).expect("show");

        assert_eq!(code, 0);
        let output = String::from_utf8(stdout).expect("utf8");
        assert!(!output.contains("sk-ant-secret"));
        assert!(output.contains("[redacted]"));
        serde_json::from_str::<Value>(&output).expect("json");
    }

    #[test]
    fn show_human_prints_small_redacted_documents() {
        let dir = tempdir().expect("tempdir");
        let settings = dir.path().join(".claude").join("settings.local.json");
        fs::create_dir_all(settings.parent().expect("settings parent")).expect("settings dir");
        fs::write(
            &settings,
            r#"{"permissions":{"allow":["mcp__fff__grep"]},"env":{"ANTHROPIC_API_KEY":"sk-ant-secret"}}"#,
        )
        .expect("write settings");
        let mut stdout = Vec::new();

        let code = write_test_show(dir.path(), &ConfigShowArgs { json: false }, &mut stdout)
            .expect("show");

        assert_eq!(code, 0);
        let output = String::from_utf8(stdout).expect("utf8");
        assert!(output.contains("Redacted document:"));
        assert!(output.contains("mcp__fff__grep"));
        assert!(!output.contains("sk-ant-secret"));
    }

    #[test]
    fn show_human_summarizes_large_documents_without_dumping_contents() {
        let dir = tempdir().expect("tempdir");
        let settings = dir.path().join(".claude").join("settings.local.json");
        fs::create_dir_all(settings.parent().expect("settings parent")).expect("settings dir");
        let values = (0..40).map(|index| format!(r#""value-{index}""#)).collect::<Vec<_>>();
        fs::write(
            &settings,
            format!(
                r#"{{"additionalModelOptionsCache":[{}],"anonymousId":"claudecode.v1.example"}}"#,
                values.join(",")
            ),
        )
        .expect("write settings");
        let mut stdout = Vec::new();

        let code = write_test_show(dir.path(), &ConfigShowArgs { json: false }, &mut stdout)
            .expect("show");

        assert_eq!(code, 0);
        let output = String::from_utf8(stdout).expect("utf8");
        assert!(output.contains("Top-level keys: 2"));
        assert!(output.contains("Redacted document:"));
        assert!(output.contains("hidden from human output"));
        assert!(output.contains("Full redacted JSON: claude-rs config show --json"));
        assert!(!output.contains("value-39"));
        assert!(!output.contains("claudecode.v1.example"));
    }

    #[test]
    fn show_malformed_json_returns_failure_without_backup() {
        let dir = tempdir().expect("tempdir");
        let local_settings = dir.path().join(".claude").join("settings.local.json");
        fs::create_dir_all(local_settings.parent().expect("local settings parent"))
            .expect("local settings dir");
        fs::write(&local_settings, "{ not-json").expect("write malformed");
        let mut stdout = Vec::new();

        let code = write_test_show(dir.path(), &ConfigShowArgs { json: false }, &mut stdout)
            .expect("show");

        assert_eq!(code, 1);
        let output = String::from_utf8(stdout).expect("utf8");
        assert!(output.contains("[INVALID] Local settings"));
        let backups = fs::read_dir(local_settings.parent().expect("local settings parent"))
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path != &local_settings)
            .collect::<Vec<_>>();
        assert!(backups.is_empty());
    }

    #[test]
    fn export_refuses_existing_output_without_overwrite() {
        let dir = tempdir().expect("tempdir");
        let output_path = dir.path().join("config.json");
        fs::write(&output_path, "keep").expect("write output");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = write_test_export(
            dir.path(),
            &ConfigExportArgs { output: Some(output_path.clone()) },
            &mut stdout,
            &mut stderr,
        )
        .expect("export");

        assert_eq!(code, 1);
        assert!(stdout.is_empty());
        assert!(String::from_utf8(stderr).expect("utf8").contains("Refusing to overwrite"));
        assert_eq!(fs::read_to_string(output_path).expect("read output"), "keep");
    }

    #[test]
    fn export_fails_invalid_config_without_partial_file() {
        let dir = tempdir().expect("tempdir");
        let local_settings = dir.path().join(".claude").join("settings.local.json");
        fs::create_dir_all(local_settings.parent().expect("local settings parent"))
            .expect("local settings dir");
        fs::write(local_settings, "{ not-json").expect("write malformed");
        let output_path = dir.path().join("out").join("config.json");
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let code = write_test_export(
            dir.path(),
            &ConfigExportArgs { output: Some(output_path.clone()) },
            &mut stdout,
            &mut stderr,
        )
        .expect("export");

        assert_eq!(code, 1);
        assert!(!output_path.exists());
        assert!(String::from_utf8(stderr).expect("utf8").contains("Cannot export config"));
    }

    #[test]
    fn atomic_export_preserves_existing_file_when_create_new_fails() {
        let dir = tempdir().expect("tempdir");
        let output_path = dir.path().join("config.json");
        fs::write(&output_path, "keep").expect("write output");

        let error = write_new_file_atomically(&output_path, "{}").expect_err("existing output");

        assert!(error.to_string().contains("failed to create config export"));
        assert_eq!(fs::read_to_string(output_path).expect("read output"), "keep");
    }

    fn write_test_path(
        project_root: &Path,
        args: &ConfigPathArgs,
        stdout: &mut impl Write,
    ) -> anyhow::Result<i32> {
        let cli = test_cli(project_root);
        write_path_with_home_override(&cli, args, Some(project_root), stdout)
    }

    fn write_test_show(
        project_root: &Path,
        args: &ConfigShowArgs,
        stdout: &mut impl Write,
    ) -> anyhow::Result<i32> {
        let cli = test_cli(project_root);
        write_show_with_home_override(&cli, args, Some(project_root), stdout)
    }

    fn write_test_export(
        project_root: &Path,
        args: &ConfigExportArgs,
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) -> anyhow::Result<i32> {
        let cli = test_cli(project_root);
        write_export_with_home_override(&cli, args, Some(project_root), stdout, stderr)
    }

    fn test_cli(project_root: &Path) -> Cli {
        Cli {
            command: None,
            no_update_check: false,
            dir: Some(project_root.to_path_buf()),
            bridge_script: None,
            enable_logs: false,
            diagnostics_preset: None,
            log_file: None,
            log_filter: None,
            log_append: false,
            enable_perf: false,
            perf_log: None,
            perf_append: false,
        }
    }
}
