// SPDX-License-Identifier: Apache-2.0
use super::test_support::{InventoryCli, settle};
use super::*;
use crate::agent::events::ClientEvent;
use crate::agent::wire::BridgeCommand;
use crate::app::App;
use crate::app::handle_client_event;
use crate::app::test_support::press;
use crossterm::event::{Event, KeyCode};
use std::time::{Duration, Instant};

use crate::app::test_support::app_with_connection;

fn begin_local_folder(app: &mut App, folder: &str) {
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.active_tab = crate::app::ConfigTab::Plugins;
    assert!(handle_key(
        app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('l'),
            crossterm::event::KeyModifiers::NONE,
        )
    ));
    crate::app::handle_terminal_event(app, crossterm::event::Event::Paste(folder.to_owned()));
    crate::app::handle_terminal_event(
        app,
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        )),
    );
}

fn held_local_reload(app: &mut App, request_id: String) {
    crate::app::handle_client_event(
        app,
        crate::agent::events::ClientEvent::RuntimeReloadHeld {
            session_id: "session-1".to_owned(),
            request_id: Some(request_id),
            error_count: 0,
            cache_impact: crate::agent::types::RuntimeReloadCacheImpact {
                mcp_servers_added: vec!["plugin:fixture:tools".to_owned()],
                mcp_servers_removed: vec![],
                lsp_tool_change: None,
                invalid_server_name_count: 0,
            },
        },
    );
}

#[test]
fn local_folder_hold_apply_and_partial_result_keep_request_identity_and_confirmed_inventory() {
    let (mut app, mut commands) = app_with_connection();
    begin_local_folder(&mut app, "relative-folder");
    let first = commands.try_recv().expect("folder reload");
    assert!(
        matches!(first.command, BridgeCommand::ReloadPlugins { force: false, ref add_plugins, .. } if add_plugins == &["relative-folder"])
    );
    let request = first.request_id.expect("correlated operation");
    held_local_reload(&mut app, request.clone());
    assert!(app.plugins.session_plugins.is_empty());
    assert!(app.config.confirmation_overlay().is_some());
    let key = |app: &mut App, code| {
        crate::app::handle_terminal_event(
            app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                code,
                crossterm::event::KeyModifiers::NONE,
            )),
        )
    };
    key(&mut app, crossterm::event::KeyCode::Down);
    key(&mut app, crossterm::event::KeyCode::Enter);
    let apply = commands.try_recv().expect("apply folder reload");
    assert!(
        matches!(apply.command, BridgeCommand::ReloadPlugins { force: true, ref add_plugins, .. } if add_plugins == &["relative-folder"])
    );
    let current = apply.request_id.expect("new request identity");
    assert_ne!(request, current);
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::RuntimeReloadFailed {
            session_id: "session-1".to_owned(),
            request_id: Some(request),
            message: "stale error".to_owned(),
        },
    );
    assert!(app.plugins.loading);
    let plugins = vec![crate::agent::types::SessionPlugin {
        name: "fixture".to_owned(),
        path: app.cwd_raw.clone(),
        source: Some("fixture@inline".to_owned()),
        version: Some("1.2.3".to_owned()),
    }];
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::PluginInventory {
            session_id: "session-1".to_owned(),
            plugins: plugins.clone(),
        },
    );
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::RuntimeReloadCompleted {
            session_id: "session-1".to_owned(),
            request_id: Some(current),
            error_count: 1,
            reconciliation_error: None,
        },
    );
    assert_eq!(app.plugins.session_plugins, plugins);
    assert!(
        app.plugins.installed.is_empty(),
        "session loading must not fabricate installed entries"
    );
    assert_eq!(app.plugins.active_tab, PluginsViewTab::Session);
    assert!(app.config.overlay.is_none());
    assert!(!app.plugins.loading);
    assert!(
        app.plugins
            .status_message
            .as_ref()
            .is_some_and(|text| text.contains("applied") && text.contains("1 component"))
    );
}

#[test]
fn declining_a_local_folder_reload_preserves_the_draft_and_does_not_dispatch_an_apply() {
    let (mut app, mut commands) = app_with_connection();
    begin_local_folder(&mut app, "folder with spaces");
    let request = commands.try_recv().expect("reload").request_id.expect("request");
    held_local_reload(&mut app, request);
    crate::app::handle_terminal_event(
        &mut app,
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        )),
    );
    assert!(commands.try_recv().is_err());
    assert!(app.plugins.session_plugins.is_empty());
    assert!(app.plugins.pending_reload.is_none());
    assert_eq!(
        app.config.plugin_source_input_overlay().expect("retained input").draft,
        "folder with spaces"
    );
}

#[test]
fn a_refused_local_folder_keeps_its_input_and_confirmed_inventory_available_for_retry() {
    let (mut app, mut commands) = app_with_connection();
    let confirmed = session_plugins("existing", "old-folder");
    app.plugins.session_plugins = confirmed.clone();
    app.input.set_text("keep my chat draft");
    begin_local_folder(&mut app, "new-folder");
    let request = folder_request(&mut commands, "new-folder");
    handle_client_event(
        &mut app,
        ClientEvent::RuntimeReloadFailed {
            session_id: "session-1".into(),
            request_id: Some(request.clone()),
            message: "Managed settings refuse local plugin folders".into(),
        },
    );
    assert_eq!(app.plugins.session_plugins, confirmed);
    assert_eq!(app.config.plugin_source_input_overlay().expect("retry input").draft, "new-folder");
    assert!(
        app.config.overlay_message.as_ref().is_some_and(|m| m.text.contains("Managed settings"))
    );
    assert!(commands.try_recv().is_err());
    retry_folder(&mut app, &mut commands, &confirmed, Some(request));
}

fn session_plugins(name: &str, path: &str) -> Vec<crate::agent::types::SessionPlugin> {
    vec![crate::agent::types::SessionPlugin {
        name: name.into(),
        path: path.into(),
        source: None,
        version: None,
    }]
}

fn folder_request(commands: &mut crate::agent::client::CommandReceiver, folder: &str) -> String {
    let request = commands.try_recv().expect("folder request");
    assert!(
        matches!(request.command, BridgeCommand::ReloadPlugins {ref session_id, force:false, ref add_plugins} if session_id=="session-1" && add_plugins==&[folder])
    );
    assert!(commands.try_recv().is_err(), "one folder request");
    request.request_id.expect("correlated request")
}

fn retry_folder(
    app: &mut App,
    commands: &mut crate::agent::client::CommandReceiver,
    confirmed: &[crate::agent::types::SessionPlugin],
    old: Option<String>,
) {
    // Modify the retained input rather than replacing the overlay with a fabricated draft.
    press(app, KeyCode::End);
    crate::app::handle_terminal_event(app, Event::Paste("-corrected".into()));
    press(app, KeyCode::Enter);
    let current = folder_request(commands, "new-folder-corrected");
    assert_eq!(app.plugins.session_plugins, confirmed, "requested is not confirmed");
    if let Some(old) = old {
        assert_ne!(current, old);
        handle_client_event(
            app,
            ClientEvent::RuntimeReloadCompleted {
                session_id: "session-1".into(),
                request_id: Some(old),
                error_count: 0,
                reconciliation_error: None,
            },
        );
        assert_eq!(
            app.plugins.pending_reload.as_ref().expect("retry still pending").request_id,
            current
        );
        assert!(app.config.plugin_source_input_overlay().is_some());
    }
    let updated = session_plugins("corrected", "new-folder-corrected");
    handle_client_event(
        app,
        ClientEvent::PluginInventory { session_id: "session-1".into(), plugins: updated.clone() },
    );
    handle_client_event(
        app,
        ClientEvent::RuntimeReloadCompleted {
            session_id: "session-1".into(),
            request_id: Some(current),
            error_count: 0,
            reconciliation_error: None,
        },
    );
    assert_eq!(app.plugins.session_plugins, updated);
    assert!(app.plugins.installed.is_empty());
    assert!(app.plugins.pending_reload.is_none());
    assert!(app.config.overlay.is_none());
    assert_eq!(app.input.text(), "keep my chat draft");
    press(app, KeyCode::Enter);
    assert_eq!(
        app.config.plugin_source_input_overlay().expect("editor usable after completion").draft,
        "new-folder-corrected"
    );
}

#[test]
fn folder_admission_failure_retains_editable_input_and_retries_on_a_restored_connection() {
    for unavailable in [false, true] {
        let (mut app, commands) = app_with_connection();
        drop(commands);
        if unavailable {
            app.session_runtime.conn = None;
        }
        let confirmed = session_plugins("existing", "old-folder");
        app.plugins.session_plugins = confirmed.clone();
        app.input.set_text("keep my chat draft");
        begin_local_folder(&mut app, "new-folder");
        assert_eq!(
            app.config.plugin_source_input_overlay().expect("retained folder").draft,
            "new-folder"
        );
        assert_eq!(app.plugins.session_plugins, confirmed);
        assert!(app.plugins.installed.is_empty());
        assert!(app.plugins.pending_reload.is_none());
        assert!(app.config.overlay_message.as_ref().is_some_and(|m| m.text.contains("retry")));
        let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
        app.session_runtime.conn = Some(std::rc::Rc::new(connection));
        retry_folder(&mut app, &mut commands, &confirmed, None);
    }
}

fn installed_plugin_entry(
    id: &str,
    scope: &str,
    project_path: Option<&str>,
    mcp_server_names: &[&str],
) -> InstalledPluginEntry {
    InstalledPluginEntry {
        id: id.to_owned(),
        version: Some("1.0.0".to_owned()),
        scope: scope.to_owned(),
        enabled: true,
        installed_at: None,
        last_updated: None,
        project_path: project_path.map(ToOwned::to_owned),
        mcp_server_names: mcp_server_names.iter().map(|name| (*name).to_owned()).collect(),
    }
}

#[tokio::test]
async fn conditional_inventory_refresh_honors_freshness_and_manual_refresh() {
    let fixture = InventoryCli::new();
    let (mut app, mut commands) = app_with_connection();
    fixture.attach(&mut app);
    settle(&mut app, request_inventory_refresh_if_needed).await;
    super::test_support::assert_fixture_snapshot(&PluginsInventorySnapshot {
        installed: app.plugins.installed.clone(),
        marketplace: app.plugins.marketplace.clone(),
        marketplaces: app.plugins.marketplaces.clone(),
    });
    let listing = vec![
        vec!["plugin", "list", "--json"],
        vec!["plugin", "list", "--available", "--json"],
        vec!["plugin", "marketplace", "list", "--json"],
    ];
    assert_eq!(fixture.calls(), listing);
    settle(&mut app, request_inventory_refresh_if_needed).await;
    assert_eq!(fixture.calls(), listing, "fresh snapshot suppresses a real producer");
    fixture.write("installed.json", &fixture.read("installed-after.json"));
    app.plugins.last_inventory_refresh_at = Some(
        Instant::now().checked_sub(Duration::from_secs(6)).expect("fixture clock supports expiry"),
    );
    settle(&mut app, |app| {
        request_inventory_refresh_if_needed(app);
        request_inventory_refresh_if_needed(app); // outstanding work must not duplicate
    })
    .await;
    assert_eq!(fixture.calls(), (0..2).flat_map(|_| listing.clone()).collect::<Vec<_>>());
    assert_eq!(
        app.plugins
            .installed
            .iter()
            .map(|p| (p.id.as_str(), p.enabled, p.version.as_deref()))
            .collect::<Vec<_>>(),
        [("alpha@fixture", false, Some("2.8"))]
    );
    assert!(commands.try_recv().is_err(), "passive reads do not reload runtime");
    fixture.write("installed.json", "[]");
    settle(&mut app, super::inventory::request_inventory_refresh_manual).await;
    assert_eq!(
        fixture.calls(),
        (0..3).flat_map(|_| listing.clone()).collect::<Vec<_>>(),
        "manual refresh bypasses fresh cache"
    );
    assert!(app.plugins.installed.is_empty());
    let reload = commands.try_recv().expect("manual runtime refresh");
    assert!(
        matches!(reload.command,BridgeCommand::ReloadPlugins {ref session_id,force:false,ref add_plugins} if session_id=="session-1" && add_plugins.is_empty())
    );
    handle_client_event(
        &mut app,
        ClientEvent::RuntimeReloadCompleted {
            session_id: "session-1".into(),
            request_id: reload.request_id,
            error_count: 0,
            reconciliation_error: None,
        },
    );
    settle(&mut app, super::inventory::request_inventory_refresh).await;
    assert_eq!(fixture.calls(), (0..4).flat_map(|_| listing.clone()).collect::<Vec<_>>());
    assert!(
        commands.try_recv().is_err(),
        "manual intent is consumed, not inherited by passive completion"
    );
}

#[test]
fn marketplace_search_fields_and_independent_tab_queries_keep_the_selected_target() {
    let (mut app, _commands) = app_with_connection();
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.active_tab = crate::app::ConfigTab::Plugins;
    app.input.set_text("chat draft remains");
    app.plugins.marketplace = vec![
        marketplace_entry("raw-id@first", "Display", "Description", "Catalog", "9.8"),
        marketplace_entry("other@second", "Other", "Unrelated", "Elsewhere", "7.6"),
    ];
    for (query, expected) in [
        ("RAW-ID", vec!["raw-id@first"]),
        ("display", vec!["raw-id@first"]),
        ("DESCRIPTION", vec!["raw-id@first"]),
        ("catalog", vec!["raw-id@first"]),
        ("9.8", vec!["raw-id@first"]),
        ("missing", vec![]),
        ("", vec!["raw-id@first", "other@second"]),
    ] {
        app.plugins.plugins_search_query = query.into();
        assert_eq!(
            filtered_marketplace_plugins(&app.plugins)
                .iter()
                .map(|p| p.plugin_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "query {query}"
        );
    }
    app.plugins.installed = vec![
        installed_plugin_entry("retain@installed", "user", None, &[]),
        installed_plugin_entry("different@installed", "user", None, &[]),
    ];
    press(&mut app, KeyCode::Up);
    crate::app::handle_terminal_event(&mut app, Event::Paste("retain".into()));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    let target = app.config.installed_plugin_actions_overlay().expect("filtered installed target");
    assert_eq!((&*target.plugin_id, &*target.scope), ("retain@installed", "user"));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Up);
    for ch in "display".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.config.plugin_install_overlay().expect("matching catalog item").plugin_id,
        "raw-id@first"
    );
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Delete);
    crate::app::handle_terminal_event(&mut app, Event::Paste("other".into()));
    press(&mut app, KeyCode::Left);
    assert_eq!(app.plugins.installed_search_query, "retain");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.config
            .installed_plugin_actions_overlay()
            .expect("preserved installed selection")
            .plugin_id,
        "retain@installed"
    );
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.plugins.plugins_search_query, "other");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        app.config.plugin_install_overlay().expect("changed catalog selection").plugin_id,
        "other@second"
    );
    assert_eq!(app.input.text(), "chat draft remains");
}

fn marketplace_entry(
    id: &str,
    name: &str,
    description: &str,
    marketplace: &str,
    version: &str,
) -> MarketplaceEntry {
    MarketplaceEntry {
        plugin_id: id.into(),
        name: name.into(),
        description: Some(description.into()),
        marketplace_name: Some(marketplace.into()),
        version: Some(version.into()),
        install_count: None,
        source: None,
    }
}

#[test]
fn installed_eligibility_preserves_project_identity_and_selected_scope() {
    use std::collections::BTreeSet;
    let root = tempfile::tempdir().expect("projects");
    let current = root.path().join("Project");
    let other = root.path().join("Other");
    std::fs::create_dir(&current).expect("current project");
    std::fs::create_dir(&other).expect("other project");
    let (mut app, _commands) = app_with_connection();
    app.cwd_raw = current.to_str().expect("current path").into();
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.active_tab = crate::app::ConfigTab::Plugins;
    let other = other.to_str().expect("other path");
    app.plugins.installed = vec![
        installed_plugin_entry("other-local@fixture", "local", Some(other), &[]),
        installed_plugin_entry("user@fixture", "user", None, &[]),
        installed_plugin_entry("current-local@fixture", "local", Some(&app.cwd_raw), &[]),
        installed_plugin_entry("current-project@fixture", "project", Some(&app.cwd_raw), &[]),
        installed_plugin_entry("other-project@fixture", "project", Some(other), &[]),
        installed_plugin_entry("missing-local@fixture", "local", None, &[]),
        installed_plugin_entry("missing-project@fixture", "project", None, &[]),
        installed_plugin_entry("managed@fixture", "managed", None, &[]),
        installed_plugin_entry("unknown@fixture", "unknown", Some(&app.cwd_raw), &[]),
    ];
    // Only assert case isolation after demonstrating two genuinely distinct directories.
    let differently_cased = root.path().join("project");
    if !differently_cased.exists() {
        std::fs::create_dir(&differently_cased).expect("case-distinct directory");
        std::fs::write(current.join("identity"), "current").expect("identity marker");
        assert!(!differently_cased.join("identity").exists());
        app.plugins.installed.push(installed_plugin_entry(
            "case-neighbor@fixture",
            "project",
            differently_cased.to_str(),
            &[],
        ));
    }
    let eligible = ordered_installed(&app.plugins, &app.cwd_raw)
        .iter()
        .map(|p| (p.id.as_str(), p.scope.as_str()))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        eligible,
        BTreeSet::from([
            ("user@fixture", "user"),
            ("current-local@fixture", "local"),
            ("current-project@fixture", "project")
        ])
    );
    let target_index = ordered_installed(&app.plugins, &app.cwd_raw)
        .iter()
        .position(|entry| entry.id == "current-local@fixture")
        .expect("eligible target");
    for _ in 0..target_index {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Enter);
    let target = app.config.installed_plugin_actions_overlay().expect("eligible action target");
    assert_eq!(
        (&*target.plugin_id, &*target.scope, target.project_path.as_deref()),
        ("current-local@fixture", "local", Some(app.cwd_raw.as_str()))
    );
}

#[test]
fn persisted_actions_keep_disk_success_separate_from_correlated_runtime_outcomes() {
    for outcome in ["applied", "failed", "force", "cancel", "escape", "offline"] {
        let (mut app, mut commands) = app_with_connection();
        app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
        app.config.active_tab = crate::app::ConfigTab::Plugins;
        app.plugins.session_plugins = session_plugins("confirmed", "confirmed-folder");
        let confirmed = app.plugins.session_plugins.clone();
        if outcome == "offline" {
            app.session_runtime.session_id = None;
        }
        let snapshot = deliver_persisted_snapshot(&mut app);
        assert_eq!(app.plugins.session_plugins, confirmed);
        if outcome == "offline" {
            assert!(commands.try_recv().is_err());
            assert!(
                app.config
                    .status_message
                    .as_deref()
                    .is_some_and(|m| m.contains("Updated installed@disk"))
            );
            assert!(app.config.last_error.is_none());
            assert!(app.plugins.pending_reload.is_none());
            continue;
        }
        let mut request = runtime_request(&mut commands, false);
        for (session, id) in [("session-1", "wrong-id"), ("old-session", request.as_str())] {
            complete_runtime(&mut app, session, id);
            assert_eq!(
                app.plugins.pending_reload.as_ref().expect("current operation").request_id,
                request
            );
            assert!(
                !app.config
                    .status_message
                    .as_deref()
                    .is_some_and(|m| m.contains("Updated installed@disk"))
            );
        }
        if ["force", "cancel", "escape"].contains(&outcome) {
            hold_persisted_runtime(&mut app, &request);
            if outcome != "force" {
                press(&mut app, if outcome == "cancel" { KeyCode::Enter } else { KeyCode::Esc });
                assert!(commands.try_recv().is_err(), "cancellation must not force");
                assert!(app.plugins.pending_reload.is_none());
                let feedback =
                    app.config.status_message.as_deref().expect("persisted change feedback");
                assert!(
                    feedback.contains("files were updated")
                        && feedback.contains("kept its existing tools"),
                    "{feedback}"
                );
                assert_eq!(app.plugins.installed, snapshot.installed);
                assert_eq!(app.plugins.session_plugins, confirmed);
                continue;
            }
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Enter);
            let forced = runtime_request(&mut commands, true);
            assert_ne!(forced, request);
            complete_runtime(&mut app, "session-1", &request);
            assert_eq!(
                app.plugins.pending_reload.as_ref().expect("force still pending").request_id,
                forced
            );
            request = forced;
        }
        if outcome == "failed" {
            request =
                retry_failed_runtime(&mut app, &mut commands, &request, &snapshot, &confirmed);
        }
        complete_runtime(&mut app, "session-1", &request);
        assert!(app.plugins.pending_reload.is_none());
        assert!(app.config.last_error.is_none());
        let completed_feedback = app.config.status_message.clone();
        assert!(completed_feedback.as_deref().is_some_and(|m| if outcome == "failed" {
            m.contains("refreshed")
        } else {
            m.contains("Updated installed@disk")
        }));
        handle_client_event(
            &mut app,
            ClientEvent::RuntimeReloadFailed {
                session_id: "session-1".into(),
                request_id: Some(request.clone()),
                message: "late failure after success".into(),
            },
        );
        complete_runtime(&mut app, "session-1", &request);
        assert_eq!(app.config.status_message, completed_feedback);
        assert!(app.config.last_error.is_none());
        assert_eq!(app.plugins.session_plugins, confirmed);
        assert!(commands.try_recv().is_err());
    }
}

fn deliver_persisted_snapshot(app: &mut App) -> PluginsInventorySnapshot {
    let snapshot = PluginsInventorySnapshot {
        installed: vec![installed_plugin_entry("installed@disk", "user", None, &[])],
        marketplace: vec![marketplace_entry(
            "available@catalog",
            "Available",
            "Independent description",
            "Catalog",
            "4.0",
        )],
        marketplaces: vec![MarketplaceSourceEntry {
            name: "Catalog".into(),
            source: Some("git".into()),
            repo: Some("team/catalog".into()),
        }],
    };
    let cwd_raw = app.cwd_raw.clone();
    handle_client_event(
        app,
        ClientEvent::PluginsCliActionSucceeded {
            cwd_raw,
            result: PluginsCliActionSuccess {
                snapshot: snapshot.clone(),
                message: "Updated installed@disk in user scope".into(),
                claude_path: "unused-fixture-cli".into(),
            },
        },
    );
    assert_eq!(app.plugins.installed, snapshot.installed);
    assert_eq!(app.plugins.marketplace, snapshot.marketplace);
    assert_eq!(app.plugins.marketplaces, snapshot.marketplaces);
    snapshot
}

fn retry_failed_runtime(
    app: &mut App,
    commands: &mut crate::agent::client::CommandReceiver,
    request: &str,
    snapshot: &PluginsInventorySnapshot,
    confirmed: &[crate::agent::types::SessionPlugin],
) -> String {
    handle_client_event(
        app,
        ClientEvent::RuntimeReloadFailed {
            session_id: "session-1".into(),
            request_id: Some(request.into()),
            message: "fixture runtime refused changed tools".into(),
        },
    );
    assert!(
        app.config
            .last_error
            .as_deref()
            .is_some_and(|m| m.contains("fixture runtime refused changed tools"))
    );
    assert_eq!(app.plugins.installed, snapshot.installed);
    assert_eq!(app.plugins.session_plugins, confirmed);
    // Session-tab refresh retries only the runtime application, not the disk mutation.
    press(app, KeyCode::Left);
    press(app, KeyCode::Char('r'));
    let retry = runtime_request(commands, false);
    assert_ne!(retry, request);
    complete_runtime(app, "session-1", request);
    assert_eq!(app.plugins.pending_reload.as_ref().expect("retry still pending").request_id, retry);
    retry
}

fn runtime_request(
    commands: &mut crate::agent::client::CommandReceiver,
    force_expected: bool,
) -> String {
    let request = commands.try_recv().expect("runtime reload");
    assert!(
        matches!(request.command,BridgeCommand::ReloadPlugins {ref session_id,force,ref add_plugins} if session_id=="session-1" && force==force_expected && add_plugins.is_empty())
    );
    assert!(commands.try_recv().is_err());
    request.request_id.expect("correlated runtime reload")
}

fn complete_runtime(app: &mut App, session: &str, request: &str) {
    handle_client_event(
        app,
        ClientEvent::RuntimeReloadCompleted {
            session_id: session.into(),
            request_id: Some(request.into()),
            error_count: 0,
            reconciliation_error: None,
        },
    );
}

fn hold_persisted_runtime(app: &mut App, request: &str) {
    handle_client_event(
        app,
        ClientEvent::RuntimeReloadHeld {
            session_id: "session-1".into(),
            request_id: Some(request.into()),
            error_count: 0,
            cache_impact: crate::agent::types::RuntimeReloadCacheImpact {
                mcp_servers_added: vec!["plugin:new:search".into()],
                mcp_servers_removed: vec!["plugin:old:fetch".into()],
                lsp_tool_change: Some("adds".into()),
                invalid_server_name_count: 1,
            },
        },
    );
    let body = &app.config.confirmation_overlay().expect("actual held confirmation").body;
    for (label, value) in [
        ("Add", "plugin:new:search"),
        ("Remove", "plugin:old:fetch"),
        ("LSP", "adds"),
        ("hidden", "unsafe"),
    ] {
        assert!(body.lines().any(|line| line.contains(label) && line.contains(value)), "{body}");
    }
}

#[path = "persistent_audit_tests.rs"]
mod persistent_audit;
