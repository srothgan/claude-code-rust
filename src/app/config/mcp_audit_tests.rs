// SPDX-License-Identifier: Apache-2.0
use super::super::mcp::McpServerActionKind as Action;
use super::*;
use crate::agent::{events::ClientEvent, model::McpAuthCapabilities, types};
use crate::app::handle_client_event;
use std::collections::BTreeSet;

fn connected() -> (TempDir, App, crate::agent::client::CommandReceiver) {
    let (dir, mut app) = open_settings_test_app();
    app.config.active_tab = ConfigTab::Mcp;
    app.session_runtime.session_id = Some("mcp-audit".into());
    let rx = attach_test_connection(&mut app);
    (dir, app, rx)
}
fn key(app: &mut App, code: KeyCode) {
    crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}
fn commands(rx: &mut crate::agent::client::CommandReceiver) -> Vec<BridgeCommand> {
    std::iter::from_fn(|| rx.try_recv().ok()).map(|e| e.command).collect()
}
fn names(app: &App) -> BTreeSet<&str> {
    app.mcp.servers.iter().map(|s| s.name.as_str()).collect()
}
fn choose(app: &mut App, action: Action) {
    key(app, KeyCode::Enter);
    let server = app.mcp.servers[app.config.mcp_selected_server_index].clone();
    let actions = available_mcp_actions(app, &server);
    let position = actions.iter().position(|a| *a == action).expect("offered action");
    for _ in 0..position {
        key(app, KeyCode::Down);
    }
    key(app, KeyCode::Enter);
}
fn confirm(app: &mut App) {
    assert!(app.config.confirmation_overlay().is_some());
    key(app, KeyCode::Down);
    key(app, KeyCode::Enter);
}
fn request_dynamic_removal(app: &mut App, rx: &mut crate::agent::client::CommandReceiver) {
    choose(app, Action::RemoveDynamicConfig);
    confirm(app);
    assert!(
        matches!(commands(rx).as_slice(), [BridgeCommand::McpSetServers {session_id,..}] if session_id=="mcp-audit")
    );
}
fn snapshot(
    app: &mut App,
    session: &str,
    source: types::McpSnapshotSource,
    servers: Vec<McpServerStatus>,
    error: Option<&str>,
) {
    handle_client_event(
        app,
        ClientEvent::McpSnapshotReceived {
            session_id: session.into(),
            servers,
            source: Some(source),
            error: error.map(Into::into),
            auth_capabilities: McpAuthCapabilities::default(),
        },
    );
}
fn result(app: &mut App, session: &str, removed: &[&str]) {
    handle_client_event(
        app,
        ClientEvent::McpSetServersResult {
            session_id: session.into(),
            result: types::McpSetServersResult {
                removed: removed.iter().map(|s| (*s).into()).collect(),
                ..Default::default()
            },
        },
    );
}

#[test]
fn ownership_controls_the_only_removal_or_plugin_action() {
    let (_dir, mut app, _rx) = connected();
    app.plugins.installed = vec![notion_plugin_entry()];
    let ownership_actions = [
        Action::RemoveUserConfig,
        Action::RemoveLocalConfig,
        Action::RemoveProjectConfig,
        Action::RemoveDynamicConfig,
        Action::ManagePlugin,
    ];
    for (source, scope, name, expected) in [
        (Some("user"), Some("dynamic"), "target", Some(Action::RemoveUserConfig)),
        (Some("local"), Some("user"), "target", Some(Action::RemoveLocalConfig)),
        (Some("project"), None, "target", Some(Action::RemoveProjectConfig)),
        (Some("dynamic"), Some("user"), "target", Some(Action::RemoveDynamicConfig)),
        (None, Some("user"), "target", Some(Action::RemoveUserConfig)),
        (None, Some("local"), "target", Some(Action::RemoveLocalConfig)),
        (None, Some("project"), "target", Some(Action::RemoveProjectConfig)),
        (None, Some("dynamic"), "target", Some(Action::RemoveDynamicConfig)),
        (None, Some("session"), "target", None),
        (None, None, "target", None),
        (Some("sdk"), Some("dynamic"), "plugin:Notion:notion", None),
        (Some("future-source"), Some("user"), "plugin:Notion:notion", None),
        (Some("plugin"), Some("user"), "plugin:Notion:notion", Some(Action::ManagePlugin)),
        (None, Some("dynamic"), "plugin:Notion:notion", Some(Action::ManagePlugin)),
        (Some("plugin"), Some("dynamic"), "plugin:Missing:tools", None),
        (None, Some("dynamic"), "plugin:Missing:tools", None),
    ] {
        let mut server = connected_mcp_server_status(name, "unused", None);
        server.source = source.map(Into::into);
        server.scope = scope.map(Into::into);
        let actual: Vec<_> = available_mcp_actions(&app, &server)
            .into_iter()
            .filter(|a| ownership_actions.contains(a))
            .collect();
        assert_eq!(actual, expected.into_iter().collect::<Vec<_>>(), "{source:?}/{scope:?}/{name}");
    }
}

#[test]
fn explicit_dynamic_source_removes_the_selected_target_and_preserves_rich_wire_configs() {
    let (_dir, mut app, mut rx) = connected();
    let mut target = dynamic_http_mcp_server_status("target", "https://remove.invalid");
    target.source = Some("dynamic".into());
    target.scope = Some("user".into());
    let rich = serde_json::json!({
        "stdio": {"type":"stdio","command":"fixture-tool","args":["--mode","read only"],"env":{"TOKEN":"fixture-value"},"timeout":4300,"request_timeout_ms":9001,"always_load":false},
        "http": {"type":"http","url":"https://keep.invalid/mcp","headers":{"Authorization":"Bearer fixture-only"},"timeout":5000,"request_timeout_ms":8000,"always_load":true,"tools":[{"name":"read","permission_policy":"always_ask","org_max_permission":"allow"}]},
        "sse": {"type":"sse","url":"https://events.invalid/sse","headers":{"X-Fixture":"keep"},"timeout":1234,"request_timeout_ms":9876,"always_load":false,"tools":[{"name":"write","permission_policy":"always_deny","org_max_permission":"ask"}]}
    });
    app.mcp.servers =
        vec![connected_mcp_server_status("persisted", "project", None), target.clone()];
    for (name, config) in rich.as_object().unwrap() {
        app.mcp.servers.push(connected_mcp_server_status(
            name,
            "dynamic",
            Some(serde_json::from_value(config.clone()).unwrap()),
        ));
    }
    key(&mut app, KeyCode::Down);
    choose(&mut app, Action::RemoveDynamicConfig);
    confirm(&mut app);
    let sent = commands(&mut rx);
    assert_eq!(sent.len(), 1, "{sent:?}; {:?}", app.mcp.last_error);
    let BridgeCommand::McpSetServers { session_id, servers } = &sent[0] else {
        panic!("set servers required")
    };
    assert_eq!(session_id, "mcp-audit");
    assert_eq!(serde_json::to_value(servers).unwrap(), rich);
    assert_eq!(names(&app), BTreeSet::from(["persisted", "target", "stdio", "http", "sse"]));
    result(&mut app, "old-session", &["target"]);
    assert_eq!(app.mcp.pending_dynamic_config_removal.as_deref(), Some("target"));
    result(&mut app, "mcp-audit", &["target"]);
    assert_eq!(names(&app), BTreeSet::from(["persisted", "stdio", "http", "sse"]));
    // A stale, ordinary status response cannot resurrect a removed dynamic owner.
    snapshot(
        &mut app,
        "mcp-audit",
        types::McpSnapshotSource::McpStatus,
        vec![target.clone()],
        None,
    );
    assert!(app.mcp.servers.is_empty());
    // A persisted owner exposed under the same name is not the removed dynamic entry.
    let fallback = connected_mcp_server_status("target", "user", None);
    snapshot(
        &mut app,
        "mcp-audit",
        types::McpSnapshotSource::McpStatus,
        vec![fallback.clone()],
        None,
    );
    assert_eq!(app.mcp.servers, vec![fallback]);
}

#[test]
fn ambiguous_dynamic_removal_waits_for_its_session_and_source_and_can_retry_failures() {
    for outcome in [
        "absent",
        "persisted-owner",
        "present",
        "snapshot-error",
        "operation-error",
        "result-error",
    ] {
        let (_dir, mut app, mut rx) = connected();
        let target = dynamic_http_mcp_server_status("target", "https://target.invalid");
        let other = dynamic_http_mcp_server_status("other", "https://other.invalid");
        app.mcp.servers = vec![target.clone(), other.clone()];
        request_dynamic_removal(&mut app, &mut rx);
        result(&mut app, "mcp-audit", &[]);
        assert_eq!(names(&app), BTreeSet::from(["target", "other"]));
        assert_eq!(app.mcp.pending_dynamic_config_removal.as_deref(), Some("target"));
        snapshot(&mut app, "old-session", types::McpSnapshotSource::McpSetServers, vec![], None);
        snapshot(
            &mut app,
            "mcp-audit",
            types::McpSnapshotSource::McpStatus,
            vec![target.clone(), other.clone()],
            None,
        );
        assert_eq!(app.mcp.pending_dynamic_config_removal.as_deref(), Some("target"));
        match outcome {
            "operation-error" => handle_client_event(
                &mut app,
                ClientEvent::McpOperationError {
                    session_id: "mcp-audit".into(),
                    error: types::McpOperationError {
                        server_name: None,
                        operation: "set-servers".into(),
                        message: "fixture transport failure".into(),
                    },
                },
            ),
            "result-error" => handle_client_event(
                &mut app,
                ClientEvent::McpSetServersResult {
                    session_id: "mcp-audit".into(),
                    result: types::McpSetServersResult {
                        errors: BTreeMap::from([(
                            "target".into(),
                            "fixture permission failure".into(),
                        )]),
                        ..Default::default()
                    },
                },
            ),
            _ => snapshot(
                &mut app,
                "mcp-audit",
                types::McpSnapshotSource::McpSetServers,
                if outcome == "absent" {
                    vec![other.clone()]
                } else if outcome == "persisted-owner" {
                    vec![connected_mcp_server_status("target", "user", None), other.clone()]
                } else {
                    vec![target.clone(), other.clone()]
                },
                if outcome == "snapshot-error" { Some("fixture snapshot failure") } else { None },
            ),
        }
        assert!(app.mcp.pending_dynamic_config_removal.is_none(), "{outcome}");
        assert!(app.mcp.removed_config_servers.is_empty());
        assert!(!app.mcp.in_flight);
        if outcome == "absent" {
            assert_eq!(names(&app), BTreeSet::from(["other"]));
            assert!(app.mcp.last_error.is_none());
        } else if outcome == "persisted-owner" {
            assert_eq!(names(&app), BTreeSet::from(["target", "other"]));
            assert_eq!(
                app.mcp.servers.iter().find(|s| s.name == "target").unwrap().scope.as_deref(),
                Some("user")
            );
            assert!(app.mcp.last_error.is_none());
        } else {
            assert_eq!(names(&app), BTreeSet::from(["target", "other"]));
            let error = app.mcp.last_error.as_deref().unwrap();
            assert!(
                error.contains("target")
                    && error.contains(if outcome == "present" { "confirming" } else { "fixture" }),
                "{error}"
            );
            app.config.clear_overlay();
            request_dynamic_removal(&mut app, &mut rx);
            result(&mut app, "mcp-audit", &["target"]);
            snapshot(
                &mut app,
                "mcp-audit",
                types::McpSnapshotSource::McpSetServers,
                vec![other],
                None,
            );
            assert_eq!(names(&app), BTreeSet::from(["other"]));
            assert!(app.mcp.last_error.is_none());
        }
    }
}

#[test]
fn unsupported_dynamic_siblings_prevent_any_destructive_command_or_snapshot_loss() {
    for config in [
        None,
        Some(McpServerStatusConfig::Sdk { name: "fixture".into() }),
        Some(McpServerStatusConfig::Unknown { raw_type: "future".into() }),
        Some(McpServerStatusConfig::ClaudeaiProxy {
            url: "https://proxy.invalid".into(),
            id: "proxy".into(),
            timeout: None,
        }),
    ] {
        let (_dir, mut app, mut rx) = connected();
        app.mcp.servers = vec![
            dynamic_http_mcp_server_status("target", "https://target.invalid"),
            connected_mcp_server_status("unsupported-sibling", "dynamic", config),
        ];
        let original = app.mcp.servers.clone();
        choose(&mut app, Action::RemoveDynamicConfig);
        confirm(&mut app);
        assert!(commands(&mut rx).is_empty());
        assert_eq!(app.mcp.servers, original);
        assert!(app.mcp.pending_dynamic_config_removal.is_none());
        assert!(app.mcp.removed_config_servers.is_empty());
        let error = app.config.overlay_message.as_ref().unwrap();
        assert_eq!(error.kind, OverlayMessageKind::Error);
        assert!(error.text.contains("unsupported-sibling"), "{}", error.text);
    }
}

#[test]
fn clear_auth_defaults_to_cancel_and_only_confirmation_sends_the_selected_identity() {
    for cancel in [KeyCode::Enter, KeyCode::Esc] {
        let (_dir, mut app, mut rx) = connected();
        app.mcp.auth_capabilities.clear_auth = true;
        app.mcp.servers = vec![
            connected_mcp_server_status("decoy", "user", None),
            connected_mcp_server_status("chosen", "project", None),
        ];
        key(&mut app, KeyCode::Down);
        choose(&mut app, Action::ClearAuth);
        key(&mut app, cancel);
        assert_eq!(app.config.mcp_details_overlay().unwrap().server_name, "chosen");
        assert!(commands(&mut rx).is_empty());
        key(&mut app, KeyCode::Enter);
        confirm(&mut app);
        let sent = commands(&mut rx);
        assert_eq!(
            sent.iter()
                .filter(|c| matches!(c, BridgeCommand::McpClearAuth { .. }))
                .collect::<Vec<_>>(),
            vec![&BridgeCommand::McpClearAuth {
                session_id: "mcp-audit".into(),
                server_name: "chosen".into()
            }]
        );
    }
}

#[test]
fn callback_validation_keeps_the_target_and_sends_only_the_corrected_url() {
    let (_dir, mut app, mut rx) = connected();
    app.mcp.servers = vec![connected_mcp_server_status("chosen", "user", None)];
    app.config.overlay = Some(ConfigOverlayState::McpCallbackUrl(McpCallbackUrlOverlayState {
        server_name: "chosen".into(),
        draft: "   ".into(),
        cursor: 3,
    }));
    key(&mut app, KeyCode::Enter);
    assert!(commands(&mut rx).is_empty());
    assert_eq!(app.config.mcp_callback_url_overlay().unwrap().server_name, "chosen");
    assert_eq!(app.config.overlay_message.as_ref().unwrap().kind, OverlayMessageKind::Error);
    let url = "http://localhost:8912/callback?code=fixture&state=chosen";
    crate::app::handle_terminal_event(&mut app, Event::Paste(url.into()));
    key(&mut app, KeyCode::Enter);
    let sent = commands(&mut rx);
    assert!(sent.contains(&BridgeCommand::McpOauthCallbackUrl {
        session_id: "mcp-audit".into(),
        server_name: "chosen".into(),
        callback_url: url.into()
    }));
    assert_eq!(
        sent.iter().filter(|c| matches!(c, BridgeCommand::McpOauthCallbackUrl { .. })).count(),
        1
    );
    assert!(app.config.overlay.is_none());
}

#[test]
fn refresh_replaces_stale_rows_and_routes_the_received_server_actions() {
    let (_dir, mut app, mut rx) = connected();
    app.mcp.servers = vec![connected_mcp_server_status("stale", "user", None)];
    key(&mut app, KeyCode::Char('r'));
    let sent = commands(&mut rx);
    assert_eq!(sent.len(), 2);
    assert!(sent.contains(&BridgeCommand::ReloadPlugins {
        session_id: "mcp-audit".into(),
        force: false,
        add_plugins: vec![]
    }));
    assert!(sent.contains(&BridgeCommand::GetMcpSnapshot { session_id: "mcp-audit".into() }));
    assert!(app.mcp.servers.is_empty());
    key(&mut app, KeyCode::Enter);
    assert!(app.config.overlay.is_none());
    snapshot(
        &mut app,
        "mcp-audit",
        types::McpSnapshotSource::McpStatus,
        vec![connected_mcp_server_status("fresh", "project", None)],
        None,
    );
    choose(&mut app, Action::Reconnect);
    assert!(commands(&mut rx).contains(&BridgeCommand::McpReconnect {
        session_id: "mcp-audit".into(),
        server_name: "fresh".into()
    }));
}

#[test]
fn auth_capability_combinations_gate_actual_command_delivery() {
    for bits in 0..8 {
        for proxy in [false, true] {
            for action in [Action::Authenticate, Action::ClearAuth] {
                let (_dir, mut app, mut rx) = connected();
                app.mcp.auth_capabilities = McpAuthCapabilities {
                    authenticate: bits & 1 != 0,
                    submit_oauth_callback_url: bits & 2 != 0,
                    clear_auth: bits & 4 != 0,
                };
                let mut server = connected_mcp_server_status("chosen", "session", None);
                server.status = McpServerConnectionStatus::NeedsAuth;
                if proxy {
                    server.config = Some(McpServerStatusConfig::ClaudeaiProxy {
                        url: "https://proxy.invalid".into(),
                        id: "proxy".into(),
                        timeout: None,
                    });
                }
                app.mcp.servers = vec![server];
                choose(&mut app, action);
                let allowed = if action == Action::Authenticate {
                    bits & 3 == 3 && !proxy
                } else {
                    bits & 4 != 0
                };
                if action == Action::ClearAuth && allowed {
                    confirm(&mut app);
                }
                let sent = commands(&mut rx);
                if allowed {
                    let expected = if action == Action::Authenticate {
                        BridgeCommand::McpAuthenticate {
                            session_id: "mcp-audit".into(),
                            server_name: "chosen".into(),
                        }
                    } else {
                        BridgeCommand::McpClearAuth {
                            session_id: "mcp-audit".into(),
                            server_name: "chosen".into(),
                        }
                    };
                    assert!(sent.contains(&expected), "{bits}/{proxy}/{action:?}");
                } else {
                    assert!(sent.is_empty(), "{bits}/{proxy}/{action:?}");
                    assert_eq!(app.config.mcp_details_overlay().unwrap().server_name, "chosen");
                }
            }
        }
    }
}

#[test]
fn inventory_and_snapshots_reconcile_plugin_owners_in_either_arrival_order() {
    use crate::agent::events::ClientEvent;
    use crate::app::handle_client_event;
    use crate::app::plugins::{PluginsCliActionSuccess, PluginsInventorySnapshot};
    use std::collections::BTreeSet;
    for action_result in [false, true] {
        for inventory_first in [false, true] {
            for selected in ["plugin:Stale:gone", "fff"] {
                let (_dir, mut app) = open_settings_test_app();
                app.session_runtime.session_id =
                    Some(crate::agent::model::SessionId::new("session-1"));
                app.plugins.session_plugins = vec![crate::agent::types::SessionPlugin {
                    name: "attached".into(),
                    path: "session-folder".into(),
                    source: None,
                    version: None,
                }];
                let mut runtime =
                    connected_mcp_server_status("plugin:Runtime:tools", "dynamic", None);
                runtime.source = Some("sdk".into());
                let servers = vec![
                    runtime,
                    notion_mcp_server_status(),
                    connected_mcp_server_status("plugin:attached:tools", "dynamic", None),
                    connected_mcp_server_status("plugin:Stale:gone", "dynamic", None),
                    connected_mcp_server_status("fff", "user", None),
                ];
                let snapshot = PluginsInventorySnapshot {
                    installed: vec![notion_plugin_entry()],
                    marketplace: vec![],
                    marketplaces: vec![],
                };
                let inventory = if action_result {
                    ClientEvent::PluginsCliActionSucceeded {
                        cwd_raw: app.cwd_raw.clone(),
                        result: PluginsCliActionSuccess {
                            snapshot,
                            message: "Uninstalled stale plugin".into(),
                            claude_path: "unused-fixture-cli".into(),
                        },
                    }
                } else {
                    ClientEvent::PluginsInventoryUpdated {
                        cwd_raw: app.cwd_raw.clone(),
                        snapshot,
                        claude_path: "unused-fixture-cli".into(),
                    }
                };
                let mcp = ClientEvent::McpSnapshotReceived {
                    session_id: "session-1".into(),
                    servers,
                    auth_capabilities: crate::agent::model::McpAuthCapabilities::default(),
                    source: Some(crate::agent::types::McpSnapshotSource::ReloadPlugins),
                    error: None,
                };
                if inventory_first {
                    handle_client_event(&mut app, inventory);
                    handle_client_event(&mut app, mcp);
                } else {
                    handle_client_event(&mut app, mcp);
                    assert!(
                        app.mcp.servers.iter().any(|s| s.name == "plugin:Stale:gone"),
                        "no confirmed inventory yet"
                    );
                    app.config.overlay =
                        Some(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
                            server_name: selected.into(),
                            selected_index: 0,
                        }));
                    handle_client_event(&mut app, inventory);
                    if selected == "fff" {
                        assert_eq!(
                            app.config
                                .mcp_details_overlay()
                                .expect("live target remains")
                                .server_name,
                            "fff"
                        );
                    } else {
                        assert!(app.config.overlay.is_none(), "stale details close");
                    }
                }
                assert_eq!(
                    app.mcp.servers.iter().map(|s| s.name.as_str()).collect::<BTreeSet<_>>(),
                    BTreeSet::from([
                        "plugin:Notion:notion",
                        "plugin:attached:tools",
                        "plugin:Runtime:tools",
                        "fff"
                    ])
                );
            }
        }
    }
}

#[test]
fn manage_plugin_routes_to_the_matching_project_or_session_owner() {
    for session_owner in [false, true] {
        let (_dir, mut app, mut rx) = connected();
        let mut wrong_project = notion_plugin_entry();
        wrong_project.scope = "project".into();
        wrong_project.project_path = Some("/unrelated-project".into());
        let mut chosen = notion_plugin_entry();
        chosen.scope = "project".into();
        chosen.project_path = Some(app.cwd_raw.clone());
        let inventory = crate::app::plugins::PluginsInventorySnapshot {
            installed: vec![
                installed_mcp_plugin_entry("other@market", &["notion"]),
                wrong_project,
                chosen,
            ],
            marketplace: vec![],
            marketplaces: vec![],
        };
        if session_owner {
            app.plugins.session_plugins = vec![
                types::SessionPlugin {
                    name: "other".into(),
                    path: "other-folder".into(),
                    source: None,
                    version: None,
                },
                types::SessionPlugin {
                    name: "Notion".into(),
                    path: "session-folder".into(),
                    source: Some("Notion@inline".into()),
                    version: None,
                },
            ];
        }
        snapshot(
            &mut app,
            "mcp-audit",
            types::McpSnapshotSource::Init,
            vec![connected_mcp_server_status("decoy", "user", None), notion_mcp_server_status()],
            None,
        );
        let cwd_raw = app.cwd_raw.clone();
        handle_client_event(
            &mut app,
            ClientEvent::PluginsInventoryUpdated {
                cwd_raw,
                snapshot: inventory,
                claude_path: "fixture-only".into(),
            },
        );
        assert!(names(&app).contains("plugin:Notion:notion"));
        commands(&mut rx);
        key(&mut app, KeyCode::Down);
        choose(&mut app, Action::ManagePlugin);
        if session_owner {
            assert_eq!(app.config.active_tab, ConfigTab::Plugins);
            assert_eq!(app.plugins.active_tab, crate::app::plugins::PluginsViewTab::Session);
            let plugin = &app.plugins.session_plugins[app.plugins.session_selected_index];
            assert_eq!((&*plugin.name, &*plugin.path), ("Notion", "session-folder"));
            assert!(app.config.installed_plugin_actions_overlay().is_none());
        } else {
            let overlay = app.config.installed_plugin_actions_overlay().unwrap();
            assert_eq!(overlay.plugin_id, "notion@claude-plugins-official");
            assert_eq!(overlay.scope, "project");
            assert_eq!(overlay.project_path.as_deref(), Some(app.cwd_raw.as_str()));
        }
        assert!(commands(&mut rx).is_empty());
    }
}
