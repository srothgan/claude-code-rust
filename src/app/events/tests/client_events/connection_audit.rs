// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{client::CommandReceiver, wire::BridgeCommand};
use crate::app::HistoryOutputId;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;

fn preferences(home: &Path, approved: &[&Path]) -> std::path::PathBuf {
    let mut projects = serde_json::Map::new();
    for directory in approved {
        projects.insert(
            directory.to_string_lossy().into_owned(),
            json!({"hasTrustDialogAccepted":true}),
        );
    }
    let path = home.join("trust.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"projects":projects,"unrelated":17})).unwrap(),
    )
    .unwrap();
    path
}

pub(super) fn fixture(home: &Path) -> (App, CommandReceiver) {
    let (mut app, commands) = app_with_bridge_connection();
    app.settings_home_override = Some(home.to_path_buf());
    app.cwd_raw = home.to_string_lossy().into_owned();
    app.trust.preferences_path = Some(preferences(home, &[home]));
    (app, commands)
}

fn connection(session: &str, home: &Path) -> ClientEvent {
    crate::app::test_support::session_connection(session, home)
}

fn account(session: &str, subscription: &str) -> ClientEvent {
    ClientEvent::StatusSnapshotReceived {
        session_id: session.into(),
        account: model::AccountInfo {
            email: Some("fixture@example.invalid".into()),
            organization: None,
            subscription_type: Some(subscription.into()),
            token_source: None,
            api_key_source: None,
            api_provider: None,
        },
    }
}

fn welcome_ids(app: &App) -> Vec<HistoryOutputId> {
    app.transcript
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            MessageBlock::Welcome(welcome) => Some(HistoryOutputId::Block(welcome.id)),
            _ => None,
        })
        .collect()
}

fn welcome_region(app: &mut App) -> Vec<String> {
    let ids = welcome_ids(app);
    let rows = crate::ui::inline_chat_rows::serialize_live_rows_at(
        app,
        180,
        &BTreeSet::new(),
        Instant::now() + Duration::from_secs(3),
        &crate::ui::layout_control::LayoutControl::uninterrupted(),
    );
    rows.segments()
        .iter()
        .filter(|segment| segment.ids.iter().any(|id| ids.contains(id)))
        .flat_map(|segment| {
            rows.segment_rows(segment).expect("welcome region").iter().map(ToString::to_string)
        })
        .collect()
}

fn field<'a>(rows: &'a [String], label: &str) -> &'a str {
    let matching = rows.iter().filter(|row| row.contains(label)).collect::<Vec<_>>();
    assert_eq!(matching.len(), 1, "one associated {label} row: {rows:?}");
    matching[0]
}

#[test]
fn connected_welcome_identity_updates_for_pristine_and_started_chat_without_changing_stable_fields()
{
    for started in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let (mut app, _commands) = fixture(home.path());
        app.push_message_tracked(ChatMessage::welcome(
            env!("CARGO_PKG_VERSION"),
            "-",
            "/old",
            "old-session",
        ));
        if started {
            app.push_message_tracked(user_msg("earlier conversation"));
        }
        handle_client_event(&mut app, connection("connected-alpha", home.path()));
        let first = welcome_region(&mut app);
        assert!(field(&first, "Session ID:").contains("connected-alpha"));
        assert!(field(&first, "Version:").contains(env!("CARGO_PKG_VERSION")));
        handle_client_event(&mut app, connection("connected-beta", home.path()));
        let second = welcome_region(&mut app);
        assert!(field(&second, "Session ID:").contains("connected-beta"));
        for label in ["Version:", "Subscription:", "Cwd:"] {
            assert_eq!(field(&first, label), field(&second, label));
        }
        assert_eq!(
            crate::app::test_support::welcome_art(&first),
            crate::app::test_support::welcome_art(&second)
        );
    }
}

#[test]
fn applied_model_change_cannot_take_connection_and_account_owned_welcome_fields() {
    let home = tempfile::tempdir().unwrap();
    let (mut app, _commands) = fixture(home.path());
    app.ensure_welcome_message();
    handle_client_event(&mut app, connection("model-session", home.path()));
    handle_client_event(&mut app, account("model-session", "Fixture Max"));
    let before = welcome_region(&mut app);
    app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
        "model",
        json!("conflicting-saved-model"),
    ));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "model-session".into(),
            update: model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
                test_current_model("confirmed-sonnet"),
            )),
        },
    );
    assert_eq!(app.session_runtime.current_model.as_ref().unwrap().resolved_id, "confirmed-sonnet");
    let after = welcome_region(&mut app);
    for label in ["Version:", "Subscription:", "Cwd:", "Session ID:"] {
        assert_eq!(field(&before, label), field(&after, label));
    }
    assert!(field(&after, "Subscription:").contains("Fixture Max"));
    assert!(field(&after, "Session ID:").contains("model-session"));
}

#[test]
fn account_snapshots_update_one_welcome_region_and_suppression_hides_it_without_dropping_account_data()
 {
    for visible in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let (mut app, _commands) = fixture(home.path());
        app.ensure_welcome_message();
        handle_client_event(&mut app, connection("account-session", home.path()));
        app.show_session_overview = visible;
        let original = welcome_ids(&app);
        assert_eq!(original.len(), 1, "one canonical overview");
        if visible {
            let region = welcome_region(&mut app);
            assert!(field(&region, "Subscription:").contains("Ready"));
            assert!(!field(&region, "Subscription:").contains("Fixture Max"));
        } else {
            assert!(welcome_region(&mut app).is_empty());
        }
        for subscription in ["Fixture Max", "Fixture Pro"] {
            handle_client_event(&mut app, account("account-session", subscription));
            assert_eq!(
                app.session_runtime.account_info.as_ref().unwrap().subscription_type.as_deref(),
                Some(subscription)
            );
            assert_eq!(
                welcome_ids(&app),
                original,
                "account refresh must retain one canonical welcome owner"
            );
            let region = welcome_region(&mut app);
            if visible {
                assert!(field(&region, "Subscription:").contains(subscription));
                assert!(field(&region, "Session ID:").contains("account-session"));
            } else {
                assert!(region.is_empty(), "suppressed welcome produced rows: {region:?}");
            }
        }
    }
}

fn refresh_commands(commands: &mut CommandReceiver, session: &str) -> BTreeSet<&'static str> {
    let mut kinds = BTreeSet::new();
    while let Ok(envelope) = commands.try_recv() {
        let (kind, target) = match envelope.command {
            BridgeCommand::GetMcpSnapshot { session_id } => ("mcp", session_id),
            BridgeCommand::GetStatusSnapshot { session_id } => ("status", session_id),
            BridgeCommand::GetContextUsage { session_id } => ("context", session_id),
            BridgeCommand::InspectSettings { session_id, .. } => ("settings", session_id),
            other => panic!("unexpected refresh: {other:?}"),
        };
        assert_eq!(target, session);
        kinds.insert(kind);
    }
    kinds
}

#[test]
fn connection_and_replacement_request_each_snapshot_for_the_new_session_outside_its_tab() {
    for replacement in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let (mut app, mut commands) = fixture(home.path());
        app.config.active_tab = crate::app::ConfigTab::Help;
        let event = if replacement {
            crate::app::test_support::session_replacement(
                "new-authority",
                home.path(),
                vec![],
                None,
            )
        } else {
            connection("new-authority", home.path())
        };
        handle_client_event(&mut app, event);
        assert_eq!(app.session_runtime.session_id.as_ref().unwrap().as_str(), "new-authority");
        assert_eq!(app.config.active_tab, crate::app::ConfigTab::Help);
        assert_eq!(
            refresh_commands(&mut commands, "new-authority"),
            BTreeSet::from(["mcp", "status", "context", "settings"])
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn connection_on_usage_tab_requests_and_applies_structured_usage_for_its_session() {
    tokio::task::LocalSet::new().run_until(Box::pin(async {
        let home = tempfile::tempdir().unwrap();
        let (mut app, mut commands) = fixture(home.path());
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Usage;
        handle_client_event(&mut app, connection("usage-authority", home.path()));
        let mut requested = false;
        while let Ok(envelope) = commands.try_recv() {
            if let BridgeCommand::GetUsage { session_id } = envelope.command { assert_eq!(session_id, "usage-authority"); requested = true; }
        }
        assert!(requested, "connected Usage view must request bridge-owned usage");
        let snapshot = serde_json::from_value(json!({"subscription_type":"Fixture Team","rate_limits_available":true,"five_hour":{"utilization":27.0,"resets_at":null},"seven_day":{"utilization":73.0,"resets_at":null}})).unwrap();
        handle_client_event(&mut app, ClientEvent::StructuredUsageReceived { session_id: "usage-authority".into(), snapshot: Some(snapshot), error: None });
        let usage = app.usage.snapshot.as_ref().expect("received usage");
        assert_eq!(usage.source, UsageSourceKind::Sdk);
        assert_eq!(usage.subscription_type.as_deref(), Some("Fixture Team"));
        assert!((usage.five_hour.as_ref().unwrap().utilization - 27.0).abs() < 0.001);
        assert!((usage.seven_day.as_ref().unwrap().utilization - 73.0).abs() < 0.001);
        assert!(!app.usage.in_flight);
    })).await;
}

#[test]
fn connection_reconciles_trust_against_the_new_directorys_independent_saved_approval() {
    let home = tempfile::tempdir().unwrap();
    let old = home.path().join("old-project");
    let new = home.path().join("new-project");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    let (mut app, _commands) = fixture(home.path());
    app.trust.preferences_path = Some(preferences(home.path(), &[&old]));
    handle_client_event(&mut app, connection("old-project-session", &old));
    assert_eq!(app.trust.status, crate::app::trust::TrustStatus::Trusted);
    let saved = std::fs::read(app.trust.preferences_path.as_ref().unwrap()).unwrap();
    handle_client_event(&mut app, connection("unapproved-project-session", &new));
    assert_eq!(app.cwd_raw, new.to_string_lossy());
    assert_eq!(app.trust.status, crate::app::trust::TrustStatus::Untrusted);
    assert_eq!(
        std::fs::read(app.trust.preferences_path.as_ref().unwrap()).unwrap(),
        saved,
        "connecting must not grant trust"
    );
    preferences(home.path(), &[&old, &new]);
    handle_client_event(&mut app, connection("approved-project-session", &new));
    assert_eq!(app.trust.status, crate::app::trust::TrustStatus::Trusted);
}

#[test]
fn update_check_event_persists_result_and_preserves_unrelated_settings_without_chat_output() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("app-settings.json");
    let original =
        json!({"updates":{"autoInstall":true,"skipped_version":"9.9.9"},"unrelated":{"keep":17}});
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let mut app = make_test_app();
    app.global_settings = serde_json::from_value(original).unwrap();
    app.global_settings_path = Some(path.clone());
    app.push_message_tracked(user_msg("retained question"));
    let existing = app.transcript.messages.iter().map(|m| m.id).collect::<Vec<_>>();
    handle_client_event(
        &mut app,
        ClientEvent::UpdateCheckCompleted {
            result: crate::app::settings::UpdateCheckResult {
                current_version: "0.2.0".into(),
                latest_version: "0.3.0".into(),
                release_url: "https://github.com/srothgan/claude-code-rust/releases/tag/v0.3.0"
                    .into(),
                checked_at_unix_secs: 12345,
                source: "github_release".into(),
            },
        },
    );
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["updates"]["last_result"],
        json!({"current_version":"0.2.0","latest_version":"0.3.0","release_url":"https://github.com/srothgan/claude-code-rust/releases/tag/v0.3.0","checked_at_unix_secs":12345,"source":"github_release"})
    );
    assert_eq!(saved["updates"]["autoInstall"], true);
    assert_eq!(saved["updates"]["skipped_version"], "9.9.9");
    assert_eq!(saved["unrelated"]["keep"], 17);
    assert_eq!(app.transcript.messages.iter().map(|m| m.id).collect::<Vec<_>>(), existing);
    let saved_message =
        app.transcript.messages.iter().find(|message| message.id == existing[0]).unwrap();
    assert_eq!(saved_message.role, MessageRole::User);
    assert!(
        matches!(saved_message.blocks.as_slice(), [MessageBlock::Text(text)] if text.text == "retained question")
    );
}

#[test]
fn early_fast_update_has_no_session_authority_and_connection_supplies_the_confirmed_snapshot() {
    let home = tempfile::tempdir().unwrap();
    let (mut app, _commands) = fixture(home.path());
    app.session_runtime.session_id = None;
    app.session_runtime.fast_mode_state = model::FastModeState::Off;
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "arriving-session".into(),
            update: model::SessionUpdate::FastModeUpdate {
                state: model::FastModeState::On,
                disabled_reason: None,
            },
        },
    );
    assert!(app.session_runtime.session_id.is_none());
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Off);
    let mut event = connection("arriving-session", home.path());
    let ClientEvent::Connected { fast_mode_state, .. } = &mut event else { unreachable!() };
    *fast_mode_state = model::FastModeState::On;
    handle_client_event(&mut app, event);
    assert_eq!(app.session_runtime.session_id.as_ref().unwrap().as_str(), "arriving-session");
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::On);
}
