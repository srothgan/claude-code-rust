// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::error::AppError;
use anyhow::Context as _;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub const BRIDGE_SCRIPT_RELATIVE_PATH: &str = "agent-sdk/dist/bridge.js";
pub const BRIDGE_SCRIPT_ENV_VAR: &str = "CLAUDE_RS_AGENT_BRIDGE";
pub const BRIDGE_RUNTIME_ENV_VAR: &str = "CLAUDE_RS_AGENT_BRIDGE_RUNTIME";
const ROOT_NPM_PACKAGE_NAME: &str = "claude-code-rust";
const MAX_BRIDGE_EXE_ANCESTORS: usize = 8;
#[cfg(windows)]
const BUNDLED_BUN_RUNTIME_FILE_NAME: &str = "claude-rs-bridge-bun.exe";
#[cfg(not(windows))]
const BUNDLED_BUN_RUNTIME_FILE_NAME: &str = "claude-rs-bridge-bun";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeLauncher {
    pub runtime_path: PathBuf,
    pub script_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeCandidateInspection {
    pub source: String,
    pub path: PathBuf,
    pub is_file: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeRuntimeKind {
    Explicit,
    BundledBun,
    PathBun,
}

impl BridgeRuntimeKind {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::BundledBun => "bundled_bun",
            Self::PathBun => "path_bun",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeScriptInspection {
    pub explicit_path: Option<PathBuf>,
    pub env_path: Option<PathBuf>,
    pub candidates: Vec<BridgeCandidateInspection>,
    pub resolved_path: Option<PathBuf>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeRuntimeInspection {
    pub env_path: Option<PathBuf>,
    pub packaged_candidates: Vec<BridgeCandidateInspection>,
    pub path_bun: Option<PathBuf>,
    pub resolved_kind: Option<BridgeRuntimeKind>,
    pub resolved_path: Option<PathBuf>,
    pub error: Option<String>,
}

impl BridgeLauncher {
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{} {}", self.runtime_path.to_string_lossy(), self.script_path.to_string_lossy())
    }

    #[must_use]
    pub fn command(&self, bridge_diagnostics_enabled: bool) -> Command {
        let mut cmd = Command::new(&self.runtime_path);
        cmd.arg(&self.script_path);
        cmd.env("CLAUDE_RS_BRIDGE_DIAGNOSTICS", if bridge_diagnostics_enabled { "1" } else { "0" });
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.kill_on_drop(true);
        cmd.stderr(if bridge_diagnostics_enabled {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        });
        cmd
    }
}

pub fn resolve_bridge_launcher(explicit_script: Option<&Path>) -> anyhow::Result<BridgeLauncher> {
    let script = resolve_bridge_script_path(explicit_script)?;
    let runtime = resolve_bridge_runtime_path()?;
    Ok(BridgeLauncher { runtime_path: runtime, script_path: script })
}

pub fn inspect_bridge_script(explicit_script: Option<&Path>) -> BridgeScriptInspection {
    let resolver = BridgeScriptResolver::from_process(explicit_script);
    let explicit_path = resolver.explicit_script.map(Path::to_path_buf);

    if let Some(path) = resolver.explicit_script {
        let is_file = path.is_file();
        return BridgeScriptInspection {
            explicit_path,
            env_path: resolver.env_script.clone(),
            candidates: Vec::new(),
            resolved_path: is_file.then(|| path.to_path_buf()),
            error: (!is_file).then(|| {
                if path.exists() {
                    format!("bridge script is not a file: {}", path.display())
                } else {
                    format!("bridge script does not exist: {}", path.display())
                }
            }),
        };
    }

    if let Some(path) = resolver.env_script.as_deref() {
        let is_file = path.is_file();
        return BridgeScriptInspection {
            explicit_path,
            env_path: resolver.env_script.clone(),
            candidates: Vec::new(),
            resolved_path: is_file.then(|| path.to_path_buf()),
            error: (!is_file).then(|| {
                if path.exists() {
                    format!(
                        "bridge script from {BRIDGE_SCRIPT_ENV_VAR} is not a file: {}",
                        path.display()
                    )
                } else {
                    format!(
                        "bridge script from {BRIDGE_SCRIPT_ENV_VAR} does not exist: {}",
                        path.display()
                    )
                }
            }),
        };
    }

    let candidates = resolver
        .automatic_candidates()
        .into_iter()
        .map(|candidate| BridgeCandidateInspection {
            source: candidate.source.label().to_owned(),
            is_file: candidate.path.is_file(),
            path: candidate.path,
        })
        .collect::<Vec<_>>();
    let resolved_path = candidates
        .iter()
        .find(|candidate| candidate.is_file)
        .map(|candidate| candidate.path.clone());

    BridgeScriptInspection {
        explicit_path,
        env_path: resolver.env_script.clone(),
        error: resolved_path
            .is_none()
            .then(|| "bridge script not found near the installed executable".to_owned()),
        candidates,
        resolved_path,
    }
}

pub fn inspect_bridge_runtime() -> BridgeRuntimeInspection {
    let env_path = runtime_override_from_env();
    let current_exe = current_exe_path();
    inspect_bridge_runtime_with(
        env_path.as_deref(),
        current_exe.as_deref(),
        cfg!(debug_assertions),
        || which::which("bun"),
    )
}

fn inspect_bridge_runtime_with(
    env_path: Option<&Path>,
    current_exe: Option<&Path>,
    allow_dev_fallbacks: bool,
    bun_lookup: impl FnOnce() -> Result<PathBuf, which::Error>,
) -> BridgeRuntimeInspection {
    if allow_dev_fallbacks && let Some(path) = env_path {
        let is_file = path.is_file();
        let resolved_path = is_file.then(|| path.to_path_buf());
        let resolved_kind = is_file.then_some(BridgeRuntimeKind::Explicit);
        let error = (!is_file).then(|| {
            if path.exists() {
                format!(
                    "bridge runtime from {BRIDGE_RUNTIME_ENV_VAR} is not a file: {}",
                    path.display()
                )
            } else {
                format!(
                    "bridge runtime from {BRIDGE_RUNTIME_ENV_VAR} does not exist: {}",
                    path.display()
                )
            }
        });
        return BridgeRuntimeInspection {
            env_path: env_path.map(Path::to_path_buf),
            packaged_candidates: Vec::new(),
            path_bun: None,
            resolved_kind,
            resolved_path,
            error,
        };
    }

    let packaged_candidates = bundled_bun_runtime_candidates(current_exe)
        .into_iter()
        .map(|path| BridgeCandidateInspection {
            source: "packaged-runtime".to_owned(),
            is_file: path.is_file(),
            path,
        })
        .collect::<Vec<_>>();
    let packaged_runtime = packaged_candidates
        .iter()
        .find(|candidate| candidate.is_file)
        .map(|candidate| candidate.path.clone());
    if let Some(resolved_path) = packaged_runtime {
        return BridgeRuntimeInspection {
            env_path: env_path.map(Path::to_path_buf),
            packaged_candidates,
            path_bun: None,
            resolved_kind: Some(BridgeRuntimeKind::BundledBun),
            resolved_path: Some(resolved_path),
            error: None,
        };
    }

    let path_bun = allow_dev_fallbacks.then(bun_lookup).and_then(Result::ok);
    let resolved_kind = path_bun.as_ref().map(|_| BridgeRuntimeKind::PathBun);
    let resolved_path = path_bun.clone();

    BridgeRuntimeInspection {
        env_path: env_path.map(Path::to_path_buf),
        packaged_candidates,
        path_bun,
        resolved_kind,
        error: resolved_path.is_none().then(|| "bundled Bun bridge runtime not found".to_owned()),
        resolved_path,
    }
}

#[cfg(test)]
fn resolve_bridge_launcher_with_runtime(
    runtime_path: PathBuf,
    explicit_script: Option<&Path>,
) -> anyhow::Result<BridgeLauncher> {
    let script_path = resolve_bridge_script_path(explicit_script)?;
    Ok(BridgeLauncher { runtime_path, script_path })
}

fn resolve_bridge_script_path(explicit_script: Option<&Path>) -> anyhow::Result<PathBuf> {
    BridgeScriptResolver::from_process(explicit_script).resolve()
}

fn resolve_bridge_runtime_path() -> anyhow::Result<PathBuf> {
    let env_runtime = runtime_override_from_env();
    let current_exe = current_exe_path();
    resolve_bridge_runtime_path_with(
        env_runtime.as_deref(),
        current_exe.as_deref(),
        cfg!(debug_assertions),
        || which::which("bun"),
    )
}

fn resolve_bridge_runtime_path_with(
    env_runtime: Option<&Path>,
    current_exe: Option<&Path>,
    allow_dev_fallbacks: bool,
    bun_lookup: impl FnOnce() -> Result<PathBuf, which::Error>,
) -> anyhow::Result<PathBuf> {
    if allow_dev_fallbacks && let Some(path) = env_runtime {
        return validate_runtime_path(path)
            .with_context(|| format!("invalid {BRIDGE_RUNTIME_ENV_VAR} runtime override"));
    }

    for candidate in bundled_bun_runtime_candidates(current_exe) {
        if is_automatic_runtime_candidate(&candidate) {
            return Ok(candidate);
        }
    }

    if allow_dev_fallbacks {
        return bun_lookup()
            .map_err(|_| anyhow::Error::new(AppError::BridgeRuntimeNotFound))
            .context("failed to resolve development `bun` runtime");
    }

    Err(anyhow::Error::new(AppError::BridgeRuntimeNotFound))
        .context("failed to resolve bundled Bun bridge runtime")
}

fn validate_script_path(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.exists() {
        return Err(anyhow::Error::new(AppError::BridgeSpawnFailed)
            .context(format!("bridge script does not exist: {}", path.display())));
    }
    if !path.is_file() {
        return Err(anyhow::Error::new(AppError::BridgeSpawnFailed)
            .context(format!("bridge script is not a file: {}", path.display())));
    }
    Ok(path.to_path_buf())
}

fn validate_runtime_path(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.exists() {
        return Err(anyhow::Error::new(AppError::BridgeRuntimeNotFound)
            .context(format!("bridge runtime does not exist: {}", path.display())));
    }
    if !path.is_file() {
        return Err(anyhow::Error::new(AppError::BridgeRuntimeNotFound)
            .context(format!("bridge runtime is not a file: {}", path.display())));
    }
    Ok(path.to_path_buf())
}

struct BridgeScriptResolver<'a> {
    explicit_script: Option<&'a Path>,
    env_script: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    allow_dev_fallbacks: bool,
    cwd_script: PathBuf,
    manifest_script: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutomaticLookupMode {
    CargoDevTarget,
    ExecutableRelativePreferred,
    DevFallbackOnly,
    ExecutableRelativeOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutomaticCandidateSource {
    ExecutableRelative,
    WorkingDirectory,
    ManifestProject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AutomaticCandidate {
    source: AutomaticCandidateSource,
    path: PathBuf,
}

impl AutomaticCandidate {
    fn describe(&self) -> String {
        format!("{}: {}", self.source.label(), self.path.display())
    }
}

impl AutomaticCandidateSource {
    fn label(self) -> &'static str {
        match self {
            Self::ExecutableRelative => "executable-relative",
            Self::WorkingDirectory => "working-directory",
            Self::ManifestProject => "manifest-project",
        }
    }
}

impl<'a> BridgeScriptResolver<'a> {
    fn from_process(explicit_script: Option<&'a Path>) -> Self {
        Self {
            explicit_script,
            env_script: std::env::var_os(BRIDGE_SCRIPT_ENV_VAR).map(PathBuf::from),
            current_exe: current_exe_path(),
            allow_dev_fallbacks: cfg!(debug_assertions),
            cwd_script: PathBuf::from(BRIDGE_SCRIPT_RELATIVE_PATH),
            manifest_script: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(BRIDGE_SCRIPT_RELATIVE_PATH),
        }
    }

    fn resolve(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = self.explicit_script {
            return validate_script_path(path);
        }

        if let Some(path) = self.env_script.as_deref() {
            return validate_script_path(path);
        }

        let candidates = self.automatic_candidates();
        for candidate in &candidates {
            if is_automatic_script_candidate(&candidate.path) {
                return Ok(candidate.path.clone());
            }
        }

        let checked =
            candidates.iter().map(AutomaticCandidate::describe).collect::<Vec<_>>().join(", ");

        Err(anyhow::anyhow!(
            "bridge script not found near the installed executable. lookup mode {:?} checked: {}. expected bundled `agent-sdk/dist/bridge.js`; debug builds also check repo-local fallbacks. set CLAUDE_RS_AGENT_BRIDGE to override.",
            self.lookup_mode(),
            if checked.is_empty() { "<none>" } else { checked.as_str() }
        ))
    }

    fn automatic_candidates(&self) -> Vec<AutomaticCandidate> {
        let mut candidates = Vec::new();

        match self.lookup_mode() {
            AutomaticLookupMode::CargoDevTarget => {
                Self::push_candidate(
                    &mut candidates,
                    AutomaticCandidateSource::ManifestProject,
                    self.manifest_script.clone(),
                );
                Self::push_candidate(
                    &mut candidates,
                    AutomaticCandidateSource::WorkingDirectory,
                    self.cwd_script.clone(),
                );
                self.push_executable_relative_candidates(&mut candidates);
            }
            AutomaticLookupMode::ExecutableRelativePreferred => {
                self.push_executable_relative_candidates(&mut candidates);
                Self::push_candidate(
                    &mut candidates,
                    AutomaticCandidateSource::WorkingDirectory,
                    self.cwd_script.clone(),
                );
                Self::push_candidate(
                    &mut candidates,
                    AutomaticCandidateSource::ManifestProject,
                    self.manifest_script.clone(),
                );
            }
            AutomaticLookupMode::DevFallbackOnly => {
                Self::push_candidate(
                    &mut candidates,
                    AutomaticCandidateSource::WorkingDirectory,
                    self.cwd_script.clone(),
                );
                Self::push_candidate(
                    &mut candidates,
                    AutomaticCandidateSource::ManifestProject,
                    self.manifest_script.clone(),
                );
            }
            AutomaticLookupMode::ExecutableRelativeOnly => {
                self.push_executable_relative_candidates(&mut candidates);
            }
        }

        candidates
    }

    fn lookup_mode(&self) -> AutomaticLookupMode {
        match (self.allow_dev_fallbacks, self.current_exe.as_deref()) {
            (true, Some(current_exe)) if self.is_cargo_dev_target_executable(current_exe) => {
                AutomaticLookupMode::CargoDevTarget
            }
            (true, Some(_)) => AutomaticLookupMode::ExecutableRelativePreferred,
            (true, None) => AutomaticLookupMode::DevFallbackOnly,
            (false, Some(_) | None) => AutomaticLookupMode::ExecutableRelativeOnly,
        }
    }

    fn is_cargo_dev_target_executable(&self, current_exe: &Path) -> bool {
        let Some(manifest_root) = manifest_root_from_script(&self.manifest_script) else {
            return false;
        };
        current_exe.starts_with(manifest_root.join("target"))
    }

    fn push_executable_relative_candidates(&self, candidates: &mut Vec<AutomaticCandidate>) {
        let Some(current_exe) = self.current_exe.as_deref() else {
            return;
        };
        for path in exe_relative_bridge_candidates(current_exe) {
            Self::push_candidate(candidates, AutomaticCandidateSource::ExecutableRelative, path);
        }
    }

    fn push_candidate(
        candidates: &mut Vec<AutomaticCandidate>,
        source: AutomaticCandidateSource,
        path: PathBuf,
    ) {
        if path.as_os_str().is_empty() || candidates.iter().any(|candidate| candidate.path == path)
        {
            return;
        }
        candidates.push(AutomaticCandidate { source, path });
    }
}

fn exe_relative_bridge_candidates(current_exe: &Path) -> Vec<PathBuf> {
    let current_exe = canonicalize_executable_path(current_exe);
    let mut candidates = Vec::new();

    for ancestor in current_exe.ancestors().skip(1).take(MAX_BRIDGE_EXE_ANCESTORS) {
        push_unique_path(&mut candidates, ancestor.join(BRIDGE_SCRIPT_RELATIVE_PATH));
        if is_node_modules_dir(ancestor) {
            push_unique_path(
                &mut candidates,
                ancestor.join(ROOT_NPM_PACKAGE_NAME).join(BRIDGE_SCRIPT_RELATIVE_PATH),
            );
        }
    }

    candidates
}

fn bundled_bun_runtime_candidates(current_exe: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(current_exe) = current_exe {
        let current_exe = canonicalize_executable_path(current_exe);
        for ancestor in current_exe.ancestors().skip(1).take(MAX_BRIDGE_EXE_ANCESTORS) {
            push_unique_path(&mut candidates, ancestor.join(BUNDLED_BUN_RUNTIME_FILE_NAME));
        }
    }

    candidates
}

fn runtime_override_from_env() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        std::env::var_os(BRIDGE_RUNTIME_ENV_VAR).map(PathBuf::from)
    } else {
        None
    }
}

fn current_exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok().map(|path| canonicalize_executable_path(&path))
}

fn canonicalize_executable_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn push_unique_path(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() || candidates.iter().any(|candidate| candidate == &path) {
        return;
    }
    candidates.push(path);
}

fn manifest_root_from_script(manifest_script: &Path) -> Option<&Path> {
    manifest_script.parent()?.parent()?.parent()
}

fn is_node_modules_dir(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "node_modules")
}

fn is_automatic_script_candidate(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.is_file()
}

fn is_automatic_runtime_candidate(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.is_file()
}

#[cfg(test)]
mod tests {
    use super::{
        AutomaticCandidateSource, BRIDGE_SCRIPT_RELATIVE_PATH, BUNDLED_BUN_RUNTIME_FILE_NAME,
        BridgeLauncher, BridgeRuntimeKind, BridgeScriptResolver, canonicalize_executable_path,
        exe_relative_bridge_candidates, inspect_bridge_runtime_with, resolve_bridge_launcher,
        resolve_bridge_launcher_with_runtime, resolve_bridge_runtime_path_with,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[test]
    fn explicit_missing_script_path_reports_script_error() {
        let err = resolve_bridge_launcher(Some(Path::new("agent-sdk/dist/missing.mjs")))
            .expect_err("missing script should fail");
        assert!(
            err.to_string().contains("bridge script does not exist"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn explicit_script_path_builds_launcher_with_supplied_runtime() {
        let fixture = runtime_fixture().expect("runtime fixture");
        let launcher = resolve_bridge_launcher_with_runtime(
            fixture.runtime_path.clone(),
            Some(&fixture.script_path),
        )
        .expect("launcher");

        assert_eq!(
            launcher,
            BridgeLauncher {
                runtime_path: fixture.runtime_path.clone(),
                script_path: fixture.script_path.clone(),
            }
        );
        assert_eq!(
            launcher.describe(),
            format!(
                "{} {}",
                fixture.runtime_path.to_string_lossy(),
                fixture.script_path.to_string_lossy()
            )
        );
    }

    #[test]
    fn bridge_process_is_killed_if_its_owner_is_dropped() {
        let launcher = BridgeLauncher {
            runtime_path: PathBuf::from("bun"),
            script_path: PathBuf::from("bridge.js"),
        };

        assert!(launcher.command(false).get_kill_on_drop());
    }

    #[tokio::test]
    async fn command_runs_script_with_diagnostics_disabled() {
        let fixture = runtime_fixture().expect("runtime fixture");
        let launcher = BridgeLauncher {
            runtime_path: fixture.runtime_path,
            script_path: fixture.script_path.clone(),
        };

        let output = launcher.command(false).output().await.expect("spawn test runtime");
        assert!(output.status.success(), "child failed: {output:?}");

        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        assert!(stdout.contains(&format!("script={}", fixture.script_path.display())));
        assert!(stdout.contains("diag=0"));
    }

    #[tokio::test]
    async fn command_runs_script_with_diagnostics_enabled() {
        let fixture = runtime_fixture().expect("runtime fixture");
        let launcher = BridgeLauncher {
            runtime_path: fixture.runtime_path,
            script_path: fixture.script_path.clone(),
        };

        let output = launcher.command(true).output().await.expect("spawn test runtime");
        assert!(output.status.success(), "child failed: {output:?}");

        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
        assert!(stdout.contains(&format!("script={}", fixture.script_path.display())));
        assert!(stdout.contains("diag=1"));
        assert!(stderr.contains("diagnostics-stderr"));
    }

    #[test]
    fn explicit_missing_script_path_short_circuits_before_fallbacks() {
        let fixture = resolver_fixture();
        let missing = fixture.dir.path().join("missing.js");

        let err = BridgeScriptResolver {
            explicit_script: Some(&missing),
            env_script: Some(fixture.env_script.clone()),
            current_exe: Some(fixture.installed_exe.clone()),
            allow_dev_fallbacks: true,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect_err("explicit missing path should fail");

        assert!(
            err.to_string().contains("bridge script does not exist"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn env_override_wins_over_automatic_lookup() {
        let fixture = resolver_fixture();

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: Some(fixture.env_script.clone()),
            current_exe: Some(fixture.installed_exe.clone()),
            allow_dev_fallbacks: false,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("env override should resolve");

        assert_eq!(resolved, fixture.env_script);
    }

    #[test]
    fn packaged_bridge_precedence_beats_cwd_bridge() {
        let fixture = resolver_fixture();

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.installed_exe.clone()),
            allow_dev_fallbacks: true,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("packaged bridge should resolve");

        assert_eq!(resolved, fixture.packaged_bridge);
    }

    #[test]
    fn debug_build_falls_back_to_cwd_bridge() {
        let fixture = resolver_fixture();

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.unbundled_exe.clone()),
            allow_dev_fallbacks: true,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("cwd fallback should resolve");

        assert_eq!(resolved, fixture.cwd_script);
    }

    #[test]
    fn debug_build_falls_back_to_manifest_bridge_after_cwd() {
        let fixture = resolver_fixture();
        let missing_cwd = fixture.dir.path().join("missing-cwd").join(BRIDGE_SCRIPT_RELATIVE_PATH);

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.unbundled_exe.clone()),
            allow_dev_fallbacks: true,
            cwd_script: missing_cwd,
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("manifest fallback should resolve");

        assert_eq!(resolved, fixture.manifest_script);
    }

    #[test]
    fn cargo_run_prefers_manifest_bridge_over_target_bundle() {
        let fixture = resolver_fixture();

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.cargo_target_exe.clone()),
            allow_dev_fallbacks: true,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("cargo-run resolver should prefer manifest bridge");

        assert_eq!(resolved, fixture.manifest_script);
        assert_ne!(resolved, fixture.cargo_target_bridge);
    }

    #[test]
    fn cargo_run_candidate_order_prefers_manifest_then_cwd_before_executable_relative() {
        let fixture = resolver_fixture();

        let candidates = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.cargo_target_exe.clone()),
            allow_dev_fallbacks: true,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .automatic_candidates();

        assert_eq!(candidates[0].source, AutomaticCandidateSource::ManifestProject);
        assert_eq!(candidates[0].path, fixture.manifest_script);
        assert_eq!(candidates[1].source, AutomaticCandidateSource::WorkingDirectory);
        assert_eq!(candidates[1].path, fixture.cwd_script);
    }

    #[test]
    fn installed_candidate_order_prefers_executable_relative_before_dev_fallbacks() {
        let fixture = resolver_fixture();

        let candidates = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.installed_exe.clone()),
            allow_dev_fallbacks: true,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .automatic_candidates();

        assert_eq!(candidates[0].source, AutomaticCandidateSource::ExecutableRelative);
        assert_eq!(
            candidates[0].path,
            fixture.installed_exe.parent().expect("exe parent").join(BRIDGE_SCRIPT_RELATIVE_PATH)
        );
    }

    #[test]
    fn release_mode_does_not_use_dev_fallbacks() {
        let fixture = resolver_fixture();

        let err = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.unbundled_exe.clone()),
            allow_dev_fallbacks: false,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect_err("release resolver should reject repo-local fallbacks");

        assert!(
            err.to_string().contains("bridge script not found near the installed executable"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn missing_current_exe_still_allows_debug_fallbacks() {
        let fixture = resolver_fixture();

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: None,
            allow_dev_fallbacks: true,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("debug fallback should resolve without current_exe");

        assert_eq!(resolved, fixture.cwd_script);
    }

    #[test]
    fn missing_current_exe_in_release_mode_does_not_enable_repo_fallbacks() {
        let fixture = resolver_fixture();

        let err = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: None,
            allow_dev_fallbacks: false,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect_err("release resolver should fail without bundled bridge");

        assert!(
            err.to_string().contains("bridge script not found near the installed executable"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn executable_relative_candidates_walk_up_to_package_root() {
        let fixture = resolver_fixture();
        let candidates = exe_relative_bridge_candidates(&fixture.installed_exe);

        assert!(candidates.contains(&fixture.packaged_bridge));
        assert_eq!(
            candidates[0],
            fixture.installed_exe.parent().expect("exe parent").join(BRIDGE_SCRIPT_RELATIVE_PATH)
        );
    }

    #[test]
    fn sibling_npm_package_bridge_resolves_without_env_override() {
        let fixture = resolver_fixture();

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(fixture.npm_platform_exe.clone()),
            allow_dev_fallbacks: false,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("sibling npm root bridge should resolve");

        assert_eq!(resolved, fixture.npm_sibling_bridge);
    }

    #[test]
    fn executable_relative_candidates_include_sibling_npm_root_package() {
        let fixture = resolver_fixture();
        let candidates = exe_relative_bridge_candidates(&fixture.npm_platform_exe);

        assert!(candidates.contains(&fixture.npm_sibling_bridge));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_current_exe_resolves_before_bridge_candidate_walk() {
        let fixture = resolver_fixture();
        let shim = fixture.dir.path().join("global-bin").join("claude-rs");
        fs::create_dir_all(shim.parent().expect("shim parent")).expect("create shim parent");
        std::os::unix::fs::symlink(&fixture.npm_platform_exe, &shim).expect("create symlink");

        let resolved = BridgeScriptResolver {
            explicit_script: None,
            env_script: None,
            current_exe: Some(canonicalize_executable_path(&shim)),
            allow_dev_fallbacks: false,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        }
        .resolve()
        .expect("canonicalized symlink should resolve sibling bridge");

        assert_eq!(resolved, fixture.npm_sibling_bridge);
    }

    #[test]
    fn bundled_bun_runtime_next_to_installed_exe_wins_over_path_bun() {
        let fixture = resolver_fixture();

        let resolved =
            resolve_bridge_runtime_path_with(None, Some(&fixture.installed_exe), true, || {
                Ok(fixture.path_bun_runtime.clone())
            })
            .expect("bundled bridge runtime should resolve");

        assert_eq!(resolved, fixture.packaged_runtime);
    }

    #[test]
    fn bridge_runtime_env_override_wins_over_packaged_runtime_in_dev_resolution() {
        let fixture = resolver_fixture();

        let resolved = resolve_bridge_runtime_path_with(
            Some(&fixture.env_runtime),
            Some(&fixture.installed_exe),
            true,
            || Ok(fixture.path_bun_runtime.clone()),
        )
        .expect("env bridge runtime should resolve");

        assert_eq!(resolved, fixture.env_runtime);
    }

    #[test]
    fn release_bridge_runtime_resolution_ignores_env_override() {
        let fixture = resolver_fixture();

        let resolved = resolve_bridge_runtime_path_with(
            Some(&fixture.env_runtime),
            Some(&fixture.installed_exe),
            false,
            || Ok(fixture.path_bun_runtime.clone()),
        )
        .expect("release resolution should use bundled runtime");

        assert_eq!(resolved, fixture.packaged_runtime);
    }

    #[test]
    fn bridge_runtime_inspection_reports_explicit_kind_for_dev_override() {
        let fixture = resolver_fixture();

        let inspection = inspect_bridge_runtime_with(
            Some(&fixture.env_runtime),
            Some(&fixture.installed_exe),
            true,
            || Ok(fixture.path_bun_runtime.clone()),
        );

        assert_eq!(inspection.resolved_path, Some(fixture.env_runtime));
        assert_eq!(inspection.resolved_kind, Some(BridgeRuntimeKind::Explicit));
        assert_eq!(inspection.path_bun, None);
        assert!(inspection.error.is_none());
    }

    #[test]
    fn path_bun_is_dev_fallback_when_bundled_runtime_is_missing() {
        let fixture = resolver_fixture();

        let resolved =
            resolve_bridge_runtime_path_with(None, Some(&fixture.unbundled_exe), true, || {
                Ok(fixture.path_bun_runtime.clone())
            })
            .expect("dev bun fallback should resolve");

        assert_eq!(resolved, fixture.path_bun_runtime);
    }

    #[test]
    fn release_bridge_runtime_resolution_fails_without_bundled_runtime() {
        let fixture = resolver_fixture();

        let err =
            resolve_bridge_runtime_path_with(None, Some(&fixture.unbundled_exe), false, || {
                Ok(fixture.path_bun_runtime.clone())
            })
            .expect_err("release resolution should not use PATH bun");

        assert!(
            err.to_string().contains("failed to resolve bundled Bun bridge runtime"),
            "unexpected error: {err:#}"
        );
    }

    struct RuntimeFixture {
        _dir: TempDir,
        runtime_path: PathBuf,
        script_path: PathBuf,
    }

    struct ResolverFixture {
        dir: TempDir,
        installed_exe: PathBuf,
        npm_platform_exe: PathBuf,
        unbundled_exe: PathBuf,
        cargo_target_exe: PathBuf,
        packaged_bridge: PathBuf,
        npm_sibling_bridge: PathBuf,
        packaged_runtime: PathBuf,
        cargo_target_bridge: PathBuf,
        cwd_script: PathBuf,
        manifest_script: PathBuf,
        env_script: PathBuf,
        env_runtime: PathBuf,
        path_bun_runtime: PathBuf,
    }

    fn runtime_fixture() -> std::io::Result<RuntimeFixture> {
        let dir = tempfile::tempdir()?;
        let runtime_path = dir.path().join(test_runtime_name());
        let script_path = dir.path().join(test_bridge_script_name());
        fs::write(&runtime_path, test_runtime_contents())?;
        fs::write(&script_path, "// bridge test fixture\n")?;
        make_executable(&runtime_path)?;

        Ok(RuntimeFixture { _dir: dir, runtime_path, script_path })
    }

    fn resolver_fixture() -> ResolverFixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let installed_exe =
            dir.path().join("package").join("vendor").join("x86_64").join("claude-rs");
        let npm_platform_exe = dir
            .path()
            .join("npm-prefix")
            .join("node_modules")
            .join("@srothgan")
            .join("claude-code-rust-win32-x64-msvc")
            .join("bin")
            .join("claude-rs");
        let unbundled_exe =
            dir.path().join("other").join("vendor").join("x86_64").join("claude-rs");
        let cargo_target_exe =
            dir.path().join("manifest").join("target").join("debug").join("claude-rs");
        let packaged_bridge = dir.path().join("package").join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let npm_sibling_bridge = dir
            .path()
            .join("npm-prefix")
            .join("node_modules")
            .join("claude-code-rust")
            .join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let packaged_runtime = installed_exe
            .parent()
            .expect("installed exe parent")
            .join(BUNDLED_BUN_RUNTIME_FILE_NAME);
        let cargo_target_bridge =
            dir.path().join("manifest").join("target").join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let cwd_script = dir.path().join("repo").join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let manifest_script = dir.path().join("manifest").join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let env_script = dir.path().join("env").join("bridge.js");
        let env_runtime = dir.path().join("env").join(BUNDLED_BUN_RUNTIME_FILE_NAME);
        let path_bun_runtime = dir.path().join("bun").join(test_runtime_name());

        write_test_file(&installed_exe);
        write_test_file(&npm_platform_exe);
        write_test_file(&unbundled_exe);
        write_test_file(&cargo_target_exe);
        write_test_file(&packaged_bridge);
        write_test_file(&npm_sibling_bridge);
        write_test_file(&packaged_runtime);
        write_test_file(&cargo_target_bridge);
        write_test_file(&cwd_script);
        write_test_file(&manifest_script);
        write_test_file(&env_script);
        write_test_file(&env_runtime);
        write_test_file(&path_bun_runtime);

        let installed_exe = canonicalize_executable_path(&installed_exe);
        let npm_platform_exe = canonicalize_executable_path(&npm_platform_exe);
        let unbundled_exe = canonicalize_executable_path(&unbundled_exe);
        let cargo_target_exe = canonicalize_executable_path(&cargo_target_exe);
        let packaged_bridge = canonicalize_executable_path(&packaged_bridge);
        let npm_sibling_bridge = canonicalize_executable_path(&npm_sibling_bridge);
        let packaged_runtime = canonicalize_executable_path(&packaged_runtime);
        let cargo_target_bridge = canonicalize_executable_path(&cargo_target_bridge);
        let cwd_script = canonicalize_executable_path(&cwd_script);
        let manifest_script = canonicalize_executable_path(&manifest_script);
        let env_script = canonicalize_executable_path(&env_script);
        let env_runtime = canonicalize_executable_path(&env_runtime);
        let path_bun_runtime = canonicalize_executable_path(&path_bun_runtime);

        ResolverFixture {
            dir,
            installed_exe,
            npm_platform_exe,
            unbundled_exe,
            cargo_target_exe,
            packaged_bridge,
            npm_sibling_bridge,
            packaged_runtime,
            cargo_target_bridge,
            cwd_script,
            manifest_script,
            env_script,
            env_runtime,
            path_bun_runtime,
        }
    }

    fn write_test_file(path: &Path) {
        let parent = path.parent().expect("path parent");
        fs::create_dir_all(parent).expect("create parent directories");
        fs::write(path, "// bridge test fixture\n").expect("write fixture");
    }

    #[cfg(windows)]
    fn test_runtime_name() -> &'static str {
        "bridge_runtime_test.cmd"
    }

    #[cfg(not(windows))]
    fn test_runtime_name() -> &'static str {
        "bridge_runtime_test.sh"
    }

    #[cfg(windows)]
    fn test_bridge_script_name() -> &'static str {
        "bridge_target.js"
    }

    #[cfg(not(windows))]
    fn test_bridge_script_name() -> &'static str {
        "bridge_target.js"
    }

    #[cfg(windows)]
    fn test_runtime_contents() -> &'static str {
        "@echo off\r\necho script=%~f1\r\necho diag=%CLAUDE_RS_BRIDGE_DIAGNOSTICS%\r\necho diagnostics-stderr 1>&2\r\n"
    }

    #[cfg(not(windows))]
    fn test_runtime_contents() -> &'static str {
        "#!/bin/sh\nprintf 'script=%s\\n' \"$1\"\nprintf 'diag=%s\\n' \"$CLAUDE_RS_BRIDGE_DIAGNOSTICS\"\nprintf 'diagnostics-stderr\\n' >&2\n"
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
    }

    #[cfg(not(unix))]
    #[allow(clippy::unnecessary_wraps)]
    fn make_executable(_path: &Path) -> std::io::Result<()> {
        Ok(())
    }
}
