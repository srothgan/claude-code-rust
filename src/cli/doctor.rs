// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::bridge::{
    BRIDGE_RUNTIME_ENV_VAR, BRIDGE_SCRIPT_ENV_VAR, BridgeRuntimeInspection, BridgeScriptInspection,
    inspect_bridge_runtime, inspect_bridge_script,
};
use crate::app::{auth, config};
use crate::{Cli, DoctorArgs};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::IsTerminal as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const MIN_NODE_MAJOR: u64 = 18;
const ROOT_NPM_PACKAGE_NAME: &str = "claude-code-rust";

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DoctorReport {
    version: String,
    platform: PlatformReport,
    checks: Vec<DoctorCheck>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct PlatformReport {
    os: String,
    arch: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct DoctorCheck {
    id: &'static str,
    label: &'static str,
    status: DoctorStatus,
    message: String,
    hard_failure: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    details: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

pub fn run(cli: &Cli, args: &DoctorArgs, writer: &mut impl Write) -> anyhow::Result<i32> {
    let report = build_report(cli);

    if args.json {
        serde_json::to_writer_pretty(&mut *writer, &report)?;
        writeln!(writer)?;
    } else {
        write_human_report(writer, &report)?;
    }

    Ok(i32::from(args.strict && report.has_hard_failures()))
}

pub(crate) fn build_report(cli: &Cli) -> DoctorReport {
    let runtime = inspect_bridge_runtime();
    let script = inspect_bridge_script(cli.bridge_script.as_deref());
    let mut checks = vec![
        binary_version_check(),
        platform_check(),
        current_exe_check(),
        node_runtime_check(&runtime),
        node_version_check(runtime.resolved_path.as_deref()),
        bridge_script_check(&script),
    ];
    checks.extend(config_path_checks(cli.dir.as_deref()));
    checks.extend(log_path_checks());
    checks.extend(npm_metadata_checks(&script));
    checks.push(credentials_check());

    DoctorReport {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: PlatformReport {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        },
        checks,
    }
}

impl DoctorReport {
    fn has_hard_failures(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.hard_failure && matches!(check.status, DoctorStatus::Fail))
    }

    fn status_count(&self, status: DoctorStatus) -> usize {
        self.checks.iter().filter(|check| check.status == status).count()
    }
}

fn write_human_report(writer: &mut impl Write, report: &DoctorReport) -> std::io::Result<()> {
    let style = HumanStyle::detect();
    writeln!(writer, "{}", style.title("claude-rs doctor"))?;
    writeln!(writer, "{} {}", style.detail_label("Version:"), report.version)?;
    writeln!(
        writer,
        "{} {}/{}",
        style.detail_label("Platform:"),
        report.platform.os,
        report.platform.arch
    )?;
    writeln!(
        writer,
        "{} {}, {}, {}",
        style.detail_label("Summary:"),
        style.summary_count(report.status_count(DoctorStatus::Pass), "passed", DoctorStatus::Pass),
        style.summary_count(
            report.status_count(DoctorStatus::Warn),
            "warnings",
            DoctorStatus::Warn
        ),
        style.summary_count(
            report.status_count(DoctorStatus::Fail),
            "failures",
            DoctorStatus::Fail
        )
    )?;
    writeln!(writer)?;

    for section in DoctorSection::ALL {
        let section_checks = report
            .checks
            .iter()
            .filter(|check| DoctorSection::for_check(check.id) == section)
            .collect::<Vec<_>>();
        if section_checks.is_empty() {
            continue;
        }

        writeln!(writer, "{}", style.heading(section.title()))?;
        writeln!(writer, "  {}  {:<22} Result", style.table_header("Status"), "Check")?;
        writeln!(writer, "  {}  {:<22} ------", style.table_header("------"), "-----")?;
        for (index, check) in section_checks.iter().enumerate() {
            write_human_check(writer, check, style)?;
            if index + 1 < section_checks.len() {
                writeln!(writer)?;
            }
        }
        writeln!(writer)?;
    }

    Ok(())
}

fn write_human_check(
    writer: &mut impl Write,
    check: &DoctorCheck,
    style: HumanStyle,
) -> std::io::Result<()> {
    writeln!(writer, "  {}  {:<22} {}", style.status(check.status), check.label, check.message)?;

    let mut candidate_details = Vec::new();
    for (key, value) in &check.details {
        if is_candidate_detail_key(key) {
            candidate_details.push(parse_candidate_detail(value));
        } else {
            writeln!(writer, "      - {} {}", style.detail_label(&format!("{key}:")), value)?;
        }
    }

    if !candidate_details.is_empty() {
        write_candidate_table(writer, &candidate_details, style)?;
    }

    Ok(())
}

fn write_candidate_table(
    writer: &mut impl Write,
    candidates: &[CandidateDetail<'_>],
    style: HumanStyle,
) -> std::io::Result<()> {
    let show_source = candidates.iter().any(|candidate| candidate.source.is_some());
    writeln!(writer, "      {}", style.detail_label("Candidates:"))?;

    if show_source {
        writeln!(
            writer,
            "        {:>2}  {:<21} {:<8} Path",
            style.table_header("#"),
            "Source",
            "State"
        )?;
        writeln!(
            writer,
            "        {:>2}  {:<21} {:<8} ----",
            style.table_header("--"),
            "------",
            "-----"
        )?;
        for (index, candidate) in candidates.iter().enumerate() {
            writeln!(
                writer,
                "        {:>2}. {:<21} {:<8} {}",
                index + 1,
                candidate.source.unwrap_or("-"),
                style.state(candidate.state),
                candidate.path
            )?;
        }
    } else {
        writeln!(writer, "        {:>2}  {:<8} Path", style.table_header("#"), "State")?;
        writeln!(writer, "        {:>2}  {:<8} ----", style.table_header("--"), "-----")?;
        for (index, candidate) in candidates.iter().enumerate() {
            writeln!(
                writer,
                "        {:>2}. {:<8} {}",
                index + 1,
                style.state(candidate.state),
                candidate.path
            )?;
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CandidateDetail<'a> {
    source: Option<&'a str>,
    path: &'a str,
    state: &'a str,
}

fn parse_candidate_detail(value: &str) -> CandidateDetail<'_> {
    let (value, state) = value
        .strip_suffix(')')
        .and_then(|without_suffix| without_suffix.rsplit_once(" ("))
        .unwrap_or((value, ""));
    let (source, path) = value
        .split_once(": ")
        .filter(|(source, _)| !source.contains('\\') && !source.contains('/'))
        .map_or((None, value), |(source, path)| (Some(source), path));

    CandidateDetail { source, path, state }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DoctorSection {
    Overview,
    Runtime,
    Configuration,
    Logs,
    Npm,
    Credentials,
}

impl DoctorSection {
    const ALL: [Self; 6] = [
        Self::Overview,
        Self::Runtime,
        Self::Configuration,
        Self::Logs,
        Self::Npm,
        Self::Credentials,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Runtime => "Runtime prerequisites",
            Self::Configuration => "Configuration",
            Self::Logs => "Logs",
            Self::Npm => "npm installation",
            Self::Credentials => "Claude credentials",
        }
    }

    fn for_check(id: &str) -> Self {
        match id {
            "node_runtime" | "node_version" | "bridge_script" => Self::Runtime,
            "config_settings" | "config_local_settings" | "config_preferences" | "config_paths" => {
                Self::Configuration
            }
            "runtime_log_dir" | "legacy_log_path" | "perf_log_dir" => Self::Logs,
            "npm_root_package" | "npm_platform_package" => Self::Npm,
            "claude_credentials" => Self::Credentials,
            _ => Self::Overview,
        }
    }
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

    fn table_header(self, text: &str) -> String {
        if self.color { format!("\x1b[2m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn detail_label(self, text: &str) -> String {
        if self.color { format!("\x1b[2m{text}\x1b[0m") } else { text.to_owned() }
    }

    fn status(self, status: DoctorStatus) -> String {
        let label = format!("[{}]", status.as_label());
        if !self.color {
            return label;
        }

        match status {
            DoctorStatus::Pass => format!("\x1b[32m{label}\x1b[0m"),
            DoctorStatus::Warn => format!("\x1b[33m{label}\x1b[0m"),
            DoctorStatus::Fail => format!("\x1b[31m{label}\x1b[0m"),
        }
    }

    fn state(self, state: &str) -> String {
        if !self.color {
            return state.to_owned();
        }

        match state {
            "file" | "directory" | "exists" => format!("\x1b[32m{state}\x1b[0m"),
            "missing" => format!("\x1b[2m{state}\x1b[0m"),
            _ => state.to_owned(),
        }
    }

    fn summary_count(self, count: usize, label: &str, status: DoctorStatus) -> String {
        let text = format!("{count} {label}");
        if !self.color {
            return text;
        }

        match status {
            DoctorStatus::Pass => format!("\x1b[32m{text}\x1b[0m"),
            DoctorStatus::Warn => format!("\x1b[33m{text}\x1b[0m"),
            DoctorStatus::Fail => format!("\x1b[31m{text}\x1b[0m"),
        }
    }
}

fn is_candidate_detail_key(key: &str) -> bool {
    key.starts_with("candidate_") || key.starts_with("packaged_candidate_")
}

impl DoctorStatus {
    fn as_label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

fn binary_version_check() -> DoctorCheck {
    DoctorCheck::pass("binary_version", "Binary version", env!("CARGO_PKG_VERSION"))
}

fn platform_check() -> DoctorCheck {
    let mut details = BTreeMap::new();
    details.insert("os".to_owned(), std::env::consts::OS.to_owned());
    details.insert("arch".to_owned(), std::env::consts::ARCH.to_owned());
    DoctorCheck::pass_with_details("platform", "Platform", "platform detected", details)
}

fn current_exe_check() -> DoctorCheck {
    match std::env::current_exe() {
        Ok(path) => DoctorCheck::pass_with_detail(
            "current_exe",
            "Current executable",
            "current executable resolved",
            "path",
            path.display().to_string(),
        ),
        Err(error) => DoctorCheck::warn(
            "current_exe",
            "Current executable",
            format!("failed to resolve current executable: {error}"),
        ),
    }
}

fn node_runtime_check(inspection: &BridgeRuntimeInspection) -> DoctorCheck {
    let mut details = BTreeMap::new();
    details.insert(
        BRIDGE_RUNTIME_ENV_VAR.to_owned(),
        inspection
            .env_path
            .as_ref()
            .map_or_else(|| "unset".to_owned(), |path| path.display().to_string()),
    );
    if let Some(path) = &inspection.path_node {
        details.insert("path_node".to_owned(), path.display().to_string());
    }
    for (index, candidate) in inspection.packaged_candidates.iter().enumerate() {
        details.insert(
            format!("packaged_candidate_{index}"),
            format!("{} ({})", candidate.path.display(), file_state(candidate.path.as_path())),
        );
    }

    match (&inspection.resolved_path, &inspection.error) {
        (Some(path), _) => DoctorCheck::pass_with_details(
            "node_runtime",
            "Node runtime",
            format!("resolved {}", path.display()),
            details,
        ),
        (None, Some(error)) => DoctorCheck::fail_hard_with_details(
            "node_runtime",
            "Node runtime",
            error.clone(),
            details,
        ),
        (None, None) => DoctorCheck::fail_hard_with_details(
            "node_runtime",
            "Node runtime",
            "Node.js runtime not found".to_owned(),
            details,
        ),
    }
}

fn node_version_check(runtime: Option<&Path>) -> DoctorCheck {
    let Some(runtime) = runtime else {
        return DoctorCheck::fail_hard(
            "node_version",
            "Node version",
            "cannot check Node version because no runtime was resolved",
        );
    };

    match Command::new(runtime).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            classify_node_version(&raw, runtime)
        }
        Ok(output) => DoctorCheck::fail_hard(
            "node_version",
            "Node version",
            format!("failed to run `{}` --version: {}", runtime.display(), output.status),
        ),
        Err(error) => DoctorCheck::fail_hard(
            "node_version",
            "Node version",
            format!("failed to run `{}` --version: {error}", runtime.display()),
        ),
    }
}

fn classify_node_version(raw: &str, runtime: &Path) -> DoctorCheck {
    let Some(major) = parse_node_major(raw) else {
        return DoctorCheck::fail_hard_with_detail(
            "node_version",
            "Node version",
            format!("could not parse Node version output `{raw}`"),
            "runtime",
            runtime.display().to_string(),
        );
    };

    let mut details = BTreeMap::new();
    details.insert("runtime".to_owned(), runtime.display().to_string());
    details.insert("version".to_owned(), raw.to_owned());
    details.insert("minimum_major".to_owned(), MIN_NODE_MAJOR.to_string());

    if major >= MIN_NODE_MAJOR {
        DoctorCheck::pass_with_details("node_version", "Node version", raw, details)
    } else {
        DoctorCheck::fail_hard_with_details(
            "node_version",
            "Node version",
            format!("Node.js {raw} is older than required major version {MIN_NODE_MAJOR}"),
            details,
        )
    }
}

fn parse_node_major(raw: &str) -> Option<u64> {
    raw.trim().strip_prefix('v').unwrap_or(raw.trim()).split('.').next()?.parse().ok()
}

fn bridge_script_check(inspection: &BridgeScriptInspection) -> DoctorCheck {
    let mut details = BTreeMap::new();
    details.insert(
        BRIDGE_SCRIPT_ENV_VAR.to_owned(),
        inspection
            .env_path
            .as_ref()
            .map_or_else(|| "unset".to_owned(), |path| path.display().to_string()),
    );
    if let Some(path) = &inspection.explicit_path {
        details.insert("explicit_bridge_script".to_owned(), path.display().to_string());
    }
    for (index, candidate) in inspection.candidates.iter().enumerate() {
        details.insert(
            format!("candidate_{index}"),
            format!(
                "{}: {} ({})",
                candidate.source,
                candidate.path.display(),
                file_state(candidate.path.as_path())
            ),
        );
    }

    match (&inspection.resolved_path, &inspection.error) {
        (Some(path), _) => DoctorCheck::pass_with_details(
            "bridge_script",
            "Bridge script",
            format!("resolved {}", path.display()),
            details,
        ),
        (None, Some(error)) => DoctorCheck::fail_hard_with_details(
            "bridge_script",
            "Bridge script",
            error.clone(),
            details,
        ),
        (None, None) => DoctorCheck::fail_hard_with_details(
            "bridge_script",
            "Bridge script",
            "bridge script not found".to_owned(),
            details,
        ),
    }
}

fn config_path_checks(project_root: Option<&Path>) -> Vec<DoctorCheck> {
    match config::store::resolve_paths(None, project_root) {
        Ok(paths) => vec![
            path_check("config_settings", "Global settings", &paths.settings, false),
            path_check("config_local_settings", "Local settings", &paths.local_settings, false),
            path_check("config_preferences", "Preferences", &paths.preferences, false),
        ],
        Err(error) => vec![DoctorCheck::warn(
            "config_paths",
            "Config paths",
            format!("failed to resolve config paths: {error}"),
        )],
    }
}

fn log_path_checks() -> Vec<DoctorCheck> {
    vec![
        result_path_check(
            "runtime_log_dir",
            "Runtime log directory",
            crate::logging::default_runtime_log_dir(),
            false,
        ),
        result_path_check(
            "legacy_log_path",
            "Legacy log path",
            crate::logging::default_legacy_log_path(),
            false,
        ),
        result_path_check(
            "perf_log_dir",
            "Perf log directory",
            crate::logging::default_perf_log_dir(),
            false,
        ),
    ]
}

fn result_path_check(
    id: &'static str,
    label: &'static str,
    result: anyhow::Result<PathBuf>,
    require_exists: bool,
) -> DoctorCheck {
    match result {
        Ok(path) => path_check(id, label, &path, require_exists),
        Err(error) => DoctorCheck::warn(id, label, format!("failed to resolve path: {error}")),
    }
}

fn path_check(
    id: &'static str,
    label: &'static str,
    path: &Path,
    require_exists: bool,
) -> DoctorCheck {
    let mut details = BTreeMap::new();
    details.insert("path".to_owned(), path.display().to_string());
    details.insert("state".to_owned(), file_state(path).to_owned());

    if require_exists && !path.exists() {
        DoctorCheck::fail_hard_with_details(id, label, "path is missing".to_owned(), details)
    } else {
        DoctorCheck::pass_with_details(id, label, "path resolved", details)
    }
}

fn npm_metadata_checks(script: &BridgeScriptInspection) -> Vec<DoctorCheck> {
    vec![root_package_check(script), platform_package_check(script)]
}

fn root_package_check(script: &BridgeScriptInspection) -> DoctorCheck {
    let candidates = root_package_json_candidates(script);
    package_json_check(
        "npm_root_package",
        "npm root package",
        candidates,
        Some(ROOT_NPM_PACKAGE_NAME),
        false,
    )
}

fn platform_package_check(script: &BridgeScriptInspection) -> DoctorCheck {
    let mut candidates = Vec::new();
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(bin_dir) = current_exe.parent()
        && let Some(package_root) = bin_dir.parent()
    {
        candidates.push(package_root.join("package.json"));
    }

    if let (Some(root), Some(package_name)) = (root_package_dir(script), platform_package_name()) {
        candidates.push(root.join("node_modules").join(package_name).join("package.json"));
    }

    package_json_check(
        "npm_platform_package",
        "npm platform package",
        candidates,
        platform_package_name(),
        true,
    )
}

fn package_json_check(
    id: &'static str,
    label: &'static str,
    candidates: Vec<PathBuf>,
    expected_name: Option<&str>,
    optional: bool,
) -> DoctorCheck {
    let mut details = BTreeMap::new();
    for (index, path) in candidates.iter().enumerate() {
        details.insert(
            format!("candidate_{index}"),
            format!("{} ({})", path.display(), file_state(path)),
        );
    }

    let mut rejected_packages = Vec::new();
    for (index, path) in candidates.into_iter().enumerate() {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return DoctorCheck::warn_with_details(
                id,
                label,
                format!("package metadata is not valid JSON: {}", path.display()),
                details,
            );
        };
        let name = json.get("name").and_then(serde_json::Value::as_str).unwrap_or("<unknown>");
        let version =
            json.get("version").and_then(serde_json::Value::as_str).unwrap_or("<unknown>");
        if let Some(expected_name) = expected_name
            && name != expected_name
        {
            details.insert(format!("candidate_{index}_name"), name.to_owned());
            rejected_packages.push(format!("{} ({name})", path.display()));
            continue;
        }

        details.insert("path".to_owned(), path.display().to_string());
        details.insert("name".to_owned(), name.to_owned());
        details.insert("version".to_owned(), version.to_owned());
        return DoctorCheck::pass_with_details(id, label, format!("{name} {version}"), details);
    }

    let message = if let (Some(expected_name), false) =
        (expected_name, rejected_packages.is_empty())
    {
        format!(
            "expected {expected_name} package metadata but found unrelated package metadata: {}",
            rejected_packages.join(", ")
        )
    } else if let Some(expected_name) = expected_name {
        format!("{expected_name} package metadata was not discoverable")
    } else if optional {
        "optional platform package metadata was not discoverable".to_owned()
    } else {
        "npm root package metadata was not discoverable".to_owned()
    };
    DoctorCheck::warn_with_details(id, label, message, details)
}

fn root_package_json_candidates(script: &BridgeScriptInspection) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(root) = root_package_dir(script) {
        candidates.push(root.join("package.json"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("package.json"));
    }
    unique_paths(candidates)
}

fn root_package_dir(script: &BridgeScriptInspection) -> Option<PathBuf> {
    let script = script.resolved_path.as_ref()?;
    let dist_dir = script.parent()?;
    let agent_sdk_dir = dist_dir.parent()?;
    agent_sdk_dir.parent().map(Path::to_path_buf)
}

fn unique_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.iter().any(|candidate| candidate == &path) {
            unique.push(path);
        }
    }
    unique
}

fn platform_package_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("@srothgan/claude-code-rust-darwin-arm64"),
        ("macos", "x86_64") => Some("@srothgan/claude-code-rust-darwin-x64"),
        ("linux", "x86_64") => Some("@srothgan/claude-code-rust-linux-x64-gnu"),
        ("windows", "x86_64") => Some("@srothgan/claude-code-rust-win32-x64-msvc"),
        _ => None,
    }
}

fn credentials_check() -> DoctorCheck {
    let mut details = BTreeMap::new();
    if let Some(path) = auth::credentials_path() {
        details.insert("path".to_owned(), path.display().to_string());
        details.insert("state".to_owned(), file_state(&path).to_owned());
    }

    if auth::has_credentials() {
        DoctorCheck::pass_with_details(
            "claude_credentials",
            "Claude credentials",
            "Claude OAuth credentials appear configured",
            details,
        )
    } else {
        DoctorCheck::warn_with_details(
            "claude_credentials",
            "Claude credentials",
            "Claude credentials were not found or are not readable",
            details,
        )
    }
}

fn file_state(path: &Path) -> &'static str {
    if path.is_file() {
        "file"
    } else if path.is_dir() {
        "directory"
    } else if path.exists() {
        "exists"
    } else {
        "missing"
    }
}

impl DoctorCheck {
    fn pass(id: &'static str, label: &'static str, message: impl Into<String>) -> Self {
        Self::new(id, label, DoctorStatus::Pass, message, false, BTreeMap::new())
    }

    fn pass_with_detail(
        id: &'static str,
        label: &'static str,
        message: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let mut details = BTreeMap::new();
        details.insert(key.into(), value.into());
        Self::pass_with_details(id, label, message, details)
    }

    fn pass_with_details(
        id: &'static str,
        label: &'static str,
        message: impl Into<String>,
        details: BTreeMap<String, String>,
    ) -> Self {
        Self::new(id, label, DoctorStatus::Pass, message, false, details)
    }

    fn warn(id: &'static str, label: &'static str, message: impl Into<String>) -> Self {
        Self::new(id, label, DoctorStatus::Warn, message, false, BTreeMap::new())
    }

    fn warn_with_details(
        id: &'static str,
        label: &'static str,
        message: impl Into<String>,
        details: BTreeMap<String, String>,
    ) -> Self {
        Self::new(id, label, DoctorStatus::Warn, message, false, details)
    }

    fn fail_hard(id: &'static str, label: &'static str, message: impl Into<String>) -> Self {
        Self::new(id, label, DoctorStatus::Fail, message, true, BTreeMap::new())
    }

    fn fail_hard_with_detail(
        id: &'static str,
        label: &'static str,
        message: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        let mut details = BTreeMap::new();
        details.insert(key.into(), value.into());
        Self::fail_hard_with_details(id, label, message, details)
    }

    fn fail_hard_with_details(
        id: &'static str,
        label: &'static str,
        message: impl Into<String>,
        details: BTreeMap<String, String>,
    ) -> Self {
        Self::new(id, label, DoctorStatus::Fail, message, true, details)
    }

    fn new(
        id: &'static str,
        label: &'static str,
        status: DoctorStatus,
        message: impl Into<String>,
        hard_failure: bool,
        details: BTreeMap<String, String>,
    ) -> Self {
        Self { id, label, status, message: message.into(), hard_failure, details }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DoctorReport, DoctorStatus, MIN_NODE_MAJOR, PlatformReport, ROOT_NPM_PACKAGE_NAME,
        classify_node_version, config_path_checks, package_json_check, parse_node_major,
        write_human_report,
    };
    use std::path::Path;

    #[test]
    fn parses_node_major_versions() {
        assert_eq!(parse_node_major("v20.11.1"), Some(20));
        assert_eq!(parse_node_major("18.19.0"), Some(18));
        assert_eq!(parse_node_major("not-node"), None);
    }

    #[test]
    fn node_version_below_minimum_is_hard_failure() {
        let raw = format!("v{}.0.0", MIN_NODE_MAJOR - 1);
        let check = classify_node_version(&raw, Path::new("node"));

        assert_eq!(check.status, DoctorStatus::Fail);
        assert!(check.hard_failure);
    }

    #[test]
    fn human_report_does_not_print_json() {
        let report = DoctorReport {
            version: "1.2.3".to_owned(),
            platform: PlatformReport { os: "test-os".to_owned(), arch: "test-arch".to_owned() },
            checks: vec![super::DoctorCheck::pass("binary_version", "Binary version", "1.2.3")],
        };
        let mut output = Vec::new();

        write_human_report(&mut output, &report).expect("write report");

        let output = String::from_utf8(output).expect("utf8");
        assert!(output.contains("claude-rs doctor"));
        assert!(output.contains("[PASS]  Binary version"));
        assert!(output.contains("Status  Check"));
        assert!(!output.trim_start().starts_with('{'));
    }

    #[test]
    fn config_path_checks_use_project_root_override_for_local_settings() {
        let project = tempfile::tempdir().expect("tempdir");

        let checks = config_path_checks(Some(project.path()));

        let local = checks
            .iter()
            .find(|check| check.id == "config_local_settings")
            .expect("local settings check");
        assert_eq!(
            local.details.get("path").map(String::as_str),
            Some(
                project
                    .path()
                    .join(".claude")
                    .join("settings.local.json")
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[test]
    fn package_json_check_skips_unrelated_packages_until_expected_package() {
        let dir = tempfile::tempdir().expect("tempdir");
        let unrelated = dir.path().join("unrelated").join("package.json");
        let expected = dir.path().join("expected").join("package.json");
        std::fs::create_dir_all(unrelated.parent().expect("unrelated parent"))
            .expect("create unrelated parent");
        std::fs::create_dir_all(expected.parent().expect("expected parent"))
            .expect("create expected parent");
        std::fs::write(&unrelated, r#"{"name":"other-project","version":"9.9.9"}"#)
            .expect("write unrelated package");
        std::fs::write(
            &expected,
            format!(r#"{{"name":"{ROOT_NPM_PACKAGE_NAME}","version":"1.2.3"}}"#),
        )
        .expect("write expected package");

        let check = package_json_check(
            "npm_root_package",
            "npm root package",
            vec![unrelated, expected.clone()],
            Some(ROOT_NPM_PACKAGE_NAME),
            false,
        );

        assert_eq!(check.status, DoctorStatus::Pass);
        assert_eq!(
            check.details.get("path").map(String::as_str),
            Some(expected.to_string_lossy().as_ref())
        );
        assert_eq!(check.details.get("name").map(String::as_str), Some(ROOT_NPM_PACKAGE_NAME));
    }

    #[test]
    fn package_json_check_warns_on_unrelated_package_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let unrelated = dir.path().join("package.json");
        std::fs::write(&unrelated, r#"{"name":"other-project","version":"9.9.9"}"#)
            .expect("write unrelated package");

        let check = package_json_check(
            "npm_root_package",
            "npm root package",
            vec![unrelated],
            Some(ROOT_NPM_PACKAGE_NAME),
            false,
        );

        assert_eq!(check.status, DoctorStatus::Warn);
        assert!(check.message.contains("expected claude-code-rust package metadata"));
    }
}
