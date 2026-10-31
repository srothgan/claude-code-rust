// SPDX-License-Identifier: Apache-2.0

use crate::app::{
    App, ConfigTab, ExtraUsage, SessionUsageSummary, UsageSnapshot, UsageSourceKind, UsageWindow,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

fn shell(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| super::render(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .collect()
}

fn card(rows: &[String], title: &str) -> String {
    let start = rows
        .iter()
        .position(|row| row.contains(title) && row.contains('┌'))
        .unwrap_or_else(|| panic!("missing {title}: {rows:?}"));
    let end =
        (start + 1..rows.len()).find(|&i| rows[i].starts_with('└')).expect("complete card bottom");
    rows[start + 1..end]
        .iter()
        .map(|row| row.trim_matches('│').trim())
        .collect::<Vec<_>>()
        .join(" ")
}

fn field(rows: &[String], label: &str) -> String {
    let group = match label {
        "Version" | "Session ID" | "cwd" | "Git branch" => "Session",
        "Login method" | "API provider" | "Email" | "Organization" | "Subscription" => "Account",
        "Model" | "Resolved model ID" | "Requested model" | "Mode" => "Model",
        "Memory" | "Setting sources" => "Settings",
        _ => panic!("unassigned field {label}"),
    };
    let start = rows
        .iter()
        .position(|row| row == group)
        .unwrap_or_else(|| panic!("missing {group}: {rows:?}"))
        + 1;
    let end = (start..rows.len())
        .find(|&index| ["Session", "Account", "Model", "Settings"].contains(&rows[index].as_str()))
        .unwrap_or(rows.len());
    let prefix = format!("{label}:");
    rows[start..end]
        .iter()
        .find_map(|row| row.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("missing {label}: {rows:?}"))
        .trim()
        .into()
}

fn usage_app() -> App {
    let mut app = App::test_default();
    app.config.active_tab = ConfigTab::Usage;
    app
}

fn snapshot() -> UsageSnapshot {
    UsageSnapshot {
        source: UsageSourceKind::Sdk,
        fetched_at: std::time::SystemTime::now(),
        subscription_type: None,
        five_hour: None,
        seven_day: None,
        seven_day_oauth_apps: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        model_scoped: vec![],
        extra_usage: None,
        session: None,
        activity: None,
    }
}

fn window(label: &str, utilization: f64, reset: &str) -> UsageWindow {
    UsageWindow {
        label: label.into(),
        utilization,
        resets_at: None,
        reset_description: Some(reset.into()),
    }
}

#[test]
fn usage_empty_loading_failure_and_received_empty_states_recover_truthfully() {
    let mut app = usage_app();
    let content = |app: &mut App| {
        let rows = rows(&shell(app, 100, 24));
        let start = rows.iter().position(|row| row.contains("Usage")).unwrap() + 1;
        let end = rows.iter().position(|row| row.contains("r refresh")).unwrap();
        rows[start..end].join(" ")
    };
    let idle = content(&mut app);
    assert!(idle.contains("No usage snapshot") && idle.contains("Press r"), "{idle}");
    app.usage.in_flight = true;
    let loading = content(&mut app);
    assert!(loading.contains("Loading") && loading.contains("Waiting"), "{loading}");
    assert!(!loading.contains("No usage snapshot"));
    app.usage.in_flight = false;
    app.usage.last_error = Some("fixture account denied access".into());
    let failure = content(&mut app);
    assert!(failure.contains("Unable") && failure.contains("fixture account denied access"));
    assert!(!failure.contains("Waiting"));
    app.usage.last_error = None;
    app.usage.snapshot = Some(snapshot());
    let empty = content(&mut app);
    assert!(empty.contains("No usage metrics"), "received empty result: {empty}");
    assert!(
        !empty.contains("No usage snapshot") && !empty.contains("denied") && !empty.contains("0%")
    );
    app.usage.snapshot.as_mut().unwrap().five_hour =
        Some(window("Account session", 37.0, "resets tomorrow"));
    let populated = content(&mut app);
    assert!(populated.contains("Account session   37% used"));
    assert!(!populated.contains("No usage"));
}

#[test]
fn session_totals_distinguish_elapsed_api_time_and_unknown_line_counts() {
    let mut app = usage_app();
    for (added, removed, expected, excluded) in [
        (Some(12.0), Some(3.0), "+12 / -3 lines", "-0"),
        (Some(12.0), None, "+12 lines", "-0"),
        (None, Some(3.0), "-3 lines", "+0"),
        (Some(0.0), Some(0.0), "+0 / -0 lines", "+12"),
        (None, None, "3 models", "lines"),
    ] {
        let mut data = snapshot();
        data.session = Some(SessionUsageSummary {
            total_cost_usd: Some(1.2345),
            total_duration_ms: Some(65_000.0),
            total_api_duration_ms: Some(2_000.0),
            total_lines_added: added,
            total_lines_removed: removed,
            model_count: Some(3),
        });
        app.usage.snapshot = Some(data);
        let rendered = rows(&shell(&mut app, 100, 24));
        let totals = card(&rendered, "Current session totals");
        for value in ["$1.2345 cost", "1m 5s elapsed", "2s API time", "3 models", expected] {
            assert!(totals.contains(value), "missing {value}: {totals}");
        }
        assert!(!totals.contains(excluded), "{totals}");
        assert!(!rendered.iter().any(|row| row.contains("% used")));
    }
}

#[test]
fn usage_windows_gauges_credit_relationship_and_failure_stay_in_their_regions() {
    let mut app = usage_app();
    for width in [40, 100] {
        for amount in [0u8, 37, 100] {
            let mut data = snapshot();
            data.five_hour = Some(window("Session quota", f64::from(amount), "resets in 2h"));
            data.seven_day = Some(window("Weekly quota", 62.0, "resets in 4d"));
            data.extra_usage = Some(ExtraUsage {
                used_credits: Some(99.99),
                monthly_limit: Some(100.0),
                currency: Some("EUR".into()),
                utilization: None,
            });
            app.usage.snapshot = Some(data);
            app.usage.last_error = Some("fixture refresh connection refused".into());
            let buffer = shell(&mut app, width, 40);
            let rendered = rows(&buffer);
            for (label, value, reset, wrong) in [
                ("Session quota", format!("{amount}% used"), "resets in 2h", "resets in 4d"),
                ("Weekly quota", "62% used".into(), "resets in 4d", "resets in 2h"),
            ] {
                let start = rendered.iter().position(|row| row.starts_with(label)).unwrap();
                assert!(rendered[start].contains(&value), "{:?}", &rendered[start..start + 3]);
                assert!(rendered[start + 2].contains(reset));
                assert!(!rendered[start..start + 3].join(" ").contains(wrong));
                if label == "Session quota" {
                    let y = u16::try_from(start + 1).unwrap();
                    let filled = |x| buffer[(x, y)].symbol() == "\u{2588}";
                    if amount == 0 {
                        assert!(!(2..width - 2).any(filled));
                    } else if amount == 100 {
                        assert!(filled(3) && filled(width - 3));
                    } else {
                        assert!(filled(3));
                        assert!(!filled(width - 3));
                    }
                }
            }
            assert_eq!(card(&rendered, "Extra credits"), "99.99 of 100.00 EUR used");
            assert_eq!(
                card(&rendered, "Latest refresh error"),
                "fixture refresh connection refused"
            );
            app.usage.last_error = None;
            app.usage.in_flight = true;
            let refreshing = rows(&shell(&mut app, width, 40));
            assert_eq!(card(&refreshing, "Extra credits"), "99.99 of 100.00 EUR used");
            assert!(!refreshing.iter().any(|row| row.contains("refused")));
            app.usage.in_flight = false;
        }
    }
}

#[test]
fn partial_credits_preserve_unknown_amounts_and_report_real_zero() {
    let mut app = usage_app();
    for (used, limit, percent, expected) in [
        (Some(0.0), Some(100.0), None, "0.00 of 100.00 EUR used"),
        (Some(12.0), None, None, "12.00 EUR used"),
        (None, Some(100.0), None, "100.00 EUR limit"),
        (None, None, Some(43.0), "43% of monthly budget"),
        (None, None, None, "No credit usage reported."),
    ] {
        let mut data = snapshot();
        data.extra_usage = Some(ExtraUsage {
            used_credits: used,
            monthly_limit: limit,
            utilization: percent,
            currency: Some("EUR".into()),
        });
        app.usage.snapshot = Some(data);
        let credits = card(&rows(&shell(&mut app, 30, 30)), "Extra credits");
        if used.is_none() && limit.is_none() && percent.is_none() {
            assert!(credits.contains("No credit usage"), "{credits}");
            assert!(!credits.contains("0.00") && !credits.contains("available"));
        } else {
            assert_eq!(credits, expected);
        }
    }
}

#[test]
fn status_fields_keep_session_model_and_source_values_associated() {
    use crate::agent::{
        model::{CurrentModel, SessionId},
        settings::{SettingsScope, SettingsSnapshot, SettingsSource},
    };
    let mut app = App::test_default();
    app.config.active_tab = ConfigTab::Status;
    app.cwd = "display/project".into();
    app.cwd_raw = "raw/project".into();
    app.set_git_branch_for_test(Some("fixture-branch"));
    app.session_runtime.mode = Some(crate::app::ModeState {
        current_mode_id: "fixture-mode-id".into(),
        current_mode_name: "Fixture mode".into(),
        available_modes: vec![],
    });
    app.session_runtime.session_id = Some(SessionId::new("status-identity"));
    let mut model = CurrentModel::new("resolved-model", "Short display", "Long model display");
    model.requested_id = Some("requested-alias".into());
    app.session_runtime.current_model = Some(model);
    app.config.snapshot = Some(SettingsSnapshot {
        sources: [
            (SettingsScope::User, "valid"),
            (SettingsScope::Project, "invalid"),
            (SettingsScope::Local, "missing"),
        ]
        .into_iter()
        .map(|(scope, status)| SettingsSource {
            scope,
            status: status.into(),
            path: String::new(),
            error: None,
            values: vec![],
        })
        .collect(),
        ..Default::default()
    });
    let rendered = rows(&shell(&mut app, 120, 40));
    for (label, expected) in [
        ("Version", env!("CARGO_PKG_VERSION")),
        ("Session ID", "status-identity"),
        ("cwd", "display/project"),
        ("Git branch", "fixture-branch"),
        ("Mode", "Fixture mode"),
        ("Model", "Long model display"),
        ("Resolved model ID", "resolved-model"),
        ("Requested model", "requested-alias"),
    ] {
        assert_eq!(field(&rendered, label), expected);
    }
    let sources = field(&rendered, "Setting sources");
    for pair in ["user (valid)", "project (invalid)", "local (missing)"] {
        assert!(sources.contains(pair), "{sources}");
    }
    app.config.snapshot.as_mut().unwrap().sources[1].status = "valid".into();
    let sources = field(&rows(&shell(&mut app, 120, 40)), "Setting sources");
    for pair in ["user (valid)", "project (valid)", "local (missing)"] {
        assert!(sources.contains(pair), "{sources}");
    }
    assert!(!sources.contains("invalid"));
    app.config.snapshot.as_mut().unwrap().sources.clear();
    assert_eq!(field(&rows(&shell(&mut app, 120, 40)), "Setting sources"), "(no sources reported)");
    app.config.snapshot = None;
    assert_eq!(field(&rows(&shell(&mut app, 120, 40)), "Setting sources"), "(not inspected)");
    for requested in [Some("resolved-model".into()), None] {
        app.session_runtime.current_model.as_mut().unwrap().requested_id = requested;
        let rendered = rows(&shell(&mut app, 120, 40));
        assert!(!rendered.iter().any(|row| row.starts_with("Requested model:")));
        assert_eq!(field(&rendered, "Resolved model ID"), "resolved-model");
    }
    app.session_runtime.current_model = None;
    app.session_runtime.session_id = None;
    app.set_git_branch_for_test(None);
    app.session_runtime.mode = None;
    let rendered = rows(&shell(&mut app, 120, 40));
    assert_eq!(field(&rendered, "Model"), "(not set)");
    assert_eq!(field(&rendered, "Session ID"), "(none)");
    assert!(!rendered.iter().any(|row| row.starts_with("Git branch:") || row.starts_with("Mode:")));
    assert!(!rendered.iter().any(|row| row.starts_with("Resolved model ID:")));
}

#[test]
fn status_account_rows_show_the_model_owned_login_classification() {
    use crate::agent::model::{AccountApiProvider, AccountInfo};
    let mut app = App::test_default();
    app.config.active_tab = ConfigTab::Status;
    for (source, token, provider, expected, provider_label) in [
        (
            Some("oauth"),
            None,
            Some(AccountApiProvider::FirstParty),
            "Claude Max Account",
            Some("First party"),
        ),
        (Some("user"), None, None, "User API key", None),
        (
            Some("oauth"),
            Some("oauth-token"),
            Some(AccountApiProvider::Bedrock),
            "External provider",
            Some("Bedrock"),
        ),
        (None, Some("fixture-token"), None, "fixture-token", None),
        (None, None, None, "Unknown", None),
    ] {
        app.session_runtime.account_info = Some(AccountInfo {
            api_key_source: source.map(Into::into),
            token_source: token.map(Into::into),
            api_provider: provider,
            email: Some("fixture@example.test".into()),
            organization: Some("fixture organization".into()),
            subscription_type: Some("fixture subscription".into()),
        });
        let rendered = rows(&shell(&mut app, 120, 40));
        assert_eq!(field(&rendered, "Login method"), expected);
        assert_eq!(field(&rendered, "Email"), "fixture@example.test");
        assert_eq!(field(&rendered, "Organization"), "fixture organization");
        assert_eq!(field(&rendered, "Subscription"), "fixture subscription");
        if let Some(label) = provider_label {
            assert_eq!(field(&rendered, "API provider"), label);
        } else {
            assert!(!rendered.iter().any(|row| row.starts_with("API provider:")));
        }
    }
    app.session_runtime.account_info = None;
    let rendered = rows(&shell(&mut app, 120, 40));
    for label in ["Login method:", "Email:", "Organization:", "Subscription:", "API provider:"] {
        assert!(!rendered.iter().any(|row| row.starts_with(label)));
    }
}

fn activity() -> crate::app::UsageActivitySummary {
    use crate::app::{UsageActivitySummary, UsageActivityWindow, UsageNamedAttribution};
    let named = |name: &str, pct| vec![UsageNamedAttribution { name: name.into(), pct }];
    UsageActivitySummary {
        day: Some(UsageActivityWindow {
            request_count: 7,
            session_count: 2,
            behaviors: vec![],
            agents: named("day-agent", 11.0),
            skills: named("day-skill", 22.0),
            plugins: named("day-plugin", 33.0),
            mcp_servers: named("day-server", 44.0),
        }),
        week: Some(UsageActivityWindow {
            request_count: 31,
            session_count: 9,
            behaviors: vec![],
            agents: named("week-agent", 55.0),
            skills: named("week-skill", 66.0),
            plugins: named("week-plugin", 77.0),
            mcp_servers: named("week-server", 88.0),
        }),
    }
}

#[test]
fn named_activity_retains_day_week_category_associations_and_local_scope() {
    let mut app = usage_app();
    let mut data = snapshot();
    data.activity = Some(activity());
    app.usage.snapshot = Some(data);
    for width in [40, 100] {
        let rendered = rows(&shell(&mut app, width, 60));
        let detail = card(&rendered, "Approximate local activity");
        let (day, week) = detail.split_once("Last 7 days:").unwrap();
        for (region, count, associations, wrong) in [
            (
                day,
                "7 requests across 2 sessions",
                [
                    "Agents: day-agent 11%",
                    "Skills: day-skill 22%",
                    "Plugins: day-plugin 33%",
                    "MCP servers: day-server 44%",
                ],
                "week-",
            ),
            (
                week,
                "31 requests across 9 sessions",
                [
                    "Agents: week-agent 55%",
                    "Skills: week-skill 66%",
                    "Plugins: week-plugin 77%",
                    "MCP servers: week-server 88%",
                ],
                "day-",
            ),
        ] {
            assert!(region.contains(count), "{region}");
            for pair in associations {
                assert!(region.contains(pair), "{pair}: {region}");
            }
            assert!(!region.contains(wrong), "{region}");
        }
        for scope in [
            "Local transcript scan",
            "approximate",
            "overlap",
            "excludes other devices",
            "claude.ai",
        ] {
            assert!(detail.contains(scope), "{scope}: {detail}");
        }
        assert!(!rendered.iter().any(|row| row.contains("% used") || row.contains("Plan:")));
    }
}

#[test]
fn overflowing_usage_and_status_remain_reachable_without_covering_controls() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let mut app = usage_app();
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    let mut data = snapshot();
    data.five_hour = Some(window("Session quota", 37.0, "resets in 2h"));
    data.activity = Some(activity());
    app.usage.snapshot = Some(data);
    app.usage.last_error = Some("last card failure cause".into());
    let press = |app: &mut App, code| {
        crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    };
    let top = rows(&shell(&mut app, 60, 18));
    assert!(top.iter().any(|row| row.starts_with("Session quota") && row.contains("37%")));
    press(&mut app, KeyCode::End);
    let bottom = rows(&shell(&mut app, 60, 18));
    assert_eq!(card(&bottom, "Latest refresh error"), "last card failure cause");
    assert!(bottom.iter().any(|row| row.contains("r refresh")));
    assert!(bottom.iter().any(|row| row.contains("Esc close")));
    press(&mut app, KeyCode::Up);
    let above_bottom = rows(&shell(&mut app, 60, 18));
    assert_ne!(above_bottom, bottom);
    // A terminal event burst can contain several keys before the next draw.
    press(&mut app, KeyCode::End);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Up);
    assert_eq!(rows(&shell(&mut app, 60, 18)), above_bottom);
    press(&mut app, KeyCode::Home);
    assert_eq!(rows(&shell(&mut app, 60, 18)), top);
    press(&mut app, KeyCode::PageDown);
    let page = rows(&shell(&mut app, 60, 18));
    assert_ne!(page, top);
    press(&mut app, KeyCode::PageUp);
    assert_eq!(rows(&shell(&mut app, 60, 18)), top);
    press(&mut app, KeyCode::Down);
    assert_ne!(rows(&shell(&mut app, 60, 18)), top);
    press(&mut app, KeyCode::Up);
    assert_eq!(rows(&shell(&mut app, 60, 18)), top);
    press(&mut app, KeyCode::End);
    // A wider/taller resize must clamp the old offset so all now-fitting content returns.
    let enlarged = rows(&shell(&mut app, 100, 60));
    assert!(enlarged.iter().any(|row| row.starts_with("Session quota")));
    assert_eq!(card(&enlarged, "Latest refresh error"), "last card failure cause");
    // Real tab navigation resets the viewport and routes the same scroll keys in Status.
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.config.active_tab, ConfigTab::Status);
    let status_top = rows(&shell(&mut app, 60, 12));
    assert_eq!(field(&status_top, "Version"), env!("CARGO_PKG_VERSION"));
    press(&mut app, KeyCode::End);
    let status_bottom = rows(&shell(&mut app, 60, 12));
    assert_eq!(field(&status_bottom, "Setting sources"), "(not inspected)");
    assert!(
        status_bottom[status_bottom.len() - 3..]
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("Esc close")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn status_open_resolves_real_memory_then_removal_without_stale_paths() {
    const NAME: &str = "status_open_resolves_real_memory_then_removal_without_stale_paths";
    if std::env::var("CLAUDE_RS_AUDIT_CASE").ok().as_deref() != Some(NAME) {
        let root = tempfile::tempdir().unwrap();
        let mut command = crate::diagnostics_test_support::command(
            &format!("ui::config::status_usage_tests::{NAME}"),
            root.path(),
            NAME,
        );
        command
            .current_dir(root.path())
            .env("CLAUDE_CONFIG_DIR", root.path().join("profile"))
            .env("CLAUDE_CODE_PROJECT_DIR_NAME", "fixed-project");
        let output = crate::diagnostics_test_support::run(command).await;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("CLAUDE_RS_AUDIT_ROOT").unwrap());
    let memory = root
        .join("profile")
        .join("projects")
        .join("fixed-project")
        .join("memory")
        .join("MEMORY.md");
    std::fs::create_dir_all(memory.parent().unwrap()).unwrap();
    std::fs::write(&memory, "isolated notes").unwrap();
    let mut app = App::test_default();
    app.settings_home_override = None;
    crate::app::config::open_tab(&mut app, ConfigTab::Status).unwrap();
    let available = field(&rows(&shell(&mut app, 240, 40)), "Memory");
    assert!(available.contains(memory.to_str().unwrap()), "{available}");
    std::fs::remove_file(&memory).unwrap();
    crate::app::config::open_tab(&mut app, ConfigTab::Status).unwrap();
    let missing = field(&rows(&shell(&mut app, 240, 40)), "Memory");
    assert!(missing.contains("no memory file"), "{missing}");
    assert!(!missing.contains(memory.to_str().unwrap()));
    app.config.memory_file = None;
    let unresolved = field(&rows(&shell(&mut app, 240, 40)), "Memory");
    assert!(unresolved.contains("unable to resolve"), "{unresolved}");
    assert_ne!(unresolved, missing);
}
