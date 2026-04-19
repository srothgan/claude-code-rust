// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::error::AppError;
use anyhow::Context as _;
use std::path::{Path, PathBuf};
use tokio::process::Command;

const BRIDGE_SCRIPT_RELATIVE_PATH: &str = "agent-sdk/dist/bridge.js";
const MAX_BRIDGE_EXE_ANCESTORS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeLauncher {
    pub runtime_path: PathBuf,
    pub script_path: PathBuf,
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
    let runtime = which::which("node")
        .map_err(|_| anyhow::Error::new(AppError::NodeNotFound))
        .context("failed to resolve `node` runtime")?;
    Ok(BridgeLauncher { runtime_path: runtime, script_path: script })
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

fn validate_script_path(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.exists() {
        return Err(anyhow::anyhow!("bridge script does not exist: {}", path.display()));
    }
    if !path.is_file() {
        return Err(anyhow::anyhow!("bridge script is not a file: {}", path.display()));
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

impl<'a> BridgeScriptResolver<'a> {
    fn from_process(explicit_script: Option<&'a Path>) -> Self {
        Self {
            explicit_script,
            env_script: std::env::var_os("CLAUDE_RS_AGENT_BRIDGE").map(PathBuf::from),
            current_exe: std::env::current_exe().ok(),
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

        for candidate in self.automatic_candidates() {
            if is_automatic_script_candidate(&candidate) {
                return Ok(candidate);
            }
        }

        Err(anyhow::anyhow!(
            "bridge script not found near the installed executable. expected bundled `agent-sdk/dist/bridge.js`; debug builds also check repo-local fallbacks. set CLAUDE_RS_AGENT_BRIDGE to override."
        ))
    }

    fn automatic_candidates(&self) -> Vec<PathBuf> {
        let mut candidates = Vec::new();

        if let Some(current_exe) = self.current_exe.as_deref() {
            candidates.extend(exe_relative_bridge_candidates(current_exe));
        }

        if self.allow_dev_fallbacks {
            candidates.push(self.cwd_script.clone());
            candidates.push(self.manifest_script.clone());
        }

        candidates
    }
}

fn exe_relative_bridge_candidates(current_exe: &Path) -> Vec<PathBuf> {
    current_exe
        .ancestors()
        .skip(1)
        .take(MAX_BRIDGE_EXE_ANCESTORS)
        .map(|ancestor| ancestor.join(BRIDGE_SCRIPT_RELATIVE_PATH))
        .collect()
}

fn is_automatic_script_candidate(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.is_file()
}

#[cfg(test)]
mod tests {
    use super::{
        BRIDGE_SCRIPT_RELATIVE_PATH, BridgeLauncher, BridgeScriptResolver,
        exe_relative_bridge_candidates, resolve_bridge_launcher,
        resolve_bridge_launcher_with_runtime,
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

    struct RuntimeFixture {
        _dir: TempDir,
        runtime_path: PathBuf,
        script_path: PathBuf,
    }

    struct ResolverFixture {
        dir: TempDir,
        installed_exe: PathBuf,
        unbundled_exe: PathBuf,
        packaged_bridge: PathBuf,
        cwd_script: PathBuf,
        manifest_script: PathBuf,
        env_script: PathBuf,
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
        let unbundled_exe =
            dir.path().join("other").join("vendor").join("x86_64").join("claude-rs");
        let packaged_bridge = dir.path().join("package").join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let cwd_script = dir.path().join("repo").join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let manifest_script = dir.path().join("manifest").join(BRIDGE_SCRIPT_RELATIVE_PATH);
        let env_script = dir.path().join("env").join("bridge.js");

        write_test_file(&installed_exe);
        write_test_file(&unbundled_exe);
        write_test_file(&packaged_bridge);
        write_test_file(&cwd_script);
        write_test_file(&manifest_script);
        write_test_file(&env_script);

        ResolverFixture {
            dir,
            installed_exe,
            unbundled_exe,
            packaged_bridge,
            cwd_script,
            manifest_script,
            env_script,
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
