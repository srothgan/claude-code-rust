use super::*;
use pretty_assertions::assert_eq;

mod connection_audit;
mod feedback_audit;

mod inventory_audit;
mod picker_audit;

#[test]
fn connected_updates_cwd_and_clears_resuming_marker() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    app.set_pending_session_resume("resume-123".into(), Some("resume-at-1".into()));

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("session-cwd"),
            cwd: "/changed".into(),
            current_model: test_current_model("claude-updated"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: Vec::new(),
        },
    );

    assert_eq!(app.cwd_raw, "/changed");
    assert_eq!(app.cwd, "/changed");
    assert!(app.pending_session_resume.is_none());
    let Some(first) = app.transcript.messages.first() else {
        panic!("missing welcome message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.cwd, "/changed");
}

#[test]
fn session_handoff_isolates_plugin_reload_and_account_context() {
    for replacement in [false, true] {
        let (mut app, mut commands) = session_reset_fixture();
        app.input.set_text("old session draft");
        app.plugins.session_plugins = reset_attachment("old");
        begin_reset_folder(&mut app, "old-folder");
        let old_request =
            commands.try_recv().expect("old local reload").request_id.expect("old correlation");

        let event = if replacement {
            ClientEvent::SessionReplaced {
                session_id: model::SessionId::new("test-session"),
                cwd: "/test".into(),
                current_model: test_current_model("claude-updated"),
                available_models: vec![],
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates: vec![],
                restored_input: None,
            }
        } else {
            connected_event("claude-updated")
        };
        handle_client_event(&mut app, event);

        assert!(matches!(app.status, AppStatus::Ready));
        assert!(app.usage.snapshot.is_none());
        assert!(app.session_runtime.account_info.is_none());
        assert!(app.plugins.installed.is_empty());
        assert!(app.config.pending_session_title_change.is_none());
        assert!(app.session_runtime.session_title.is_none());
        assert_eq!(
            app.input.text(),
            if replacement { "" } else { "old session draft" },
            "initial connection preserves the composer; replacement discards the old draft"
        );
        assert!(app.plugins.session_plugins.is_empty());
        assert!(app.plugins.pending_reload.is_none());
        crate::app::test_support::drain_session_refreshes(&mut commands, "test-session");
        begin_reset_folder(&mut app, "new-folder");
        let request = commands.try_recv().expect("new local reload");
        assert!(
            matches!(request.command,crate::agent::wire::BridgeCommand::ReloadPlugins {ref session_id,force:false,ref add_plugins} if session_id=="test-session" && add_plugins==&["new-folder"])
        );
        let current = request.request_id.expect("new correlation");
        for id in [old_request, current.clone()] {
            handle_client_event(
                &mut app,
                ClientEvent::RuntimeReloadCompleted {
                    session_id: "old-session".into(),
                    request_id: Some(id),
                    error_count: 0,
                    reconciliation_error: None,
                },
            );
        }
        handle_client_event(
            &mut app,
            ClientEvent::PluginInventory {
                session_id: "old-session".into(),
                plugins: reset_attachment("late"),
            },
        );
        assert!(app.plugins.session_plugins.is_empty());
        assert_eq!(
            app.plugins
                .pending_reload
                .as_ref()
                .expect("new operation survives old outcomes")
                .request_id,
            current
        );
        handle_client_event(
            &mut app,
            ClientEvent::PluginInventory {
                session_id: "test-session".into(),
                plugins: reset_attachment("new"),
            },
        );
        handle_client_event(
            &mut app,
            ClientEvent::RuntimeReloadCompleted {
                session_id: "test-session".into(),
                request_id: Some(current),
                error_count: 0,
                reconciliation_error: None,
            },
        );
        assert_eq!(app.plugins.session_plugins, reset_attachment("new"));
        assert!(app.plugins.pending_reload.is_none());
        assert!(app.config.overlay.is_none());
    }
}

fn reset_attachment(name: &str) -> Vec<crate::agent::types::SessionPlugin> {
    vec![crate::agent::types::SessionPlugin {
        name: name.into(),
        path: format!("{name}-folder"),
        source: None,
        version: None,
    }]
}

fn session_reset_fixture() -> (App, crate::agent::client::CommandReceiver) {
    let (mut app, commands) = app_with_bridge_connection();
    app.transcript.messages.push(user_msg("hello"));
    app.status = AppStatus::Running;
    app.usage.snapshot = Some(UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: std::time::SystemTime::now(),
        subscription_type: None,
        five_hour: None,
        seven_day: None,
        seven_day_oauth_apps: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        model_scoped: Vec::new(),
        extra_usage: None,
        session: None,
        activity: None,
    });
    app.session_runtime.account_info = Some(crate::agent::model::AccountInfo {
        email: Some("old@example.com".into()),
        organization: None,
        subscription_type: None,
        token_source: None,
        api_key_source: None,
        api_provider: None,
    });
    app.plugins.installed.push(installed_plugin_entry("old-plugin"));
    app.plugins.last_inventory_refresh_at = Some(Instant::now());
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "old-session".into(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
        });
    // The previous session's name; the bridge sends the connected one's.
    app.session_runtime.session_id = Some(model::SessionId::new("old-session"));
    app.session_runtime.session_title = Some("Old session name".to_owned());

    (app, commands)
}

fn begin_reset_folder(app: &mut App, folder: &str) {
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.active_tab = crate::app::ConfigTab::Plugins;
    crate::app::test_support::press(app, crossterm::event::KeyCode::Char('l'));
    handle_terminal_event(app, crossterm::event::Event::Paste(folder.into()));
    crate::app::test_support::press(app, crossterm::event::KeyCode::Enter);
}

#[test]
fn auth_required_sets_hint_without_prefilling_login_command() {
    let mut app = make_test_app();
    app.input.set_text("keep me");

    handle_client_event(
        &mut app,
        ClientEvent::AuthRequired {
            method_name: "oauth".into(),
            method_description: "Open browser".into(),
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.input.text(), "keep me");
    let Some(hint) = &app.session_runtime.login_hint else {
        panic!("expected login hint");
    };
    assert_eq!(hint.method_name, "oauth");
    assert_eq!(hint.method_description, "Open browser");
}

#[test]
fn service_status_warning_survives_status_snapshot_welcome_update() {
    const CONNECTIVITY_MESSAGE: &str =
        "Claude Code could not connect to the internet. Please check your connection.";
    let mut app = make_test_app();

    handle_client_event(&mut app, connected_event("opus"));
    handle_client_event(
        &mut app,
        ClientEvent::ServiceStatus {
            severity: ServiceStatusSeverity::Warning,
            message: CONNECTIVITY_MESSAGE.into(),
        },
    );
    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "test-session".into(),
            account: crate::agent::model::AccountInfo {
                email: None,
                organization: None,
                subscription_type: Some("Claude Pro".into()),
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    let warning_count = app
        .transcript
        .messages
        .iter()
        .filter(|message| {
            matches!(message.role, MessageRole::System(Some(SystemSeverity::Warning)))
        })
        .filter(|message| first_block_text(message) == CONNECTIVITY_MESSAGE)
        .count();
    assert_eq!(warning_count, 1);
}

#[test]
fn replacement_snapshot_owns_fast_mode_before_and_after_stale_updates() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("old-session"));
    app.session_runtime.fast_mode_state = model::FastModeState::On;
    app.session_runtime.fast_mode_disabled_reason = Some("stale-reason".to_owned());

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "new-session".to_owned(),
            update: model::SessionUpdate::FastModeUpdate {
                state: model::FastModeState::Cooldown,
                disabled_reason: None,
            },
        },
    );
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::On);

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("new-session"),
            cwd: "/replacement".into(),
            current_model: test_current_model("claude"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Cooldown,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "old-session".to_owned(),
            update: model::SessionUpdate::FastModeUpdate {
                state: model::FastModeState::Off,
                disabled_reason: None,
            },
        },
    );

    assert_eq!(
        app.session_runtime.session_id.as_ref().map(model::SessionId::as_str),
        Some("new-session")
    );
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Cooldown);
    assert!(app.session_runtime.fast_mode_disabled_reason.is_none());
}

#[test]
fn unrelated_slash_command_error_does_not_clear_rewind_target_loading_state() {
    let (mut app, mut rx) = crate::app::test_support::app_with_connection();
    request_picker_targets(&mut app, &mut rx, "session-1");
    handle_client_event(&mut app, slash_command_error("Unrelated settings failure".into()));
    assert!(app.sdk_inventory.rewind_targets_in_flight);
    deliver_picker_target(
        &mut app,
        "session-1",
        "usable-target",
        "Usable target after unrelated failure",
    );
    assert!(
        picker_rows(&mut app).iter().any(|r| r.contains("Usable target after unrelated failure"))
    );
    activate_picker_target(&mut app, &mut rx, "session-1", "usable-target");
}

#[test]
fn slash_command_error_for_pending_session_rename_stays_in_config_feedback() {
    let mut app = make_test_app();
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "session-1".to_owned(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Rename {
                requested_title: Some("Renamed session".to_owned()),
            },
        });

    handle_client_event(&mut app, slash_command_error("failed to rename session: boom".into()));

    assert!(app.config.pending_session_title_change.is_none());
    assert_eq!(app.config.last_error.as_deref(), Some("failed to rename session: boom"));
    assert!(app.config.status_message.is_none());
    assert!(app.transcript.messages.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn non_matching_config_option_update_keeps_pending() {
    Box::pin(tokio::task::LocalSet::new().run_until(async {
        let (mut app,mut commands)=crate::app::test_support::app_with_connection();app.status=AppStatus::Ready;
        assert!(crate::app::slash::try_handle_submit(&mut app,"/effort high"));
        let command=tokio::time::timeout(Duration::from_secs(3),commands.recv_envelope()).await.unwrap().unwrap();
        assert!(matches!(command.command,crate::agent::wire::BridgeCommand::SetEffort {session_id,effort} if session_id=="session-1" && effort.as_deref()==Some("high")));
        for (option,value,matching) in [("max_thinking_tokens",serde_json::json!(2048),false),("effortLevel",serde_json::json!("high"),true)] {
            let event=crate::app::connect::dispatch_test_event(serde_json::json!({"event":"session_update","session_id":"session-1","update":{"type":"config_option_update","option_id":option,"value":value}})).await;
            handle_client_event(&mut app,event);
            assert_eq!(app.session_runtime.config_options.get(option),Some(&value));
            if matching {assert_eq!(app.status,AppStatus::Ready);assert!(app.turn.pending_command_ack.is_none());}
            else {assert_eq!(app.status,AppStatus::CommandPending);assert!(matches!(app.turn.pending_command_ack.as_ref(),Some(PendingCommandAck::ConfigOption {option_id}) if option_id=="effortLevel"));}
            app.input.set_text("follow-up draft");crate::app::test_support::press(&mut app,KeyCode::Enter);
            if matching {
                let command=tokio::time::timeout(Duration::from_secs(3),commands.recv_envelope()).await.unwrap().unwrap();
                assert!(matches!(command.command,crate::agent::wire::BridgeCommand::Prompt {session_id,..} if session_id=="session-1"));
                assert!(app.input.text().is_empty());
            } else {assert_eq!(app.input.text(),"follow-up draft");assert!(commands.try_recv().is_err());}
        }
        assert_eq!(app.session_runtime.config_options.get("max_thinking_tokens"),Some(&serde_json::json!(2048)));
    })).await;
}

#[test]
fn session_replaced_restores_input_after_loading_history() {
    let mut app = make_test_app();
    app.turn.pending_command_label = Some("Rewinding conversation...".to_owned());
    app.status = AppStatus::CommandPending;
    let history_updates = vec![model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
        model::ContentBlock::Text(model::TextContent::new("assistant reply")),
    ))];

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("rewound"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates,
            restored_input: Some("selected prompt".to_owned()),
        },
    );

    assert_eq!(app.input.text(), "selected prompt");
    assert!(app.turn.pending_command_label.is_none());
    assert!(matches!(app.status, AppStatus::Ready));
    assert!(canonical_messages_contain_text(&app, "assistant reply"));
}

#[test]
fn ultracode_lifecycle_preserves_conversation_state_and_rejects_stale_sessions() {
    let mut app = make_test_app();
    let on = model::UltracodeState::new(true, true, true);
    app.session_runtime.ultracode = on;
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ConversationReset {
            new_conversation_id: "conversation-2".to_owned(),
            trigger: None,
            timestamp: None,
            user_message_uuid: None,
        }),
    );
    assert_eq!(app.session_runtime.ultracode, on);
    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: "replacement-session".into(),
            cwd: "/replacement".to_owned(),
            current_model: test_current_model("opus"),
            available_models: vec![],
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            ultracode: None,
            history_updates: vec![],
            restored_input: None,
        },
    );
    assert_eq!(app.session_runtime.ultracode, None);
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::UltracodeUpdate { ultracode: on }),
    );
    assert_eq!(
        app.session_runtime.ultracode, None,
        "old-session telemetry must not leak into replacement"
    );
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "replacement-session".to_owned(),
            update: model::SessionUpdate::UltracodeUpdate { ultracode: on },
        },
    );
    assert_eq!(app.session_runtime.ultracode, on);
    handle_client_event(&mut app, ClientEvent::ConnectionFailed("closed".to_owned().into()));
    assert!(app.session_runtime.ultracode.is_none());
}

#[test]
fn session_title_follows_the_child_until_another_session_connects() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    let update =
        |update| ClientEvent::SessionUpdate { session_id: "current-session".to_owned(), update };

    handle_client_event(
        &mut app,
        update(model::SessionUpdate::SessionTitleUpdate(Some("probe-e2e".to_owned()))),
    );
    assert_eq!(app.session_runtime.session_title.as_deref(), Some("probe-e2e"));

    // Reconnecting the same session keeps it.
    app.session_runtime.activate_session(model::SessionId::new("current-session"));
    assert_eq!(app.session_runtime.session_title.as_deref(), Some("probe-e2e"));

    app.session_runtime.activate_session(model::SessionId::new("other-session"));
    assert_eq!(app.session_runtime.session_title, None);
}

#[test]
fn title_update_clears_the_matching_listed_title() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("active"));
    let mut listed = listed_session("active", "Independent summary");
    listed.custom_title = Some("Old name".to_owned());
    handle_client_event(&mut app, ClientEvent::SessionsListed { sessions: vec![listed] });
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "active".to_owned(),
            update: model::SessionUpdate::SessionTitleUpdate(None),
        },
    );
    assert_eq!(app.active_session_custom_title(), None);
    assert_eq!(app.recent_sessions[0].custom_title, None);
    assert_eq!(app.recent_sessions[0].summary, "Independent summary");
}

fn reloaded(names: &[&str]) -> crate::agent::types::ReloadOutcome {
    crate::agent::types::ReloadOutcome::Reloaded {
        names: names.iter().map(|name| (*name).to_owned()).collect(),
    }
}

#[test]
fn skills_and_styles_reload_result_leaves_an_unrelated_pending_command_alone() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Switching mode...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentMode);

    handle_client_event(
        &mut app,
        ClientEvent::SkillsAndStylesReloaded {
            session_id: "test-session".to_owned(),
            skills: reloaded(&[]),
            output_styles: reloaded(&[]),
        },
    );

    assert!(matches!(app.status, AppStatus::CommandPending));
    assert!(matches!(app.turn.pending_command_ack, Some(PendingCommandAck::CurrentMode)));
}

#[test]
fn expired_reload_releases_input_even_while_a_title_change_is_pending() {
    // The title request is the kind of earlier session request that can hold /reload in the
    // bridge queue; a slash error would be taken as its failure and leave the input blocked.
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Reloading skills and output styles...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::SkillsAndStylesReload);
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "test-session".into(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
        });
    let before = app.transcript.messages.len();

    handle_client_event(
        &mut app,
        ClientEvent::SkillsAndStylesReloadExpired {
            session_id: "test-session".to_owned(),
            after_ms: 10_000,
        },
    );

    assert!(matches!(app.status, AppStatus::Ready), "the composer is released");
    assert!(app.turn.pending_command_ack.is_none());
    assert!(app.config.pending_session_title_change.is_some(), "the title request is untouched");
    assert_eq!(app.transcript.messages.len(), before + 1, "exactly one notice");
    let Some(MessageBlock::Notice(notice)) =
        app.transcript.messages.last().and_then(|message| message.blocks.first())
    else {
        panic!("expected a notice block");
    };
    assert_eq!(notice.severity, SystemSeverity::Error);
    assert!(notice.text.text.contains("did not start within 10 s"), "{}", notice.text.text);
    assert!(notice.text.text.contains("Nothing was reloaded"), "{}", notice.text.text);
}

#[test]
fn session_authority_rejects_stale_updates_and_accepts_current_outcomes() {
    use crate::app::test_support::*;
    for kind in 0..5 {
        let (mut app, mut rx) = app_with_connection();
        app.input.set_text("Current turn source");
        crate::app::input_submit::submit_input(&mut app);
        expect_session_prompt(&mut rx, "session-1", "Current turn source", &[], &[]);
        deliver_update(&mut app, source_chunk("Current answer", "current-answer", false));
        app.session_runtime.account_info = Some(handoff_account("seed@example.test"));
        app.mcp.servers = handoff_servers("seed-server");
        let before_model = app.session_runtime.current_model.clone();
        let before_fast = app.session_runtime.fast_mode_state;
        for session in ["obsolete-session", "session-1"] {
            let event = match kind {
                0 => ClientEvent::StatusSnapshotReceived {
                    session_id: session.into(),
                    account: handoff_account("accepted@example.test"),
                },
                1 => ClientEvent::SessionUpdate {
                    session_id: session.into(),
                    update: model::SessionUpdate::CurrentModelUpdate(
                        model::CurrentModelUpdate::new(test_current_model("accepted-model")),
                    ),
                },
                2 => ClientEvent::SessionUpdate {
                    session_id: session.into(),
                    update: model::SessionUpdate::FastModeUpdate {
                        state: model::FastModeState::Cooldown,
                        disabled_reason: None,
                    },
                },
                3 => ClientEvent::TurnComplete {
                    session_id: session.into(),
                    queued_turn_count: None,
                    terminal_reason: None,
                },
                _ => handoff_mcp(session, "accepted-server"),
            };
            handle_client_event(&mut app, event);
            let accepted = session == "session-1";
            match kind {
                0 => assert_eq!(
                    app.session_runtime.account_info.as_ref().unwrap().email.as_deref(),
                    Some(if accepted { "accepted@example.test" } else { "seed@example.test" })
                ),
                1 => {
                    if accepted {
                        assert_eq!(
                            app.session_runtime.current_model.as_ref().unwrap().resolved_id,
                            "accepted-model"
                        );
                    } else {
                        assert_eq!(app.session_runtime.current_model, before_model);
                    }
                }
                2 => assert_eq!(
                    app.session_runtime.fast_mode_state,
                    if accepted { model::FastModeState::Cooldown } else { before_fast }
                ),
                3 => assert_eq!(matches!(app.status, AppStatus::Ready), accepted),
                _ => assert_eq!(
                    app.mcp.servers[0].name,
                    if accepted { "accepted-server" } else { "seed-server" }
                ),
            }
            assert!(canonical_messages_contain_text(&app, "Current answer"));
        }
    }
}

fn handoff_account(email: &str) -> model::AccountInfo {
    model::AccountInfo {
        email: Some(email.into()),
        organization: None,
        subscription_type: None,
        token_source: None,
        api_key_source: None,
        api_provider: None,
    }
}
fn handoff_servers(name: &str) -> Vec<model::McpServerStatus> {
    vec![model::McpServerStatus {
        name: name.into(),
        status: model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: Some("user".into()),
        source: None,
        tools: vec![],
    }]
}
fn handoff_mcp(session: &str, name: &str) -> ClientEvent {
    ClientEvent::McpSnapshotReceived {
        session_id: session.into(),
        auth_capabilities: model::McpAuthCapabilities::default(),
        servers: handoff_servers(name),
        source: Some(crate::agent::types::McpSnapshotSource::McpStatus),
        error: None,
    }
}

#[test]
fn initial_connection_preserves_editable_source_cursor_and_image_payload() {
    use crate::app::test_support::*;
    let directory = tempfile::tempdir().unwrap();
    let (mut app, mut rx) = app_with_connection();
    app.status = AppStatus::Connecting;
    app.session_runtime.session_id = None;
    app.input.set_text("before |after [Image #1]");
    app.input.set_cursor(0, 7);
    let image = session_image();
    app.input.image_payloads.push(image.clone());
    let draft = app.input.snapshot();
    handle_client_event(&mut app, session_connection("connected", directory.path()));
    drain_session_refreshes(&mut rx, "connected");
    assert_eq!(app.input.snapshot(), draft);
    press(&mut app, KeyCode::Char('!'));
    crate::app::input_submit::submit_input(&mut app);
    expect_session_prompt(&mut rx, "connected", "before !|after [Image #1]", &[image], &[]);
}

#[test]
fn replacement_isolates_pending_source_and_releases_old_mcp_removal_guards() {
    use crate::app::test_support::*;
    for restored in [None, Some("Restored source")] {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, mut rx) = app_with_connection();
        app.input.set_text("Old active source");
        crate::app::input_submit::submit_input(&mut app);
        let active = expect_session_prompt(&mut rx, "session-1", "Old active source", &[], &[]);
        deliver_update(&mut app, source_chunk("Old assistant answer", "old-answer", false));
        app.input.set_text("Old pending source");
        crate::app::input_submit::submit_input(&mut app);
        let queued = expect_session_prompt(&mut rx, "session-1", "Old pending source", &[], &[]);
        app.input.set_text("Old unsent draft");
        app.mcp.servers = handoff_servers("old-server");
        app.mcp.removed_config_servers.insert(
            crate::app::state::types::RemovedMcpServerKey::new(
                "user".into(),
                "reusable-server".into(),
            ),
            crate::app::state::types::RemovedMcpServerGuard {
                expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
            },
        );
        let history = vec![
            source_chunk("New historical user", "new-user", true),
            source_chunk("New historical answer", "new-answer", false),
        ];
        handle_client_event(
            &mut app,
            session_replacement("replacement", directory.path(), history, restored),
        );
        drain_session_refreshes(&mut rx, "replacement");
        for uuid in [active, queued] {
            handle_client_event(
                &mut app,
                ClientEvent::UserMessageQueued {
                    session_id: "session-1".into(),
                    message_uuid: uuid.clone(),
                },
            );
            handle_client_event(
                &mut app,
                ClientEvent::UserMessageStarted {
                    session_id: "session-1".into(),
                    message_uuid: uuid.clone(),
                    source: crate::agent::types::UserMessageStartSource::StreamEvent,
                },
            );
            handle_client_event(
                &mut app,
                ClientEvent::UserMessageRejected {
                    session_id: "session-1".into(),
                    message_uuid: uuid,
                    reason: "old rejection".into(),
                },
            );
        }
        assert_eq!(app.input.text(), restored.unwrap_or(""));
        assert!(app.pending_user_messages.is_empty());
        for old in
            ["Old active source", "Old pending source", "Old assistant answer", "old rejection"]
        {
            assert!(!canonical_messages_contain_text(&app, old));
        }
        for text in ["New historical user", "New historical answer"] {
            let index = app
                .transcript
                .messages
                .iter()
                .position(|m| {
                    m.blocks.iter().any(|b| matches!(b,MessageBlock::Text(t) if t.text==text))
                })
                .unwrap();
            assert!(transcript_region(&mut app, index).contains(text));
        }
        handle_client_event(&mut app, handoff_mcp("session-1", "old-server"));
        assert!(app.mcp.servers.is_empty());
        handle_client_event(&mut app, handoff_mcp("replacement", "reusable-server"));
        assert_eq!(app.mcp.servers[0].name, "reusable-server");
        press(&mut app, KeyCode::Char('!'));
        crate::app::input_submit::submit_input(&mut app);
        expect_session_prompt(
            &mut rx,
            "replacement",
            &format!("{}!", restored.unwrap_or("")),
            &[],
            &[],
        );
    }
}

fn handoff_target(uuid: &str, text: &str) -> model::RewindTarget {
    model::RewindTarget {
        uuid: uuid.into(),
        first_text: text.into(),
        input_text: text.into(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }
}
fn request_picker_targets(
    app: &mut App,
    rx: &mut crate::agent::client::CommandReceiver,
    session: &str,
) {
    app.recent_sessions = vec![crate::app::RecentSessionInfo {
        session_id: session.into(),
        summary: "Handoff fixture".into(),
        last_modified_ms: 1,
        file_size_bytes: 1,
        cwd: None,
        git_branch: None,
        custom_title: None,
        first_prompt: None,
    }];
    crate::app::session_picker::open(app);
    crate::app::test_support::press(app, KeyCode::Right);
    assert!(
        matches!(rx.try_recv().unwrap().command,crate::agent::wire::BridgeCommand::GetRewindTargets { session_id } if session_id==session)
    );
    assert!(rx.try_recv().is_err());
}
fn picker_rows(app: &mut App) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 25)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .chunks(100)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}
fn deliver_picker_target(app: &mut App, session: &str, uuid: &str, text: &str) {
    handle_client_event(
        app,
        ClientEvent::RewindTargetsReceived {
            session_id: session.into(),
            targets: vec![handoff_target(uuid, text)],
            error: None,
        },
    );
}
fn activate_picker_target(
    app: &mut App,
    rx: &mut crate::agent::client::CommandReceiver,
    session: &str,
    uuid: &str,
) -> String {
    crate::app::test_support::press(app, KeyCode::Enter);
    let envelope = rx.try_recv().expect("selected fork command");
    assert!(
        matches!(envelope.command,crate::agent::wire::BridgeCommand::ResumeSessionAt { session_id, target_user_message_id, .. } if session_id==session && target_user_message_id==uuid)
    );
    envelope.request_id.expect("operation correlation")
}

#[test]
fn picker_handoff_rejects_old_results_then_renders_and_activates_new_target() {
    use crate::app::test_support::*;
    let directory = tempfile::tempdir().unwrap();
    let (mut app, mut rx) = app_with_connection();
    request_picker_targets(&mut app, &mut rx, "session-1");
    deliver_picker_target(&mut app, "session-1", "old-target", "Old selectable source");
    assert!(picker_rows(&mut app).iter().any(|r| r.contains("Old selectable source")));
    request_picker_targets(&mut app, &mut rx, "session-1");
    handle_client_event(
        &mut app,
        session_replacement("replacement", directory.path(), vec![], None),
    );
    drain_session_refreshes(&mut rx, "replacement");
    request_picker_targets(&mut app, &mut rx, "replacement");
    deliver_picker_target(&mut app, "session-1", "old-target", "Old selectable source");
    assert!(!picker_rows(&mut app).iter().any(|r| r.contains("Old selectable source")));
    assert!(app.sdk_inventory.rewind_targets_in_flight);
    deliver_picker_target(&mut app, "replacement", "new-target", "New selectable source");
    assert!(picker_rows(&mut app).iter().any(|r| r.contains("New selectable source")));
    activate_picker_target(&mut app, &mut rx, "replacement", "new-target");
}

#[test]
fn picker_target_failure_is_local_and_retry_can_activate_a_turn() {
    let (mut app, mut rx) = crate::app::test_support::app_with_connection();
    request_picker_targets(&mut app, &mut rx, "listed-session");
    handle_client_event(
        &mut app,
        ClientEvent::RewindTargetsReceived {
            session_id: "listed-session".into(),
            targets: vec![],
            error: Some("History fixture unavailable".into()),
        },
    );
    assert!(picker_rows(&mut app).iter().any(|r| r.contains("History fixture unavailable")));
    assert!(!canonical_messages_contain_text(&app, "History fixture unavailable"));
    crate::app::test_support::press(&mut app, KeyCode::Left);
    request_picker_targets(&mut app, &mut rx, "listed-session");
    deliver_picker_target(&mut app, "listed-session", "retry-target", "Recovered target source");
    activate_picker_target(&mut app, &mut rx, "listed-session", "retry-target");
}

#[test]
fn resume_at_failure_settles_only_the_requested_source_and_operation() {
    use crate::app::test_support::*;
    let (mut app, mut rx) = app_with_connection();
    request_picker_targets(&mut app, &mut rx, "listed-session");
    deliver_picker_target(&mut app, "listed-session", "selected-target", "Selected source");
    let operation = activate_picker_target(&mut app, &mut rx, "listed-session", "selected-target");
    app.input.set_text("Next unsent source");
    for (session, id) in
        [("wrong-session", operation.as_str()), ("listed-session", "wrong-operation")]
    {
        handle_client_event(
            &mut app,
            ClientEvent::SessionResumeFailed {
                session_id: session.into(),
                operation_id: id.into(),
                message: "STALE failure".into(),
            },
        );
        assert_eq!(app.pending_resume_at_operation_id(), Some(operation.as_str()));
        assert!(!canonical_messages_contain_text(&app, "STALE failure"));
        assert!(!app.composer_access().can_submit());
    }
    handle_client_event(
        &mut app,
        ClientEvent::SessionResumeFailed {
            session_id: "listed-session".into(),
            operation_id: operation,
            message: "Selected conversation changed".into(),
        },
    );
    assert!(canonical_messages_contain_text(&app, "Selected conversation changed"));
    assert!(app.pending_session_resume.is_none());
    assert!(app.composer_access().can_submit());
    assert_eq!(app.input.text(), "Next unsent source");
    crate::app::input_submit::submit_input(&mut app);
    expect_session_prompt(&mut rx, "session-1", "Next unsent source", &[], &[]);
}

#[test]
fn slash_target_failure_preserves_an_actual_running_turn() {
    use crate::app::test_support::*;
    let (mut app, mut rx) = app_with_connection();
    app.input.set_text("Running source");
    crate::app::input_submit::submit_input(&mut app);
    expect_session_prompt(&mut rx, "session-1", "Running source", &[], &[]);
    deliver_update(&mut app, source_chunk("Still working", "running-answer", false));
    let owner = app.active_turn_assistant_idx();
    app.input.set_text("/rewind ");
    crate::app::slash::sync_with_cursor(&mut app);
    assert!(
        matches!(rx.try_recv().unwrap().command,crate::agent::wire::BridgeCommand::GetRewindTargets {session_id} if session_id=="session-1")
    );
    handle_client_event(
        &mut app,
        ClientEvent::RewindTargetsReceived {
            session_id: "session-1".into(),
            targets: vec![],
            error: Some("Target history unavailable".into()),
        },
    );
    assert_eq!(app.active_turn_assistant_idx(), owner);
    assert!(matches!(app.status, AppStatus::Running));
    assert!(canonical_messages_contain_text(&app, "Target history unavailable"));
    assert_eq!(app.input.text(), "/rewind ");
    deliver_update(&mut app, source_chunk(" and continued", "running-answer", false));
    assert_eq!(app.active_turn_assistant_idx(), owner);
    let shown = transcript_region(&mut app, owner.unwrap());
    assert!(shown.contains("Still working") && shown.contains("and continued"), "{shown}");
}

#[test]
fn replacement_closes_old_permission_and_new_session_input_remains_usable() {
    use crate::app::test_support::*;
    let directory = tempfile::tempdir().unwrap();
    let (mut app, mut rx) = app_with_connection();
    app.session_runtime.activate_session(model::SessionId::new("session-1"));
    deliver_update(
        &mut app,
        model::SessionUpdate::ConversationReset {
            new_conversation_id: "old-conversation".into(),
            trigger: None,
            timestamp: None,
            user_message_uuid: None,
        },
    );
    assert_eq!(app.session_runtime.conversation_id.as_deref(), Some("old-conversation"));
    app.input.set_text("Old permission turn");
    crate::app::input_submit::submit_input(&mut app);
    expect_session_prompt(&mut rx, "session-1", "Old permission turn", &[], &[]);
    admit_tool(
        &mut app,
        model::ToolCall::new("old-tool", "Old command approval")
            .status(model::ToolCallStatus::InProgress)
            .meta(serde_json::json!({"claudeCode":{"toolName":"Bash"}})),
    );
    let mut response = request_permission(
        &mut app,
        model::RequestPermissionRequest::new(
            "session-1",
            model::ToolCallUpdate::new("old-tool", model::ToolCallUpdateFields::new()),
            permission_options("old-tool"),
            None,
        ),
    );
    assert_eq!(app.focus_owner(), FocusOwner::Permission);
    assert_pending(&mut response);
    handle_client_event(
        &mut app,
        session_replacement("replacement", directory.path(), vec![], None),
    );
    drain_session_refreshes(&mut rx, "replacement");
    assert_eq!(
        app.session_runtime.session_id.as_ref().map(model::SessionId::as_str),
        Some("replacement")
    );
    assert!(app.session_runtime.conversation_id.is_none());
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "session-1".into(),
            update: model::SessionUpdate::ConversationReset {
                new_conversation_id: "stale-conversation".into(),
                trigger: None,
                timestamp: None,
                user_message_uuid: None,
            },
        },
    );
    assert!(
        app.session_runtime.conversation_id.is_none(),
        "old session cannot reintroduce its conversation"
    );
    assert!(matches!(response.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Closed)));
    assert!(app.lookup_tool_call("old-tool").is_none());
    press(&mut app, KeyCode::Char('x'));
    crate::app::input_submit::submit_input(&mut app);
    expect_session_prompt(&mut rx, "replacement", "x", &[], &[]);
}

#[test]
fn replacement_releases_old_rewind_requests_for_new_session_slash_completion() {
    use crate::app::test_support::*;
    for cached in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (mut app, mut rx) = app_with_connection();
        app.input.set_text("/rewind ");
        press(&mut app, KeyCode::Tab);
        assert!(
            matches!(rx.try_recv().unwrap().command, crate::agent::wire::BridgeCommand::GetRewindTargets { session_id } if session_id == "session-1")
        );
        if cached {
            deliver_picker_target(&mut app, "session-1", "old-target", "Old rewind source");
            assert!(
                app.slash
                    .visible()
                    .unwrap()
                    .candidates
                    .iter()
                    .any(|c| c.insert_value == "old-target")
            );
        }
        handle_client_event(
            &mut app,
            session_replacement("replacement", directory.path(), vec![], None),
        );
        drain_session_refreshes(&mut rx, "replacement");
        app.input.set_text("/rewind ");
        press(&mut app, KeyCode::Tab);
        assert!(
            matches!(rx.try_recv().expect("old in-flight work must not block new-session completion").command, crate::agent::wire::BridgeCommand::GetRewindTargets { session_id } if session_id == "replacement")
        );
        deliver_picker_target(&mut app, "session-1", "old-target", "Old rewind source");
        assert!(
            !app.slash
                .visible()
                .is_some_and(|s| s.candidates.iter().any(|c| c.insert_value == "old-target"))
        );
        deliver_picker_target(&mut app, "replacement", "new-target", "New rewind source");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), "/rewind new-target ");
        assert!(rx.try_recv().is_err(), "completion alone must not execute a rewind");
        handle_terminal_event(&mut app, Event::Paste("code".into()));
        crate::app::finalize_pending_paste_event(&mut app);
        crate::app::input_submit::submit_input(&mut app);
        assert!(
            matches!(rx.try_recv().expect("explicit new-session rewind").command, crate::agent::wire::BridgeCommand::Rewind { session_id, target_user_message_id, restore_mode:crate::agent::types::RewindRestoreMode::Code, .. } if session_id == "replacement" && target_user_message_id == "new-target")
        );
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn picker_list_and_reader_activation_carry_the_selected_user_and_launch_options() {
    use crate::app::test_support::*;
    use clap::Parser;
    for reading in [false, true] {
        let (mut app, mut rx) = app_with_connection();
        let cli = crate::Cli::try_parse_from([
            "claude-rs",
            "--resume",
            "--model",
            "haiku",
            "--name",
            "Chosen fork",
        ])
        .unwrap();
        app.startup = crate::app::state::StartupState::from_cli(&cli);
        app.startup.request_connection();
        assert!(app.startup.mark_connection_started());
        app.startup.mark_recent_sessions_loaded();
        request_picker_targets(&mut app, &mut rx, "listed-source");
        let mut chosen = handoff_target("chosen-user", "Chosen message source");
        chosen.previous_assistant_uuid = Some("previous-assistant".into());
        chosen.resume_anchor_uuid = Some("different-anchor".into());
        handle_client_event(
            &mut app,
            ClientEvent::RewindTargetsReceived {
                session_id: "listed-source".into(),
                targets: vec![handoff_target("other-user", "Other source"), chosen],
                error: None,
            },
        );
        press(&mut app, KeyCode::Down);
        if reading {
            press(&mut app, KeyCode::Right);
        }
        let shown = picker_rows(&mut app);
        let heading = shown.iter().position(|r| r.contains("> Message 1")).unwrap();
        assert!(shown[heading + 1].contains("Chosen message source"));
        assert!(rx.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        let envelope = rx.try_recv().expect("confirmed fork");
        let crate::agent::wire::BridgeCommand::ResumeSessionAt {
            session_id,
            target_user_message_id,
            launch_settings,
        } = envelope.command
        else {
            panic!("ResumeSessionAt");
        };
        assert_eq!(session_id, "listed-source");
        assert_eq!(target_user_message_id, "chosen-user");
        assert_eq!(launch_settings.model.as_deref(), Some("haiku"));
        assert_eq!(launch_settings.name.as_deref(), Some("Chosen fork"));
        assert!(envelope.request_id.as_ref().is_some_and(|id| !id.is_empty()));
        assert_eq!(envelope.request_id.as_deref(), app.pending_resume_at_operation_id());
        assert_eq!(app.pending_session_resume_id(), Some("listed-source"));
        assert!(rx.try_recv().is_err());
        assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
    }
}

#[test]
fn reported_dynamic_removal_is_reversed_by_a_contradictory_authoritative_snapshot() {
    use crate::agent::types::{McpSetServersResult, McpSnapshotSource};
    use crate::app::config::mcp::{McpConfigScope, remove_mcp_server_from_config};
    let mut app = make_test_app();
    app.session_runtime.session_id = Some("guard-session".into());
    let (conn, mut rx) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(conn));
    let target = model::McpServerStatus {
        name: "chosen".into(),
        status: model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: Some("user".into()),
        source: Some("dynamic".into()),
        tools: vec![],
    };
    app.mcp.servers = vec![target.clone()];
    remove_mcp_server_from_config(&mut app, "chosen", McpConfigScope::Dynamic);
    assert!(matches!(
        rx.try_recv().unwrap().command,
        crate::agent::wire::BridgeCommand::McpSetServers { .. }
    ));
    handle_client_event(
        &mut app,
        ClientEvent::McpSetServersResult {
            session_id: "guard-session".into(),
            result: McpSetServersResult { removed: vec!["chosen".into()], ..Default::default() },
        },
    );
    assert!(app.mcp.servers.is_empty());
    for (session, source, expected_present) in [
        ("stale-session", McpSnapshotSource::McpSetServers, false),
        ("guard-session", McpSnapshotSource::McpStatus, false),
        ("guard-session", McpSnapshotSource::McpSetServers, true),
    ] {
        handle_client_event(
            &mut app,
            ClientEvent::McpSnapshotReceived {
                session_id: session.into(),
                servers: vec![target.clone()],
                auth_capabilities: model::McpAuthCapabilities::default(),
                source: Some(source),
                error: None,
            },
        );
        assert_eq!(app.mcp.servers.contains(&target), expected_present);
        assert_eq!(app.mcp.removed_config_servers.is_empty(), expected_present);
    }
    let error = app.mcp.last_error.as_ref().unwrap();
    assert!(error.contains("chosen") && error.contains("confirming"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn lifecycle_reconnection_keeps_the_welcome_settled_after_the_first_hello() {
    use crate::app::HistoryOutputId;
    use crate::app::test_support::{EventClock, output_region, welcome_art};
    use crate::ui::inline_chat_rows::serialize_live_rows_at;
    let clock = EventClock::new();
    clock.at(0);
    let mut app = make_test_app();
    let mut settled_art = None;
    for (seconds, model_name) in [(0, "first-model"), (60, "later-model")] {
        clock.at(seconds);
        handle_client_event(&mut app, connected_event(model_name));
        assert_eq!(
            app.session_runtime.current_model.as_ref().unwrap().resolved_id,
            model_name,
            "the later connection must actually be applied"
        );
        handle_client_event(
            &mut app,
            ClientEvent::StatusSnapshotReceived {
                session_id: "test-session".into(),
                account: model::AccountInfo {
                    email: None,
                    organization: None,
                    subscription_type: Some("Orchid".into()),
                    token_source: None,
                    api_key_source: None,
                    api_provider: None,
                },
            },
        );
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "prefersReducedMotion",
            serde_json::json!(false),
        ));
        let id = app
            .transcript
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .find_map(|block| match block {
                MessageBlock::Welcome(welcome) => Some(HistoryOutputId::Block(welcome.id)),
                _ => None,
            })
            .expect("current welcome identity");
        for offset in [100, 2_000] {
            let at = clock.at_millis(seconds * 1000 + offset);
            let rows = serialize_live_rows_at(
                &mut app,
                120,
                &std::collections::BTreeSet::new(),
                at,
                &crate::ui::layout_control::LayoutControl::uninterrupted(),
            );
            let (segment, text) = output_region(&rows, &id);
            assert!(
                text.iter().any(|row| row.contains("Session ID:") && row.contains("test-session"))
            );
            assert_eq!(segment.commit_ready, seconds != 0 || offset == 2_000);
            let art = welcome_art(&text);
            assert!(!art.is_empty());
            if seconds == 0 && offset == 2_000 {
                settled_art = Some(art);
            } else if seconds != 0 {
                assert_eq!(Some(art), settled_art);
            }
        }
    }
}
