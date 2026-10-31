// SPDX-License-Identifier: Apache-2.0
// These independent literal fixtures require exact values; no approximate calculation is compared.
#![allow(clippy::float_cmp)]

use super::super::test_support::CliFixture;
use super::*;

#[test]
fn selected_panel_values_ignore_context_noise_and_keep_each_model_reset() {
    for sonnet in ["Current week (Sonnet)", "Current week (Sonnet only)"] {
        let panel = format!(
            "\x1b]0;fixture title\x1b\\\x1b[32mCurrent session\x1b[0m\n12:00 | Opus | 0% left\n85% left\nResets tomorrow 日本語\nCurrent week (all models)\n12.5% used\nResets Saturday\n{sonnet}\n36% used\nResets Sunday\nCurrent week (Opus)\n47% used\nResets Monday"
        );
        let snapshot = parse_usage_output(&panel).unwrap();
        assert_eq!(snapshot.source, UsageSourceKind::Cli);
        for (window, label, value, reset) in [
            (snapshot.five_hour, "5-hour", 15.0, "Resets tomorrow 日本語"),
            (snapshot.seven_day, "7-day", 12.5, "Resets Saturday"),
            (snapshot.seven_day_sonnet, "7-day Sonnet", 36.0, "Resets Sunday"),
            (snapshot.seven_day_opus, "7-day Opus", 47.0, "Resets Monday"),
        ] {
            let window = window.unwrap();
            assert_eq!(window.label, label);
            assert_eq!(window.utilization, value);
            assert_eq!(window.reset_description.as_deref(), Some(reset));
            assert_eq!(window.resets_at, None);
        }
        assert!(
            snapshot.extra_usage.is_none()
                && snapshot.session.is_none()
                && snapshot.activity.is_none()
                && snapshot.seven_day_oauth_apps.is_none()
                && snapshot.model_scoped.is_empty()
        );
    }
    assert!(parse_usage_output("Current session\n12:00 | Opus | 0% left").is_err());
    let snapshot=parse_usage_output("Current session\n19% used\nCurrent week (all models)\nLoading...\nCurrent week (Sonnet)\n33% used\nResets next week").unwrap();
    assert!(snapshot.seven_day.is_none());
    assert_eq!(snapshot.seven_day_sonnet.unwrap().utilization, 33.0);
    let updated =
        parse_usage_output("Current session\n99% used\nCurrent session\n21% used").unwrap();
    assert_eq!(updated.five_hour.unwrap().utilization, 21.0);
}

#[test]
fn unusable_and_current_error_panels_report_a_cause_and_accept_corrected_data() {
    for (raw, cause) in [
        ("", "empty"),
        ("Loading usage data...", "missing Current session"),
        ("Current week (all models)\n25% used", "missing Current session"),
        ("Failed to load usage data", "could not load"),
        ("Current session\nrate limited", "rate limited"),
    ] {
        let error = parse_usage_output(raw).unwrap_err();
        assert!(error.contains(cause), "{error}");
    }
    let corrected = parse_usage_output("rate limited\nCurrent session\n18% used").unwrap();
    assert_eq!(corrected.five_hour.unwrap().utilization, 18.0);
    assert!(
        parse_usage_output("Current session\n18% used\nrate limited")
            .unwrap_err()
            .contains("rate limited")
    );
}

#[tokio::test]
async fn native_cli_resolution_preserves_argv_cwd_streams_and_failure_status() {
    if let Some(root) = std::env::var_os("CLAUDE_RS_AUDIT_ROOT") {
        let root = PathBuf::from(root);
        let project = root.join("selected project");
        if std::env::var("CLAUDE_RS_AUDIT_CASE").unwrap() == "missing" {
            assert!(
                Probe::system()
                    .fetch_snapshot(project.to_str().unwrap().into())
                    .await
                    .unwrap_err()
                    .contains("not found in PATH")
            );
            return;
        }
        for (mode, stdout, stderr, expected) in [
            ("ok", "Current session\n13% used\nResets stdout", "", Ok(13.0)),
            ("ok", "diagnostic on stdout", "Current session\n29% used\nResets stderr", Ok(29.0)),
            ("fail", "stdout refusal", "stderr explanation", Err("exit code 7")),
            ("fail", "Current session\n88% used", "", Err("exit code 7")),
            ("ok", "", "", Err("empty")),
            ("ok", "Loading usage data...", "", Err("missing Current session")),
        ] {
            for (name, value) in [("mode", mode), ("stdout", stdout), ("stderr", stderr)] {
                std::fs::write(project.join(name), value).unwrap();
            }
            let result = Probe::system().fetch_snapshot(project.to_str().unwrap().into()).await;
            match expected {
                Ok(value) => {
                    let snapshot = result.unwrap();
                    assert_eq!(snapshot.source, UsageSourceKind::Cli);
                    assert_eq!(snapshot.five_hour.unwrap().utilization, value);
                }
                Err(cause) => {
                    let message = result.unwrap_err();
                    assert!(message.contains(cause), "{message}");
                    if stdout == "stdout refusal" {
                        assert!(message.contains(stdout) && message.contains(stderr));
                    }
                }
            }
            assert_eq!(
                std::fs::read(project.join("argv")).unwrap(),
                b"/usage\0--allowed-tools\0\0"
            );
            assert_eq!(
                PathBuf::from(std::fs::read_to_string(project.join("cwd")).unwrap()),
                project
            );
        }
        let absent = root.join("absent-executable");
        let probe = Probe { executable: Some(absent.clone()), timeout: Duration::from_secs(2) };
        let error = probe.fetch_snapshot(project.to_str().unwrap().into()).await.unwrap_err();
        assert!(error.contains("Failed to run") && error.contains(absent.to_str().unwrap()));
        return;
    }
    let fixture = CliFixture::new();
    let empty = fixture.root.path().join("empty-bin");
    std::fs::create_dir(&empty).unwrap();
    for scenario in ["normal", "missing"] {
        let mut command = crate::diagnostics_test_support::command(
            "app::usage::cli::audit_tests::native_cli_resolution_preserves_argv_cwd_streams_and_failure_status",
            fixture.root.path(),
            scenario,
        );
        command
            .env(
                "PATH",
                if scenario == "missing" {
                    empty.as_path()
                } else {
                    fixture.executable.parent().unwrap()
                },
            )
            .env("HOME", fixture.root.path().join("home"))
            .env("CLAUDE_CONFIG_DIR", fixture.root.path().join("profile"));
        let output = crate::diagnostics_test_support::run(command).await;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}

#[tokio::test]
async fn a_timed_out_ready_cli_child_is_reaped_before_a_successful_retry() {
    let fixture = CliFixture::new();
    fixture.configure("hang", "", "");
    let mut probe = fixture.probe();
    probe.timeout = Duration::from_secs(2);
    let future = probe.fetch_snapshot(fixture.project.to_str().unwrap().into());
    tokio::pin!(future);
    let pid = tokio::select! {pid=fixture.pid()=>pid,result=&mut future=>panic!("probe ended before readiness: {result:?}")};
    assert!(!fixture.stopped(pid));
    fixture.assert_invocation();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    let error = future.await.unwrap_err();
    tokio::time::resume();
    assert!(error.contains("timed out"));
    assert!(
        fixture.stopped(pid),
        "production must terminate and reap the observed child before returning"
    );
    fixture.configure("ok", "Current session\n63% used", "");
    let recovered = probe.fetch_snapshot(fixture.project.to_str().unwrap().into()).await.unwrap();
    assert_eq!(recovered.five_hour.unwrap().utilization, 63.0);
    fixture.assert_invocation();
    assert!(fixture.stopped(fixture.pid().await));
}

#[test]
fn each_panel_window_owns_its_values_and_reset_description() {
    assert!(
        parse_usage_output(
            "Current session\nLoading...\nCurrent week (all models)\n42% used\nResets weekly"
        )
        .is_err()
    );
    let snapshot = parse_usage_output(
        "Current session\n15% used\nCurrent week (all models)\n42% used\nResets weekly",
    )
    .unwrap();
    assert_eq!(snapshot.five_hour.unwrap().reset_description, None);
    assert_eq!(snapshot.seven_day.unwrap().reset_description.as_deref(), Some("Resets weekly"));
}

#[test]
fn latest_panel_supersedes_old_errors_but_cannot_borrow_old_success() {
    let fixed = parse_usage_output("Failed to load usage data\nCurrent session\n17% used").unwrap();
    assert_eq!(fixed.five_hour.unwrap().utilization, 17.0);
    assert!(parse_usage_output("Current session\n17% used\nCurrent session\nLoading...").is_err());
    let error =
        parse_usage_output("Current session\n17% used\nFailed to load usage data").unwrap_err();
    assert!(error.contains("could not load"));
}
