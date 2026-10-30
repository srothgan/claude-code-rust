// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::diagnostics_test_support::{command, paths, run};
use serde_json::{Value, json};
use std::fs;

#[allow(clippy::unwrap_used)]
fn field<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    text.lines().find_map(|line| line.trim().strip_prefix(&format!("{label}: ")))
}

#[test]
fn error_reports_associate_status_recovery_and_safe_detail_with_isolated_logs() {
    let temp = tempfile::tempdir().unwrap();
    let paths = paths(temp.path());
    fs::create_dir_all(&paths.runtime_dir).unwrap();
    let selected = paths.runtime_dir.join("claude-rs-20260101T000000Z-p1-rabc.log");
    for has_log in [false, true] {
        if has_log {
            fs::write(&selected, b"safe log").unwrap();
        }
        for (error, category, code, next) in [
            (AppError::BridgeTimeout, "bridge_timeout", "22", "claude-rs doctor --strict"),
            (
                AppError::BridgeInitializationFailed,
                "bridge_initialization",
                "22",
                "claude-rs logs --bundle --yes",
            ),
            (
                AppError::BridgeStdoutClosed,
                "bridge_stdout_closed",
                "22",
                "claude-rs logs --bundle --yes",
            ),
            (AppError::AuthRequired, "auth", "24", "claude auth login"),
            (AppError::SessionNotFound, "session", "23", "claude-rs resume"),
        ] {
            for detail in [None, Some("before api_key=raw-report-secret after")] {
                let mut output = Vec::new();
                write_app_error_report_with_paths(
                    &mut output,
                    &error,
                    detail,
                    || Ok(paths.clone()),
                )
                .unwrap();
                let text = String::from_utf8(output).unwrap();
                assert_eq!(field(&text, "category"), Some(category));
                assert_eq!(field(&text, "exit_code"), Some(code));
                assert_eq!(field(&text, "next_step"), Some(next));
                assert_eq!(field(&text, "version"), Some(env!("CARGO_PKG_VERSION")));
                assert_eq!(
                    field(&text, "platform"),
                    Some(format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH).as_str())
                );
                assert!(!field(&text, "message").unwrap().is_empty());
                assert_eq!(
                    field(&text, "detail"),
                    detail.map(|_| "before api_key=[redacted] after")
                );
                assert!(!text.contains("raw-report-secret"));
                assert_eq!(
                    field(&text, "latest_log"),
                    Some(if has_log { selected.to_str().unwrap() } else { "not found" })
                );
            }
        }
    }
    assert_eq!(fs::read(selected).unwrap(), b"safe log");
}

#[test]
fn panic_worker() {
    let Ok(scenario) = std::env::var("CLAUDE_RS_AUDIT_CASE") else { return };
    let root = PathBuf::from(std::env::var_os("CLAUDE_RS_AUDIT_ROOT").unwrap());
    let chosen = paths(&root);
    assert!(chosen.root_dir.is_absolute());
    let metadata = last_crash_metadata_path(&chosen);
    install_panic_hook_with_paths(move || Ok(chosen.clone()));
    match scenario.as_str() {
        "string" | "blocked" => panic!("safe panic before api_key=raw-panic-secret after"),
        "owned" => {
            panic::panic_any(String::from("owned panic before api_key=raw-panic-secret after"))
        }
        "nonstring" => panic::panic_any(17_u32),
        "suppressed" | "restored" => {
            let nested = catch_expected_panic(|| {
                let inner = catch_expected_panic(|| panic!("inner expected"));
                assert!(inner.is_err());
                panic!("outer expected after inner unwind");
            });
            assert!(nested.is_err());
            let normal = catch_expected_panic(|| {
                assert!(catch_expected_panic(|| panic!("caught inner")).is_err());
                42
            });
            assert_eq!(normal.unwrap(), 42);
            fs::write(root.join("checkpoint.json"),serde_json::to_vec(&json!({"nested_failed":nested.is_err(),"continued":42,"prior":fs::read_to_string(&metadata).unwrap()})).unwrap()).unwrap();
            if scenario == "restored" {
                assert!(panic::catch_unwind(|| panic!("restored ordinary failure")).is_err());
            }
        }
        "thread" => {
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let expected = thread::spawn(move || {
                catch_expected_panic(|| {
                    ready_tx.send(()).unwrap();
                    done_rx.recv().unwrap();
                    panic!("expected thread failure");
                })
            });
            ready_rx.recv().unwrap();
            assert!(thread::spawn(|| panic!("other thread ordinary failure")).join().is_err());
            done_tx.send(()).unwrap();
            assert!(expected.join().unwrap().is_err());
        }
        _ => panic!("unknown fixture scenario"),
    }
}

#[allow(clippy::unwrap_used)]
fn assert_crash(root: &Path, stderr: &[u8], expected: &str) {
    let bytes = fs::read(root.join("last-crash.json")).unwrap();
    let report: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(report["schema"], "claude-rs-crash/v1");
    assert_eq!(report["kind"], "panic");
    assert_eq!(report["message"], expected);
    assert!(report["location"].as_str().unwrap().contains("failure_audit_tests.rs:"));
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["platform"]["os"], std::env::consts::OS);
    assert_eq!(report["platform"]["arch"], std::env::consts::ARCH);
    assert!(!report["created_at"].as_str().unwrap().is_empty());
    assert_eq!(report["diagnostics_bundle_command"], "claude-rs logs --bundle --yes");
    let selected = root.join("runtime").join("claude-rs-20260101T000000Z-p1-rabc.log");
    if selected.exists() {
        assert_eq!(report["latest_log_path"], selected.to_str().unwrap());
    } else {
        assert!(report["latest_log_path"].is_null());
    }
    let text = std::str::from_utf8(stderr).unwrap();
    assert_eq!(field(text, "category"), Some("crash"));
    assert_eq!(field(text, "message"), Some(expected));
    assert_eq!(field(text, "next_step"), Some("claude-rs logs --bundle --yes"));
    for output in [text, String::from_utf8_lossy(&bytes).as_ref()] {
        assert!(!output.contains("raw-panic-secret"));
    }
}

#[tokio::test]
async fn installed_panic_hook_redacts_raw_payloads_and_replaces_prior_metadata() {
    for (scenario, expected) in [
        ("string", "safe panic before api_key=[redacted] after"),
        ("owned", "owned panic before api_key=[redacted] after"),
        ("nonstring", "panic payload is not a string"),
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("last-crash.json"), "prior crash").unwrap();
        fs::create_dir(root.path().join("runtime")).unwrap();
        fs::write(root.path().join("runtime/claude-rs-20260101T000000Z-p1-rabc.log"), "safe log")
            .unwrap();
        let output =
            run(command("failure::audit_tests::panic_worker", root.path(), scenario)).await;
        assert!(!output.status.success());
        assert_crash(root.path(), &output.stderr, expected);
    }
}

#[tokio::test]
async fn expected_panic_scopes_are_nested_restored_and_thread_local() {
    for scenario in ["suppressed", "restored", "thread"] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("last-crash.json"), "prior crash").unwrap();
        let output =
            run(command("failure::audit_tests::panic_worker", root.path(), scenario)).await;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        if scenario != "thread" {
            let checkpoint: Value =
                serde_json::from_slice(&fs::read(root.path().join("checkpoint.json")).unwrap())
                    .unwrap();
            assert_eq!(
                checkpoint,
                json!({"nested_failed":true,"continued":42,"prior":"prior crash"})
            );
        }
        if scenario == "suppressed" {
            assert!(output.stderr.is_empty());
            assert_eq!(fs::read(root.path().join("last-crash.json")).unwrap(), b"prior crash");
        } else {
            assert_crash(
                root.path(),
                &output.stderr,
                if scenario == "thread" {
                    "other thread ordinary failure"
                } else {
                    "restored ordinary failure"
                },
            );
            let stderr = String::from_utf8(output.stderr).unwrap();
            for private in
                ["inner expected", "outer expected", "caught inner", "expected thread failure"]
            {
                assert!(!stderr.contains(private));
            }
        }
    }
}

#[tokio::test]
async fn panic_report_survives_metadata_write_failure_without_damaging_neighbors() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("last-crash.json")).unwrap();
    let blocker = root.path().join("last-crash.json/keep");
    fs::write(&blocker, b"blocker sentinel").unwrap();
    let neighbor = root.path().join("neighbor");
    fs::write(&neighbor, b"neighbor sentinel").unwrap();
    let output = run(command("failure::audit_tests::panic_worker", root.path(), "blocked")).await;
    assert!(!output.status.success());
    let text = String::from_utf8(output.stderr).unwrap();
    assert_eq!(field(&text, "category"), Some("crash"));
    assert_eq!(field(&text, "message"), Some("safe panic before api_key=[redacted] after"));
    assert_eq!(field(&text, "next_step"), Some("claude-rs logs --bundle --yes"));
    assert!(!text.contains("raw-panic-secret"));
    assert_eq!(fs::read(blocker).unwrap(), b"blocker sentinel");
    assert_eq!(fs::read(neighbor).unwrap(), b"neighbor sentinel");
}
