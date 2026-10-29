// SPDX-License-Identifier: Apache-2.0
use super::*;
fn write(path: &Path, bytes: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
#[test]
fn archive_ownership_requires_each_file_and_the_root_identity() {
    #[cfg(windows)]
    let runtime = "claude-rs-bridge-bun.exe";
    #[cfg(not(windows))]
    let runtime = "claude-rs-bridge-bun";
    for defect in [
        "none",
        "runtime-missing",
        "runtime-directory",
        "bridge-missing",
        "bridge-directory",
        "package-missing",
        "package-malformed",
        "package-wrong",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("selected install root");
        let exe = root.join("claude-rs");
        write(&exe, "binary");
        for (name, prefix) in [(runtime, "runtime"), ("agent-sdk/dist/bridge.js", "bridge")] {
            if defect == format!("{prefix}-missing") {
                continue;
            }
            if defect == format!("{prefix}-directory") {
                std::fs::create_dir_all(root.join(name)).unwrap();
            } else {
                write(&root.join(name), "artifact");
            }
        }
        match defect {
            "package-missing" => {}
            "package-malformed" => write(&root.join("package.json"), "{broken"),
            "package-wrong" => write(&root.join("package.json"), r#"{"name":"unowned"}"#),
            _ => write(&root.join("package.json"), r#"{"name":"claude-code-rust"}"#),
        }
        assert_eq!(
            detect_install_method_for_exe(&exe),
            if defect == "none" {
                InstallMethod::Script { install_dir: Some(root) }
            } else {
                InstallMethod::Unknown
            },
            "{defect}"
        );
    }
}
#[test]
fn npm_ownership_comes_from_supported_identity_and_bin_location() {
    for (identity, bin, expected) in [
        (Some(r#"{"name":"@srothgan/claude-code-rust-win32-x64-msvc"}"#), true, InstallMethod::Npm),
        (
            Some(r#"{"name":"@srothgan/claude-code-rust-linux-arm64-gnu"}"#),
            true,
            InstallMethod::Npm,
        ),
        (Some(r#"{"name":"@srothgan/claude-code-rust-darwin-arm64"}"#), true, InstallMethod::Npm),
        (Some(r#"{"name":"@srothgan/claude-code-rust-unofficial"}"#), true, InstallMethod::Unknown),
        (
            Some(r#"{"name":"@srothgan/claude-code-rust-win32-x64-msvc"}"#),
            false,
            InstallMethod::Unknown,
        ),
        (Some("{broken"), true, InstallMethod::Unknown),
        (None, true, InstallMethod::Unknown),
        (None, false, InstallMethod::Unknown),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("node_modules/@srothgan/claude-code-rust-win32-x64-msvc");
        let exe = root.join(if bin { "bin/claude-rs" } else { "claude-rs" });
        write(&exe, "binary");
        if let Some(identity) = identity {
            write(&root.join("package.json"), identity);
        }
        assert_eq!(detect_install_method_for_exe(&exe), expected, "{identity:?}, bin={bin}");
    }
}
