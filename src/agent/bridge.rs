// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::error::AppError;
use anyhow::Context as _;
use std::path::{Path, PathBuf};
use tokio::process::Command;

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
    if let Some(path) = explicit_script {
        return validate_script_path(path);
    }

    if let Some(path) = std::env::var_os("CLAUDE_RS_AGENT_BRIDGE") {
        return validate_script_path(Path::new(&path));
    }

    let mut candidates = vec![
        PathBuf::from("agent-sdk/dist/bridge.js"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("agent-sdk/dist/bridge.js"),
    ];

    if let Ok(current_exe) = std::env::current_exe() {
        for ancestor in current_exe.ancestors().skip(1).take(8) {
            candidates.push(ancestor.join("agent-sdk/dist/bridge.js"));
        }
    }

    for candidate in candidates {
        if !candidate.as_os_str().is_empty() && candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(anyhow::anyhow!(
        "bridge script not found. expected `agent-sdk/dist/bridge.js` or set CLAUDE_RS_AGENT_BRIDGE"
    ))
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

#[cfg(test)]
mod tests {
    use super::{BridgeLauncher, resolve_bridge_launcher, resolve_bridge_launcher_with_runtime};
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

    struct RuntimeFixture {
        _dir: TempDir,
        runtime_path: PathBuf,
        script_path: PathBuf,
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
