// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::fs;

#[test]
fn runtime_version_classification_associates_normalized_output_and_failure_with_its_runtime() {
    let runtime = Path::new("selected runtime/bun");
    for (input, expected) in [
        ("1.3.14", Some("1.3.14")),
        ("v1.3.14", Some("1.3.14")),
        ("1.3.14-canary.1", Some("1.3.14-canary.1")),
        ("v1.3.14+20260705", Some("1.3.14+20260705")),
        (" \t v2.1.9 \n", Some("2.1.9")),
        ("not-bun", None),
        ("", None),
        ("1.3", None),
        ("1.3.14-", None),
        ("1.3.14+", None),
    ] {
        let check = classify_bun_version(input, runtime);
        assert_eq!(check.details["runtime"], runtime.display().to_string());
        assert_eq!(check.hard_failure, expected.is_none());
        if let Some(version) = expected {
            assert_eq!(check.status, DoctorStatus::Pass);
            assert_eq!(check.details["version"], version);
            assert!(check.message.contains(version));
        } else {
            assert_eq!(check.status, DoctorStatus::Fail);
            assert!(!check.details.contains_key("version"));
            assert!(check.message.contains("parse") && check.message.contains(input));
        }
    }
}

fn report(checks: Vec<DoctorCheck>) -> String {
    let report = DoctorReport {
        version: "9.8.7".into(),
        platform: PlatformReport { os: "fixture".into(), arch: "fixture".into() },
        checks,
    };
    let mut output = Vec::new();
    write_human_report(&mut output, &report, HumanStyle::new(false)).unwrap();
    String::from_utf8(output).unwrap()
}

fn section<'a>(output: &'a str, heading: &str) -> &'a str {
    let tail = output.split_once(&format!("\n{heading}\n")).unwrap().1;
    let end = tail
        .lines()
        .scan(0, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find(|(_, line)| !line.is_empty() && !line.starts_with(' '))
        .map_or(tail.len(), |(offset, _)| offset);
    &tail[..end]
}

#[test]
fn human_report_keeps_configuration_status_and_paths_in_the_configuration_section() {
    let checks = [
        ("config_settings", "Global settings", "global/settings.json", DoctorStatus::Pass),
        (
            "config_project_settings",
            "Project settings",
            "selected/.claude/settings.json",
            DoctorStatus::Warn,
        ),
        (
            "config_local_settings",
            "Local settings",
            "selected/.claude/settings.local.json",
            DoctorStatus::Fail,
        ),
        ("config_preferences", "Preferences", "profile/.claude.json", DoctorStatus::Pass),
    ];
    let text = report(
        checks
            .iter()
            .map(|(id, label, path, status)| {
                DoctorCheck::new(
                    id,
                    label,
                    *status,
                    format!("cause for {label}"),
                    *status == DoctorStatus::Fail,
                    BTreeMap::from([("path".into(), (*path).into())]),
                )
            })
            .collect(),
    );
    let config = section(&text, "Configuration");
    for (_, label, path, status) in checks {
        let block = config.split("\n\n").find(|block| block.contains(label)).unwrap();
        let expected = match status {
            DoctorStatus::Pass => "[PASS]",
            DoctorStatus::Warn => "[WARN]",
            DoctorStatus::Fail => "[FAIL]",
        };
        assert!(
            block.contains(expected)
                && block.contains(&format!("cause for {label}"))
                && block.contains(path),
            "{block}"
        );
    }
}

#[test]
fn human_candidates_preserve_sources_states_and_rejected_package_metadata() {
    use crate::agent::bridge::BridgeCandidateInspection;
    let dir = tempfile::tempdir().unwrap();
    let wrong = dir.path().join("wrong (package).json");
    fs::write(&wrong, r#"{"name":"unrelated-package","version":"0.0.1"}"#).unwrap();
    let package = package_json_check(
        "npm_root_package",
        "npm root package",
        vec![wrong.clone()],
        Some("claude-code-rust"),
        false,
    );
    let script = bridge_script_check(&BridgeScriptInspection {
        explicit_path: None,
        env_path: None,
        resolved_path: None,
        error: Some("selected bridge unavailable".into()),
        candidates: vec![
            BridgeCandidateInspection {
                source: "package".into(),
                path: PathBuf::from(r"C:\package (selected)\bridge.js"),
                is_file: false,
            },
            BridgeCandidateInspection {
                source: "working-directory".into(),
                path: dir.path().join("missing bridge.js"),
                is_file: false,
            },
        ],
    });
    let version = classify_bun_version("v3.2.1", Path::new("chosen runtime"));
    let text = report(vec![script, version, package]);
    let runtime = section(&text, "Runtime prerequisites");
    let script_block = runtime.split("\n\n").find(|block| block.contains("Bridge script")).unwrap();
    assert!(
        script_block.contains("[FAIL]") && script_block.contains("selected bridge unavailable")
    );
    for (source, path) in [
        ("package", r"C:\package (selected)\bridge.js".to_owned()),
        ("working-directory", dir.path().join("missing bridge.js").display().to_string()),
    ] {
        let row = script_block.lines().find(|line| line.contains(&path)).unwrap();
        assert!(row.contains(source) && row.contains("missing"), "{row}");
    }
    let version =
        runtime.split("\n\n").find(|block| block.contains("Bridge runtime version")).unwrap();
    assert!(
        version.contains("[PASS]")
            && version.contains("3.2.1")
            && version.contains("chosen runtime")
    );
    let npm = section(&text, "npm installation");
    assert!(npm.contains("[WARN]") && npm.contains("expected claude-code-rust"));
    let metadata = npm
        .lines()
        .find(|line| line.contains("candidate_0_name:"))
        .expect("rejected identity remains metadata");
    assert!(metadata.contains("unrelated-package"));
    let candidate_table = npm.split_once("Candidates:").unwrap().1;
    assert!(
        candidate_table
            .lines()
            .any(|line| line.contains(&wrong.display().to_string()) && line.contains("file"))
    );
    assert!(!candidate_table.contains("unrelated-package"));
}

#[test]
fn package_discovery_reports_selected_identity_and_useful_optional_failures() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    let unrelated = dir.path().join("other.json");
    let selected = dir.path().join("selected.json");
    let malformed = dir.path().join("malformed.json");
    fs::write(&unrelated, r#"{"name":"other-tool","version":"1.0.0"}"#).unwrap();
    fs::write(&selected, r#"{"name":"claude-code-rust","version":"7.8.9"}"#).unwrap();
    fs::write(&malformed, "{broken").unwrap();
    let check = package_json_check(
        "npm_root_package",
        "npm root package",
        vec![missing.clone(), unrelated.clone(), selected.clone()],
        Some("claude-code-rust"),
        false,
    );
    assert_eq!(check.status, DoctorStatus::Pass);
    assert!(!check.hard_failure);
    assert_eq!(check.details["path"], selected.display().to_string());
    assert_eq!(check.details["name"], "claude-code-rust");
    assert_eq!(check.details["version"], "7.8.9");
    assert_eq!(check.details["candidate_1_name"], "other-tool");
    assert!(check.details["candidate_1"].contains(&unrelated.display().to_string()));
    for (path, cause) in
        [(missing, "not discoverable"), (unrelated, "other-tool"), (malformed, "not valid JSON")]
    {
        let check = package_json_check(
            "npm_root_package",
            "npm root package",
            vec![path.clone()],
            Some("claude-code-rust"),
            false,
        );
        assert_eq!(check.status, DoctorStatus::Warn);
        assert!(!check.hard_failure);
        assert!(check.message.contains(cause), "{check:?}");
        assert!(check.details["candidate_0"].contains(&path.display().to_string()));
        assert!(!check.details.contains_key("path") && !check.details.contains_key("version"));
    }
}

#[test]
fn runtime_candidates_show_the_selected_path_and_actual_file_states() {
    use crate::agent::bridge::{BridgeCandidateInspection, BridgeRuntimeKind};
    let directory = tempfile::tempdir().unwrap();
    let selected = directory.path().join("selected (runtime)");
    let missing = directory.path().join("missing runtime");
    fs::write(&selected, "fixture").unwrap();
    let check = bridge_runtime_check(&BridgeRuntimeInspection {
        env_path: None,
        path_bun: None,
        error: None,
        resolved_path: Some(selected.clone()),
        resolved_kind: Some(BridgeRuntimeKind::BundledBun),
        packaged_candidates: vec![
            BridgeCandidateInspection {
                source: "package".into(),
                path: selected.clone(),
                is_file: true,
            },
            BridgeCandidateInspection {
                source: "ancestor".into(),
                path: missing.clone(),
                is_file: false,
            },
        ],
    });
    let text = report(vec![check]);
    let runtime = section(&text, "Runtime prerequisites");
    let status = runtime.lines().find(|line| line.contains("Bridge runtime")).unwrap();
    assert!(status.contains("[PASS]") && status.contains(&selected.display().to_string()));
    assert!(runtime.contains("runtime_kind: bundled_bun"));
    let candidates = runtime.split_once("Candidates:").unwrap().1;
    for (path, state) in [(selected, "file"), (missing, "missing")] {
        let row =
            candidates.lines().find(|line| line.contains(&path.display().to_string())).unwrap();
        assert!(row.contains(state), "{row}");
    }
}

#[test]
fn platform_selection_matches_published_packages_and_refuses_unsupported_hosts() {
    for (os, arch, libc, expected) in [
        ("macos", "x86_64", None, "@srothgan/claude-code-rust-darwin-x64"),
        ("macos", "aarch64", None, "@srothgan/claude-code-rust-darwin-arm64"),
        ("windows", "x86_64", None, "@srothgan/claude-code-rust-win32-x64-msvc"),
        ("windows", "aarch64", None, "@srothgan/claude-code-rust-win32-arm64-msvc"),
        ("linux", "x86_64", Some("glibc"), "@srothgan/claude-code-rust-linux-x64-gnu"),
        ("linux", "aarch64", Some("glibc"), "@srothgan/claude-code-rust-linux-arm64-gnu"),
    ] {
        assert_eq!(
            platform_package_selection_for(os, arch, libc),
            PlatformPackageSelection::Supported(expected)
        );
    }
    for arch in ["x86_64", "aarch64"] {
        for libc in [None, Some("musl")] {
            assert_eq!(
                platform_package_selection_for("linux", arch, libc),
                PlatformPackageSelection::UnsupportedLinuxLibc {
                    arch,
                    libc: libc.unwrap_or("unknown")
                }
            );
        }
    }
    for (os, arch) in
        [("freebsd", "x86_64"), ("linux", "riscv64"), ("windows", "x86"), ("macos", "powerpc")]
    {
        assert_eq!(
            platform_package_selection_for(os, arch, Some("glibc")),
            PlatformPackageSelection::UnsupportedPlatform
        );
    }
}

#[test]
fn selected_runtime_probe_records_exact_arguments_and_distinguishes_execution_failures() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("probe.rs");
    fs::write(&source, include_str!("../../tests/fixtures/doctor_probe.rs")).unwrap();
    let runtime =
        dir.path().join(if cfg!(windows) { "selected probe.exe" } else { "selected probe" });
    let compiled = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&runtime)
        .output()
        .unwrap();
    assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
    for (mode, status, cause) in [
        ("valid", DoctorStatus::Pass, "4.5.6"),
        ("nonzero", DoctorStatus::Fail, "failed to run"),
        ("malformed", DoctorStatus::Fail, "parse"),
        ("empty", DoctorStatus::Fail, "parse"),
    ] {
        fs::write(dir.path().join("mode"), mode).unwrap();
        let check = bridge_runtime_version_check(Some(&runtime));
        assert_eq!(fs::read_to_string(dir.path().join("argv")).unwrap(), "--version\n");
        fs::remove_file(dir.path().join("argv")).unwrap();
        assert_eq!(check.status, status, "{check:?}");
        assert_eq!(check.hard_failure, status == DoctorStatus::Fail);
        assert!(check.message.contains(cause));
        let serialized = serde_json::to_string(&check).unwrap();
        assert!(serialized.contains(&runtime.file_name().unwrap().to_string_lossy().to_string()));
        if status == DoctorStatus::Pass {
            assert_eq!(check.details["version"], "4.5.6");
        }
    }
    let absent = dir.path().join("missing executable");
    let failed = bridge_runtime_version_check(Some(&absent));
    assert_eq!(failed.status, DoctorStatus::Fail);
    assert!(failed.hard_failure);
    assert!(
        failed.message.contains("failed to run") && failed.message.contains("missing executable")
    );
    let missing = bridge_runtime_version_check(None);
    assert_eq!(missing.status, DoctorStatus::Fail);
    assert!(missing.hard_failure);
    assert!(missing.message.contains("no runtime"));
    assert!(!dir.path().join("argv").exists());
}
