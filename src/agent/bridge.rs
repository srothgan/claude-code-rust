// SPDX-License-Identifier: Apache-2.0

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
        // Always captured: the bridge writes warnings and errors to stderr even when
        // detailed diagnostics are off, and the default log records them.
        cmd.stderr(std::process::Stdio::piped());
        cmd
    }
}

pub fn resolve_bridge_launcher(explicit_script: Option<&Path>) -> anyhow::Result<BridgeLauncher> {
    let script = resolve_bridge_script_path(explicit_script)?;
    let runtime = resolve_bridge_runtime_path()?;
    Ok(BridgeLauncher { runtime_path: runtime, script_path: script })
}

pub fn inspect_bridge_script(explicit_script: Option<&Path>) -> BridgeScriptInspection {
    BridgeScriptResolver::from_process(explicit_script).inspect()
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

/// Where an overriding bridge script path came from, which names it in a problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScriptOverride {
    Explicit,
    Env,
}

/// Why an explicit or environment bridge script cannot be run, or `None` when it can.
///
/// The one authority on script usability: startup validation, `doctor` and the launcher
/// all ask here. A path whose lookup fails for any reason but absence (a parent without
/// search permission) is inaccessible, not missing, and a file that cannot be opened is
/// unreadable, because the runtime would fail on it the same way.
fn script_override_problem(path: &Path, source: ScriptOverride) -> Option<String> {
    let label = match source {
        ScriptOverride::Explicit => "bridge script".to_owned(),
        ScriptOverride::Env => format!("bridge script from {BRIDGE_SCRIPT_ENV_VAR}"),
    };
    let shown = path.display();
    match std::fs::metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Some(format!("{label} does not exist: {shown}"))
        }
        Err(err) => Some(format!("{label} cannot be accessed: {shown}: {err}")),
        Ok(metadata) if !metadata.is_file() => Some(format!("{label} is not a file: {shown}")),
        Ok(_) => std::fs::File::open(path)
            .err()
            .map(|err| format!("{label} cannot be read: {shown}: {err}")),
    }
}

fn validate_script_path(path: &Path, source: ScriptOverride) -> anyhow::Result<PathBuf> {
    match script_override_problem(path, source) {
        Some(problem) => Err(anyhow::Error::new(AppError::BridgeSpawnFailed).context(problem)),
        None => Ok(path.to_path_buf()),
    }
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

    /// The resolver's choice as `doctor` reports it, with the same override checks as
    /// [`Self::resolve`].
    fn inspect(&self) -> BridgeScriptInspection {
        let explicit_path = self.explicit_script.map(Path::to_path_buf);

        let overridden = self
            .explicit_script
            .map(|path| (path, ScriptOverride::Explicit))
            .or_else(|| self.env_script.as_deref().map(|path| (path, ScriptOverride::Env)));
        if let Some((path, source)) = overridden {
            let error = script_override_problem(path, source);
            return BridgeScriptInspection {
                explicit_path,
                env_path: self.env_script.clone(),
                candidates: Vec::new(),
                resolved_path: error.is_none().then(|| path.to_path_buf()),
                error,
            };
        }

        let candidates = self
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
            env_path: self.env_script.clone(),
            error: resolved_path
                .is_none()
                .then(|| "bridge script not found near the installed executable".to_owned()),
            candidates,
            resolved_path,
        }
    }

    fn resolve(&self) -> anyhow::Result<PathBuf> {
        if let Some(path) = self.explicit_script {
            return validate_script_path(path, ScriptOverride::Explicit);
        }

        if let Some(path) = self.env_script.as_deref() {
            return validate_script_path(path, ScriptOverride::Env);
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
#[path = "bridge_audit_tests.rs"]
mod audit_tests;

#[cfg(test)]
mod tests {
    use super::audit_tests::resolver_fixture;
    use super::{BridgeScriptResolver, resolve_bridge_runtime_path_with};

    #[test]
    fn env_override_wins_over_automatic_lookup() {
        let fixture = resolver_fixture();
        let resolver = BridgeScriptResolver {
            explicit_script: None,
            env_script: Some(fixture.env_script.clone()),
            current_exe: Some(fixture.installed_exe.clone()),
            allow_dev_fallbacks: false,
            cwd_script: fixture.cwd_script.clone(),
            manifest_script: fixture.manifest_script.clone(),
        };

        let chosen = resolver.resolve().expect("env override should resolve");
        let inspection = resolver.inspect();

        assert_eq!(chosen, fixture.env_script);
        assert_eq!(inspection.resolved_path.as_ref(), Some(&fixture.env_script));
        assert_eq!(inspection.error, None);
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
}
