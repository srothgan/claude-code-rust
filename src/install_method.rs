// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;
use std::path::{Path, PathBuf};

const ROOT_PACKAGE_NAME: &str = "claude-code-rust";
const NPM_PLATFORM_PACKAGE_NAMES: [&str; 6] = [
    "@srothgan/claude-code-rust-darwin-arm64",
    "@srothgan/claude-code-rust-darwin-x64",
    "@srothgan/claude-code-rust-linux-arm64-gnu",
    "@srothgan/claude-code-rust-linux-x64-gnu",
    "@srothgan/claude-code-rust-win32-arm64-msvc",
    "@srothgan/claude-code-rust-win32-x64-msvc",
];

#[cfg(target_os = "windows")]
const BUNDLED_RUNTIME_NAME: &str = "claude-rs-bridge-bun.exe";
#[cfg(not(target_os = "windows"))]
const BUNDLED_RUNTIME_NAME: &str = "claude-rs-bridge-bun";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    Script { install_dir: Option<PathBuf> },
    Npm,
    Unknown,
}

impl InstallMethod {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Script { .. } => "install script",
            Self::Npm => "npm",
            Self::Unknown => "unknown",
        }
    }
}

#[must_use]
pub fn detect_install_method() -> InstallMethod {
    std::env::current_exe()
        .ok()
        .map_or(InstallMethod::Unknown, |current_exe| detect_install_method_for_exe(&current_exe))
}

fn detect_install_method_for_exe(current_exe: &Path) -> InstallMethod {
    let Some(exe_dir) = current_exe.parent() else {
        return InstallMethod::Unknown;
    };

    if is_script_install_root(exe_dir) {
        return InstallMethod::Script { install_dir: Some(exe_dir.to_owned()) };
    }

    if exe_dir.file_name().is_some_and(|name| name == "bin")
        && let Some(package_root) = exe_dir.parent()
        && package_name(package_root)
            .is_some_and(|name| NPM_PLATFORM_PACKAGE_NAMES.contains(&name.as_str()))
    {
        return InstallMethod::Npm;
    }

    InstallMethod::Unknown
}

fn is_script_install_root(root: &Path) -> bool {
    package_name(root).as_deref() == Some(ROOT_PACKAGE_NAME)
        && root.join(BUNDLED_RUNTIME_NAME).is_file()
        && root.join("agent-sdk").join("dist").join("bridge.js").is_file()
}

fn package_name(root: &Path) -> Option<String> {
    #[derive(Deserialize)]
    struct PackageIdentity {
        name: String,
    }

    let raw = std::fs::read_to_string(root.join("package.json")).ok()?;
    serde_json::from_str::<PackageIdentity>(&raw).ok().map(|package| package.name)
}

#[cfg(test)]
#[path = "install_method_audit_tests.rs"]
mod audit_tests;
