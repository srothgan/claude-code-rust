// SPDX-License-Identifier: Apache-2.0

use crate::logging::DiagnosticsPaths;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::process::Command;

pub(crate) fn paths(root: &Path) -> DiagnosticsPaths {
    DiagnosticsPaths {
        root_dir: root.into(),
        runtime_dir: root.join("runtime"),
        legacy_log_path: root.join("claude-rs.log"),
    }
}

#[allow(clippy::unwrap_used)]
pub(crate) fn command(test: &str, root: &Path, scenario: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("CLAUDE_RS_AUDIT_ROOT", root)
        .env("CLAUDE_RS_AUDIT_CASE", scenario)
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) async fn run(mut command: Command) -> Output {
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("bounded diagnostics worker")
        .unwrap()
}
