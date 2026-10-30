// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;

#[allow(clippy::unwrap_used)]
fn put(directory: &Path, name: &str, bytes: &[u8], seconds: u64) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, bytes).unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
        )
        .unwrap();
    path
}

#[allow(clippy::unwrap_used)]
fn contents(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| (p.file_name().unwrap().to_str().unwrap().to_owned(), fs::read(&p).unwrap()))
        .collect()
}

#[test]
fn retention_and_discovery_preserve_unmanaged_near_matches() {
    let dir = tempfile::tempdir().unwrap();
    let directory = dir.path().canonicalize().unwrap();
    let names = [
        "claude-rs-notes.log",
        "claude-rs-20260101T000000Z-p1-rabc.log.",
        "claude-rs-20260101T000000Z-p1-rabc.log.backup",
        "claude-rs-20260101T000000Z-p-rabc.log",
        "claude-rs-20260101T000000Z-p1-r.log",
        "claude-rs-20260101X000000Z-p1-rabc.log",
        "claude-rs-extra-20260101T000000Z-p1-rabc.log",
        "claude-rs.log",
        "other.log",
    ];
    for name in names {
        put(&directory, name, name.as_bytes(), 1);
    }
    let before = contents(&directory);
    let managed =
        put(&directory, "claude-rs-20260101T000000Z-p12-rhistorical.log.1", b"managed", 2);
    assert_eq!(
        list_managed_runtime_logs_in(&directory)
            .unwrap()
            .iter()
            .map(|l| &l.path)
            .collect::<Vec<_>>(),
        [&managed]
    );
    let target = LogRetentionTarget {
        directory: directory.clone(),
        prefix: "claude-rs",
        extension: "log",
        policy: LogRetentionPolicy {
            max_total_bytes: 0,
            max_age: Duration::ZERO,
            max_files: 0,
            min_files: 0,
        },
    };
    enforce_log_retention_at(&target, None, SystemTime::UNIX_EPOCH + Duration::from_secs(10))
        .unwrap();
    assert!(!managed.exists());
    assert_eq!(contents(&directory), before);
}

#[test]
fn retention_applies_independent_age_byte_and_count_pressure_oldest_first() {
    let cases = [
        (u64::MAX, Duration::MAX, 4, 1, vec!["old", "middle", "new"]),
        (14, Duration::MAX, 4, 1, vec!["old", "middle", "new"]),
        (12, Duration::MAX, 4, 1, vec!["middle", "new"]),
        (9, Duration::MAX, 4, 1, vec!["new"]),
        (u64::MAX, Duration::MAX, 2, 1, vec!["new"]),
        (u64::MAX, Duration::from_secs(100), 4, 1, vec!["middle", "new"]),
        (0, Duration::ZERO, 0, 2, vec!["new"]),
    ];
    for (max_total_bytes, max_age, max_files, min_files, survivors) in cases {
        let dir = tempfile::tempdir().unwrap();
        let active = put(dir.path(), "claude-rs-20200101T000000Z-p1-ractive.log", b"alive", 1);
        let files = [
            ("old", "claude-rs-20300101T000000Z-p1-rold.log", b"oo".as_slice(), 800),
            ("middle", "claude-rs-20200101T000000Z-p1-rmid.log.1", b"mmm".as_slice(), 900),
            ("new", "claude-rs-20100101T000000Z-p1-rnew.log", b"nnnn".as_slice(), 901),
        ];
        for (_, name, bytes, seconds) in files {
            put(dir.path(), name, bytes, seconds);
        }
        put(dir.path(), "neighbor.txt", b"private neighbor", 0);
        fs::create_dir(dir.path().join("claude-rs-20260101T000000Z-p1-rdirectory.log")).unwrap();
        let target = LogRetentionTarget {
            directory: dir.path().into(),
            prefix: "claude-rs",
            extension: "log",
            policy: LogRetentionPolicy { max_total_bytes, max_age, max_files, min_files },
        };
        enforce_log_retention_at(
            &target,
            Some(&active),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1000),
        )
        .unwrap();
        for (id, name, bytes, _) in files {
            if survivors.contains(&id) {
                assert_eq!(fs::read(dir.path().join(name)).unwrap(), bytes);
            } else {
                assert!(!dir.path().join(name).exists(), "{id}");
            }
        }
        assert_eq!(fs::read(&active).unwrap(), b"alive");
        assert_eq!(fs::read(dir.path().join("neighbor.txt")).unwrap(), b"private neighbor");
        assert!(dir.path().join("claude-rs-20260101T000000Z-p1-rdirectory.log").is_dir());
        let before = contents(dir.path());
        enforce_log_retention_at(
            &target,
            Some(&active),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1000),
        )
        .unwrap();
        assert_eq!(contents(dir.path()), before);
        let tighter = LogRetentionTarget {
            policy: LogRetentionPolicy { max_files: 1, min_files: 1, ..target.policy },
            ..target
        };
        enforce_log_retention_at(
            &tighter,
            Some(&active),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1000),
        )
        .unwrap();
        assert_eq!(
            list_managed_runtime_logs_in(dir.path())
                .unwrap()
                .into_iter()
                .map(|l| l.path)
                .collect::<Vec<_>>(),
            [active]
        );
    }
}

#[test]
fn rolling_writer_preserves_byte_boundaries_windows_and_unrelated_data() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("runtime.log");
    put(dir.path(), "neighbor", b"neighbor", 0);
    let mut writer = RollingFileWriter::new(&base, LogOpenMode::CreateNew, 6, 2).unwrap();
    writer.write_all("\u{96ea}".as_bytes()).unwrap();
    writer.flush().unwrap();
    assert_eq!(fs::read(&base).unwrap(), "\u{96ea}".as_bytes());
    assert!(!dir.path().join("runtime.log.1").exists());
    writer.write_all("\u{96ea}".as_bytes()).unwrap();
    writer.flush().unwrap();
    assert_eq!(fs::read(&base).unwrap(), "\u{96ea}\u{96ea}".as_bytes());
    assert!(!dir.path().join("runtime.log.1").exists());
    for bytes in [b"second".as_slice(), b"third!", b"fourth"] {
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    }
    assert_eq!(fs::read(&base).unwrap(), b"fourth");
    assert_eq!(fs::read(dir.path().join("runtime.log.1")).unwrap(), b"third!");
    assert_eq!(fs::read(dir.path().join("runtime.log.2")).unwrap(), b"second");
    assert!(!dir.path().join("runtime.log.3").exists());
    drop(writer);
    let mut writer = RollingFileWriter::new(&base, LogOpenMode::Append, 6, 2).unwrap();
    writer.write_all(b"a").unwrap();
    writer.flush().unwrap();
    assert_eq!(fs::read(&base).unwrap(), b"a");
    assert_eq!(fs::read(dir.path().join("runtime.log.1")).unwrap(), b"fourth");
    assert_eq!(fs::read(dir.path().join("runtime.log.2")).unwrap(), b"third!");
    drop(writer);
    let mut writer = RollingFileWriter::new(&base, LogOpenMode::Append, 6, 2).unwrap();
    writer.write_all(b"b").unwrap();
    writer.flush().unwrap();
    assert_eq!(fs::read(&base).unwrap(), b"ab");
    drop(writer);
    let mut writer = RollingFileWriter::new(&base, LogOpenMode::Truncate, 6, 2).unwrap();
    writer.write_all(b"oversized-record").unwrap();
    writer.flush().unwrap();
    assert_eq!(fs::read(&base).unwrap(), b"oversized-record");
    assert!(!dir.path().join("runtime.log.1").exists());
    assert!(!dir.path().join("runtime.log.2").exists());
    writer.write_all(b"next").unwrap();
    writer.flush().unwrap();
    assert_eq!(fs::read(&base).unwrap(), b"next");
    assert_eq!(fs::read(dir.path().join("runtime.log.1")).unwrap(), b"oversized-record");
    assert_eq!(fs::read(dir.path().join("neighbor")).unwrap(), b"neighbor");
}

#[allow(clippy::unwrap_used)]
fn event<'a>(records: &'a [Value], name: &str) -> &'a Value {
    records.iter().find(|v| v["event_name"] == name).unwrap()
}

#[test]
fn baseline_native_records_preserve_safe_values_and_bound_unicode_without_private_fields() {
    for (text, expected) in [
        ("short".into(), "short".into()),
        ("\u{96ea}".repeat(2048), "\u{96ea}".repeat(2048)),
        (format!("{}TAIL", "\u{96ea}".repeat(2048)), format!("{}...", "\u{96ea}".repeat(2048))),
    ] {
        let records = super::tests::baseline_records(|| {
            tracing::warn!(target:"app.session",event_name="native_failure",error_message="before api_key=raw-native-secret after",error_preview=%text,error=?"debug-safe",reason=%"display-safe",count=0_u64,exit_code=-2_i64,detailed_diagnostics=false,session_id="private-session",request_id="private-request",tool_call_id="private-tool",prompt="private-prompt",content="private-content",command="private-command",path="private-path");
            tracing::warn!(target:"app.session",message="private-unnamed-message",error_message="anonymous safe cause");
        });
        let record = event(&records, "native_failure");
        assert_eq!(record["schema"], "claude-rs-baseline/v1");
        assert_eq!(record["target"], "app.session");
        assert_eq!(record["level"], "WARN");
        assert_eq!(record["error_message"], "before api_key=[redacted] after");
        assert_eq!(record["error_preview"], expected);
        assert_eq!(record["reason"], "display-safe");
        assert_eq!(record["error"], "\"debug-safe\"");
        assert_eq!(record["count"], 0);
        assert_eq!(record["exit_code"], -2);
        assert_eq!(record["detailed_diagnostics"], false);
        let anonymous =
            records.iter().find(|v| v["error_message"] == "anonymous safe cause").unwrap();
        assert!(anonymous.get("message").is_none());
        let written = serde_json::to_string(&records).unwrap();
        for secret in [
            "raw-native-secret",
            "private-session",
            "private-request",
            "private-tool",
            "private-prompt",
            "private-content",
            "private-command",
            "private-path",
            "private-unnamed-message",
            "TAIL",
        ] {
            assert!(!written.contains(secret));
        }
    }
}

#[test]
fn bridge_records_route_by_level_and_target_and_distinguish_absent_from_zero() {
    let warning=json!({"schema":"claude-rs-log/v1","level":"warn","target":"bridge.permission","event_name":"bridge_warning","message":"hook failed","outcome":"failure","session_id":"session-1","request_id":"request-1","tool_call_id":"tool-1","duration_ms":0,"count":0,"size_bytes":0,"fields":{"hook_name":"PreToolUse:Bash","exit_code":0,"cause":"lint gate","attempts":0}}).to_string();
    let error=json!({"schema":"claude-rs-log/v1","level":"error","target":"unknown.target","event_name":"unknown_failure","message":"safe failure"}).to_string();
    let routine=json!({"schema":"claude-rs-log/v1","level":"debug","target":"bridge.sdk","event_name":"routine","message":"started"}).to_string();
    let records = super::tests::baseline_records(|| {
        emit_bridge_stderr_line(&warning);
        emit_bridge_stderr_line(&error);
        emit_bridge_stderr_line(&routine);
    });
    let record = event(&records, "bridge_warning");
    assert_eq!(record["target"], "bridge.permission");
    assert_eq!(record["level"], "WARN");
    assert_eq!(record["outcome"], "failure");
    for key in ["duration_ms", "count", "size_bytes"] {
        assert_eq!(record[key], 0);
        assert!(event(&records, "unknown_failure").get(key).is_none());
    }
    assert_eq!(
        serde_json::from_str::<Value>(record["fields_json"].as_str().unwrap()).unwrap(),
        json!({"hook_name":"PreToolUse:Bash","exit_code":0,"cause":"lint gate","attempts":0})
    );
    let fallback = event(&records, "unknown_failure");
    assert_eq!(fallback["target"], "bridge.sdk");
    assert_eq!(fallback["bridge_target"], "unknown.target");
    assert_eq!(fallback["level"], "ERROR");
    assert!(!records.iter().any(|v| v["event_name"] == "routine"));
    let detailed = super::test_capture::capture(|| emit_bridge_stderr_line(&warning));
    let record = event(&detailed, "bridge_warning");
    for (key, value) in
        [("session_id", "session-1"), ("request_id", "request-1"), ("tool_call_id", "tool-1")]
    {
        assert_eq!(record[key], value);
    }
    for key in ["duration_ms", "count", "size_bytes"] {
        assert_eq!(record[key], 0);
    }
}

#[test]
fn stderr_fallback_retains_exact_unicode_prefix_and_redacts_credentials() {
    let cases = [
        ("short failure".into(), "short failure".into()),
        ("\u{96ea}".repeat(240), "\u{96ea}".repeat(240)),
        (format!("{}TAIL", "\u{96ea}".repeat(240)), format!("{}...", "\u{96ea}".repeat(240))),
        (
            "failure api_key=raw-stderr-secret context".into(),
            "failure api_key=[redacted] context".into(),
        ),
    ];
    for (line, expected) in cases {
        let records = super::tests::baseline_records(|| emit_bridge_stderr_line(&line));
        let record = event(&records, "bridge_stderr_unstructured");
        assert_eq!(record["preview"], expected);
        assert_eq!(record["line_chars"], line.chars().count());
        assert!(!record.to_string().contains("raw-stderr-secret"));
    }
    for line in [
        r#"{"schema":"future/v9","level":"warn","target":"bridge.sdk","event_name":"bad","message":"schema failure"}"#,
        r#"{"schema":"claude-rs-log/v1","level":"unknown","message":"shape failure"}"#,
        "{ malformed failure",
    ] {
        let records = super::tests::baseline_records(|| emit_bridge_stderr_line(line));
        let preview = event(&records, "bridge_stderr_unstructured")["preview"].as_str().unwrap();
        if let Ok(expected) = serde_json::from_str::<Value>(line) {
            assert_eq!(serde_json::from_str::<Value>(preview).unwrap(), expected);
        } else {
            assert_eq!(preview, line);
        }
    }
}

#[test]
fn logging_worker() {
    use clap::Parser;
    let Ok(_scenario) = std::env::var("CLAUDE_RS_AUDIT_CASE") else { return };
    let root = PathBuf::from(std::env::var_os("CLAUDE_RS_AUDIT_ROOT").unwrap());
    let args: Vec<String> =
        serde_json::from_str(&std::env::var("CLAUDE_RS_AUDIT_ARGS").unwrap()).unwrap();
    let cli = Cli::try_parse_from(["claude-rs".to_owned()].into_iter().chain(args)).unwrap();
    let result =
        LoggingRuntime::init_with_paths(&cli, || Ok(crate::diagnostics_test_support::paths(&root)));
    let mut outcome = json!({"ok":result.is_ok(),"continued":42});
    match result {
        Ok(runtime) => {
            let launcher = crate::agent::bridge::BridgeLauncher {
                runtime_path: root.join("inert-runtime"),
                script_path: root.join("inert-script"),
            };
            let cmd = launcher.command(bridge_diagnostics_enabled());
            let value = cmd
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == "CLAUDE_RS_BRIDGE_DIAGNOSTICS")
                .unwrap()
                .1
                .unwrap()
                .to_str()
                .unwrap();
            outcome["bridge_env"] = json!(value);
            tracing::info!(target:"app.lifecycle",event_name="audit_lifecycle",message="lifecycle");
            tracing::info!(target:"app.session",event_name="audit_session",message="session");
            tracing::debug!(target:"app.session",event_name="audit_session_debug",message="session debug");
            tracing::trace!(target:"app.render",event_name="audit_render",message="render");
            tracing::warn!(target:"app.session",event_name="audit_warning",error_message="safe warning",session_id="detailed-identity");
            tracing::info!(target:"tui_markdown",event_name="audit_markdown_info",message="markdown info");
            tracing::debug!(target:"tui_markdown",event_name="audit_markdown_debug",message="markdown debug");
            emit_bridge_stderr_line(
                r#"{"schema":"claude-rs-log/v1","level":"info","target":"bridge.sdk","event_name":"audit_bridge","message":"bridge"}"#,
            );
            drop(runtime);
        }
        Err(error) => {
            outcome["error"] = json!(format!("{error:#}"));
            outcome["bridge_env"] = json!(if bridge_diagnostics_enabled() { "1" } else { "0" });
        }
    }
    fs::write(root.with_extension("outcome.json"), serde_json::to_vec(&outcome).unwrap()).unwrap();
}

#[allow(clippy::unwrap_used)]
async fn initialize(root: &Path, args: &[String], rust_log: Option<&str>) -> Value {
    let mut cmd = crate::diagnostics_test_support::command(
        "logging::audit_tests::logging_worker",
        root,
        "init",
    );
    cmd.env("CLAUDE_RS_AUDIT_ARGS", serde_json::to_string(args).unwrap());
    if let Some(filter) = rust_log {
        cmd.env("RUST_LOG", filter);
    }
    let output = crate::diagnostics_test_support::run(cmd).await;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&fs::read(root.with_extension("outcome.json")).unwrap()).unwrap()
}

#[allow(clippy::unwrap_used)]
fn persisted(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn production_logging_selects_baseline_and_detailed_filters_with_explicit_precedence() {
    // Expected records are literal target/level outcomes, independently of filter strings.
    let cases: &[(&[&str], Option<&str>, &[&str])] = &[
        (&[], None, &["audit_lifecycle", "audit_warning"]),
        (
            &["--enable-logs"],
            None,
            &[
                "audit_lifecycle",
                "audit_warning",
                "audit_session",
                "audit_markdown_info",
                "audit_bridge",
            ],
        ),
        (
            &["--diagnostics-preset", "session"],
            Some("off"),
            &[
                "audit_lifecycle",
                "audit_warning",
                "audit_session",
                "audit_session_debug",
                "audit_markdown_info",
                "audit_bridge",
            ],
        ),
        (
            &["--diagnostics-preset", "render"],
            None,
            &[
                "audit_lifecycle",
                "audit_warning",
                "audit_session",
                "audit_render",
                "audit_markdown_info",
                "audit_bridge",
            ],
        ),
        (
            &["--log-filter", "off,app.render=trace", "--diagnostics-preset", "session"],
            Some("app.session=debug"),
            &["audit_render", "audit_markdown_info"],
        ),
        (
            &[],
            Some("off,app.session=debug"),
            &["audit_warning", "audit_session", "audit_session_debug", "audit_markdown_info"],
        ),
        (
            &["--log-filter", "trace"],
            None,
            &[
                "audit_lifecycle",
                "audit_warning",
                "audit_session",
                "audit_session_debug",
                "audit_render",
                "audit_markdown_info",
                "audit_bridge",
            ],
        ),
        (
            &["--log-filter", "off,tui_markdown=debug"],
            None,
            &["audit_markdown_info", "audit_markdown_debug"],
        ),
    ];
    for (args, environment, expected) in cases {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("diagnostics");
        let options = args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let outcome = initialize(&root, &options, *environment).await;
        assert_eq!(outcome["ok"], true);
        let detailed = !args.is_empty() || environment.is_some();
        assert_eq!(outcome["bridge_env"], if detailed { "1" } else { "0" });
        let logs = list_managed_runtime_logs_in(&root.join("runtime")).unwrap();
        assert_eq!(logs.len(), 1);
        let records = persisted(&logs[0].path);
        let observed = records
            .iter()
            .filter_map(|v| v["event_name"].as_str())
            .filter(|n| n.starts_with("audit_"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(observed, expected.iter().copied().collect(), "{args:?} {environment:?}");
        if let Some(warning) = records.iter().find(|r| r["event_name"] == "audit_warning") {
            assert_eq!(warning["error_message"], "safe warning");
            if detailed {
                assert_eq!(warning["session_id"], "detailed-identity");
            } else {
                assert_eq!(warning["schema"], "claude-rs-baseline/v1");
                assert!(!fs::read_to_string(&logs[0].path).unwrap().contains("detailed-identity"));
            }
        }
    }
}

#[test]
fn selected_log_modes_preserve_existing_runs_and_apply_only_requested_file_policy() {
    use clap::Parser;
    let temp = tempfile::tempdir().unwrap();
    let paths = crate::diagnostics_test_support::paths(temp.path());
    fs::create_dir_all(&paths.runtime_dir).unwrap();
    let prior =
        put(&paths.runtime_dir, "claude-rs-20200101T000000Z-p1-rprevious.log", b"previous run", 1);
    let cli = Cli::try_parse_from(["claude-rs"]).unwrap();
    let selected = resolve_log_path_with(&cli, || Ok(paths.clone())).unwrap().unwrap();
    assert_eq!(selected.path.parent(), Some(paths.runtime_dir.as_path()));
    fs::write(&selected.path, b"collision sentinel").unwrap();
    assert!(RollingFileWriter::new(&selected.path, selected.open_mode, 100, 2).is_err());
    assert_eq!(fs::read(&selected.path).unwrap(), b"collision sentinel");
    for (append, explicit, expected) in
        [(false, true, b"new".as_slice()), (true, true, b"oldnew"), (true, false, b"oldnew")]
    {
        let target =
            if explicit { temp.path().join("explicit.log") } else { paths.legacy_log_path.clone() };
        fs::write(&target, b"old").unwrap();
        let mut cli = Cli::try_parse_from(["claude-rs"]).unwrap();
        cli.log_append = append;
        if explicit {
            cli.log_file = Some(target.clone());
        }
        let selected = resolve_log_path_with(&cli, || Ok(paths.clone())).unwrap().unwrap();
        assert_eq!(selected.path, target);
        assert!(selected.retention.is_none());
        let mut writer =
            RollingFileWriter::new(&selected.path, selected.open_mode, 100, 2).unwrap();
        writer.write_all(b"new").unwrap();
        writer.flush().unwrap();
        assert_eq!(fs::read(target).unwrap(), expected);
        assert_eq!(fs::read(&prior).unwrap(), b"previous run");
    }
}

#[tokio::test]
async fn repeated_default_initializations_preserve_each_runs_records_and_explicit_paths_opt_in() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("diagnostics");
    assert_eq!(initialize(&root, &[], None).await["ok"], true);
    let first = list_managed_runtime_logs_in(&root.join("runtime")).unwrap()[0].path.clone();
    let bytes = fs::read(&first).unwrap();
    assert_eq!(initialize(&root, &[], None).await["ok"], true);
    assert_eq!(fs::read(&first).unwrap(), bytes);
    let logs = list_managed_runtime_logs_in(&root.join("runtime")).unwrap();
    assert_eq!(logs.len(), 2);
    for log in logs {
        assert_eq!(event(&persisted(&log.path), "audit_warning")["error_message"], "safe warning");
    }
    let target = temp.path().join("explicit.log");
    fs::write(&target, b"stale bytes\n").unwrap();
    let options = vec!["--log-file".into(), target.to_str().unwrap().into()];
    let outcome = initialize(&root, &options, None).await;
    assert_eq!(outcome["bridge_env"], "1");
    assert_eq!(event(&persisted(&target), "audit_warning")["session_id"], "detailed-identity");
    let first = fs::read(&target).unwrap();
    let mut append = options;
    append.push("--log-append".into());
    assert_eq!(initialize(&root, &append, None).await["ok"], true);
    assert!(fs::read(&target).unwrap().starts_with(&first));
    let outcome = initialize(&root, &["--log-append".into()], None).await;
    assert_eq!(outcome["bridge_env"], "1");
    assert_eq!(
        event(&persisted(&root.join("claude-rs.log")), "audit_warning")["session_id"],
        "detailed-identity"
    );
}

#[tokio::test]
async fn initialization_failure_is_optional_only_for_baseline_and_invalid_filters_preserve_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("blocked");
    fs::write(&root, b"blocker").unwrap();
    let baseline = initialize(&root, &[], None).await;
    assert_eq!(baseline["ok"], true);
    assert_eq!(baseline["continued"], 42);
    assert_eq!(baseline["bridge_env"], "0");
    for args in [
        vec!["--enable-logs".into()],
        vec!["--diagnostics-preset".into(), "session".into()],
        vec!["--log-file".into(), root.join("explicit.log").to_str().unwrap().into()],
    ] {
        let outcome = initialize(&root, &args, None).await;
        assert_eq!(outcome["ok"], false);
        assert!(outcome["error"].as_str().unwrap().contains("log directory"));
        assert_eq!(fs::read(&root).unwrap(), b"blocker");
    }
    let log = temp.path().join("existing.log");
    fs::write(&log, b"existing data").unwrap();
    let outcome = initialize(
        &root,
        &[
            "--log-file".into(),
            log.to_str().unwrap().into(),
            "--log-filter".into(),
            "app.session=impossible-level".into(),
        ],
        None,
    )
    .await;
    assert_eq!(outcome["ok"], false);
    assert!(outcome["error"].as_str().unwrap().contains("invalid tracing filter"));
    assert_eq!(fs::read(log).unwrap(), b"existing data");
}
