use super::*;
use pretty_assertions::assert_eq;

#[test]
fn connected_updates_welcome_session_id_while_pristine() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first_mut() else {
        panic!("expected welcome block");
    };
    welcome.tip_seed = 7;

    handle_client_event(&mut app, connected_event("claude-updated"));

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing welcome message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "test-session");
    assert_eq!(welcome.tip_seed, 7);
}

#[test]
fn connected_session_preserves_inline_viewport_for_startup_welcome_transition() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    app.surface_dirty.chat.rebuild = ChatRebuildKind::None;
    app.surface_dirty.chat.repaint = false;

    handle_client_event(&mut app, connected_event("claude-updated"));

    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::None);
    assert!(app.surface_dirty.chat.repaint);
}

#[test]
fn connected_keeps_subscription_placeholder_until_status_snapshot_arrives() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "old",
        "/test",
        "old",
    ));

    handle_client_event(&mut app, connected_event("opus"));

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing welcome message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.subscription, "-");
}

#[test]
fn connected_requests_mcp_snapshot_even_outside_mcp_tab() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.config.active_tab = crate::app::config::ConfigTab::Status;
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "supabase".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new(
            "user".to_owned(),
            "supabase".to_owned(),
        ),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(&mut app, connected_event("claude-updated"));

    let envelope = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        envelope.command,
        crate::agent::wire::BridgeCommand::GetMcpSnapshot { session_id: "test-session".to_owned() }
    );
    assert!(app.mcp.in_flight);
    assert!(app.mcp.servers.is_empty());
    assert!(app.mcp.removed_config_servers.is_empty());
}

#[test]
fn connected_updates_cwd_and_clears_resuming_marker() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    app.resuming_session_id = Some("resume-123".into());

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
            history_updates: Vec::new(),
        },
    );

    assert_eq!(app.cwd_raw, "/changed");
    assert_eq!(app.cwd, "/changed");
    assert!(app.resuming_session_id.is_none());
    let Some(first) = app.transcript.messages.first() else {
        panic!("missing welcome message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.cwd, "/changed");
}

#[test]
fn connected_reconciles_trust_for_new_cwd() {
    let mut app = make_test_app();
    app.trust.status = crate::app::trust::TrustStatus::Trusted;
    app.config.committed_preferences_document = serde_json::json!({
        "projects": {}
    });

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("session-trust"),
            cwd: "/untrusted".into(),
            current_model: test_current_model("claude-updated"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            history_updates: Vec::new(),
        },
    );

    assert_eq!(app.trust.status, crate::app::trust::TrustStatus::Untrusted);
    assert_eq!(
        app.trust.project_key,
        crate::app::trust::store::normalize_project_key(std::path::Path::new("/untrusted"))
    );
}

#[test]
fn connected_updates_welcome_once_even_after_chat_started() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first_mut() else {
        panic!("expected welcome block");
    };
    welcome.tip_seed = 11;
    app.transcript.messages.push(user_msg("hello"));

    handle_client_event(&mut app, connected_event("claude-updated"));

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing first message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "test-session");
    assert_eq!(welcome.tip_seed, 11);
}

#[test]
fn current_model_update_does_not_mutate_welcome_snapshot_after_settings_reconcile() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.session_runtime.current_model = Some(test_current_model("opus"));
    app.transcript.messages =
        vec![ChatMessage::welcome(env!("CARGO_PKG_VERSION"), "-", "/test", "session-1")];
    crate::app::config::store::set_model(&mut app.config.committed_settings_document, Some("opus"));

    crate::app::config::store::set_model(
        &mut app.config.committed_settings_document,
        Some("haiku"),
    );
    app.reconcile_runtime_from_persisted_settings_change();

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
            test_current_model("claude-opus-4-7"),
        ))),
    );

    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "session-1");
    assert_eq!(welcome.subscription, "-");
}

#[test]
fn connected_resets_session_scoped_view_data() {
    let mut app = make_test_app();
    app.transcript.messages.push(user_msg("hello"));
    app.status = AppStatus::Running;
    app.files_accessed = 9;
    app.usage.snapshot = Some(UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: std::time::SystemTime::now(),
        five_hour: None,
        seven_day: None,
        seven_day_opus: None,
        seven_day_sonnet: None,
        extra_usage: None,
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

    handle_client_event(&mut app, connected_event("claude-updated"));

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert_eq!(app.files_accessed, 0);
    assert!(app.usage.snapshot.is_none());
    assert!(app.session_runtime.account_info.is_none());
    assert!(app.plugins.installed.is_empty());
    assert!(app.plugins.last_inventory_refresh_at.is_none());
    assert!(app.config.pending_session_title_change.is_none());
}

#[test]
fn current_model_update_leaves_existing_welcome_snapshot_unchanged() {
    let mut app = make_test_app();
    app.session_runtime.current_model = Some(test_current_model("opus"));
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    app.transcript.messages.push(user_msg("hello"));

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
            test_current_model("claude-opus-4-7"),
        ))),
    );

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing first message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "-");

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
            test_current_model("claude-sonnet-4-5"),
        ))),
    );

    let Some(first) = app.transcript.messages.first() else {
        panic!("missing first message");
    };
    let Some(MessageBlock::Welcome(welcome)) = first.blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.session_id, "-");
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
fn update_available_records_settings_without_transcript_message() {
    let mut app = make_test_app();
    assert!(app.global_settings.updates.last_result.is_none());

    handle_client_event(
        &mut app,
        ClientEvent::UpdateAvailable {
            latest_version: "0.3.0".into(),
            current_version: "0.2.0".into(),
        },
    );

    assert!(app.transcript.messages.is_empty());
    let Some(last_result) = app.global_settings.updates.last_result.as_ref() else {
        panic!("expected update result");
    };
    assert_eq!(last_result.current_version, "0.2.0");
    assert_eq!(last_result.latest_version, "0.3.0");
    assert_eq!(
        last_result.release_url,
        "https://github.com/srothgan/claude-code-rust/releases/tag/v0.3.0"
    );
}

#[test]
fn service_status_warning_pushes_system_warning_without_locking_input() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::ServiceStatus {
            severity: ServiceStatusSeverity::Warning,
            message: "Claude Code status: Partial Outage (indicator: minor).".into(),
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Warning))));
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
fn service_status_error_pushes_system_error_without_locking_input() {
    let mut app = make_test_app();
    app.input.set_text("draft stays");

    handle_client_event(
        &mut app,
        ClientEvent::ServiceStatus {
            severity: ServiceStatusSeverity::Error,
            message: "Claude Code status: Major Outage (indicator: major).".into(),
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.input.text(), "draft stays");
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Error))));
}

#[test]
fn session_replaced_resets_chat_and_transient_state() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "-",
    ));
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first_mut() else {
        panic!("expected welcome block");
    };
    welcome.tip_seed = 5;
    app.transcript.messages.push(user_msg("hello"));
    app.transcript
        .messages
        .push(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete("world"))]));
    app.status = AppStatus::Running;
    app.files_accessed = 9;
    app.turn.pending_interaction_ids.push("perm-1".into());
    app.sdk_inventory.tasks.push(task_item("task-1", "Task", model::TaskStatus::InProgress));
    app.mention = Some(mention::MentionState::new(0, 0, String::new(), Vec::new()));
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "supabase".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(
        app.session_runtime.session_id.as_ref().map(ToString::to_string).as_deref(),
        Some("replacement")
    );
    assert_eq!(
        app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
        Some("new-model")
    );
    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert_eq!(app.files_accessed, 0);
    assert!(app.turn.pending_interaction_ids.is_empty());
    assert!(app.sdk_inventory.tasks.is_empty());
    assert!(app.mention.is_none());
    assert!(app.mcp.servers.is_empty());
    assert!(app.mcp.removed_config_servers.is_empty());
    assert_eq!(app.cwd_raw, "/replacement");
    assert_eq!(app.cwd, "/replacement");
    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.cwd, "/replacement");
    assert_ne!(welcome.tip_seed, 5);
    assert_eq!(
        app.surface_dirty.chat.rebuild,
        ChatRebuildKind::PurgeReplay(crate::app::ChatPurgeReplayOptions::session_replacement())
    );
}

#[test]
fn session_replaced_requests_mcp_snapshot_even_outside_mcp_tab() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.config.active_tab = crate::app::config::ConfigTab::Status;
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "supabase".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("replacement"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    let envelope = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        envelope.command,
        crate::agent::wire::BridgeCommand::GetMcpSnapshot { session_id: "replacement".to_owned() }
    );
    assert!(app.mcp.in_flight);
    assert!(app.mcp.servers.is_empty());
}

#[test]
fn connected_requests_status_snapshot_on_connect() {
    let (mut app, mut rx) = app_with_bridge_connection();

    handle_client_event(&mut app, connected_event("claude-updated"));

    let mcp = rx.try_recv().expect("mcp snapshot command");
    assert_eq!(
        mcp.command,
        crate::agent::wire::BridgeCommand::GetMcpSnapshot { session_id: "test-session".to_owned() }
    );
    let status = rx.try_recv().expect("status snapshot command");
    assert_eq!(
        status.command,
        crate::agent::wire::BridgeCommand::GetStatusSnapshot {
            session_id: "test-session".to_owned(),
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn connected_requests_usage_refresh_when_usage_tab_is_open() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = make_test_app();
            app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
            app.config.active_tab = crate::app::ConfigTab::Usage;

            handle_client_event(&mut app, connected_event("claude-updated"));

            assert!(app.usage.in_flight);
        })
        .await;
}

#[test]
fn stale_status_snapshot_for_old_session_is_ignored() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "old-session".into(),
            account: crate::agent::model::AccountInfo {
                email: Some("old@example.com".into()),
                organization: None,
                subscription_type: None,
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    assert!(app.session_runtime.account_info.is_none());
}

#[test]
fn stale_session_update_is_rejected_before_dispatch() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "old-session".to_owned(),
            update: model::SessionUpdate::FastModeUpdate {
                state: model::FastModeState::Cooldown,
                disabled_reason: None,
            },
        },
    );

    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Off);
}

#[test]
fn matching_session_update_is_dispatched() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "current-session".to_owned(),
            update: model::SessionUpdate::FastModeUpdate {
                state: model::FastModeState::Cooldown,
                disabled_reason: None,
            },
        },
    );

    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Cooldown);
}

#[test]
fn connected_snapshot_recovers_fast_mode_dropped_before_authority() {
    let mut app = make_test_app();

    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "new-session".to_owned(),
            update: model::SessionUpdate::FastModeUpdate {
                state: model::FastModeState::On,
                disabled_reason: None,
            },
        },
    );
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Off);

    handle_client_event(
        &mut app,
        ClientEvent::Connected {
            session_id: model::SessionId::new("new-session"),
            cwd: "/test".into(),
            current_model: test_current_model("claude"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::On,
            fast_mode_disabled_reason: None,
            history_updates: Vec::new(),
        },
    );

    assert_eq!(
        app.session_runtime.session_id.as_ref().map(model::SessionId::as_str),
        Some("new-session")
    );
    assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::On);
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
fn stale_turn_completion_cannot_finish_current_turn() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.status = AppStatus::Running;

    handle_client_event(
        &mut app,
        ClientEvent::TurnComplete { session_id: "old-session".to_owned(), terminal_reason: None },
    );

    assert!(matches!(app.status, AppStatus::Running));
}

#[test]
fn status_snapshot_updates_welcome_subscription() {
    let mut app = make_test_app();
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "session-1",
    ));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "session-1".into(),
            account: crate::agent::model::AccountInfo {
                email: None,
                organization: None,
                subscription_type: Some("Claude Max".into()),
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.subscription, "Claude Max");
    assert!(session_overview_has_welcome(&app));
}

#[test]
fn status_snapshot_does_not_commit_welcome_when_session_overview_is_suppressed() {
    let mut app = make_test_app();
    app.show_session_overview = false;
    app.transcript.messages.push(ChatMessage::welcome(
        env!("CARGO_PKG_VERSION"),
        "-",
        "/test",
        "session-1",
    ));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "session-1".into(),
            account: crate::agent::model::AccountInfo {
                email: None,
                organization: None,
                subscription_type: Some("Claude Max".into()),
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    let Some(MessageBlock::Welcome(welcome)) = app.transcript.messages[0].blocks.first() else {
        panic!("expected welcome block");
    };
    assert_eq!(welcome.subscription, "Claude Max");
    assert!(!session_overview_has_welcome(&app));
}

#[test]
fn stale_mcp_snapshot_for_old_session_is_ignored() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.servers.push(crate::agent::model::McpServerStatus {
        name: "current".into(),
        status: crate::agent::model::McpServerConnectionStatus::Connected,
        server_info: None,
        error: None,
        config: None,
        scope: None,
        tools: Vec::new(),
    });

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "old-session".into(),
            auth_capabilities: crate::agent::model::McpAuthCapabilities::default(),
            servers: vec![crate::agent::model::McpServerStatus {
                name: "stale".into(),
                status: crate::agent::model::McpServerConnectionStatus::Connected,
                server_info: None,
                error: None,
                config: None,
                scope: None,
                tools: Vec::new(),
            }],
            source: Some(crate::agent::types::McpSnapshotSource::McpStatus),
            error: None,
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "current");
}

#[test]
fn removed_config_mcp_server_is_filtered_from_current_session_snapshot() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new("user".to_owned(), "notion".to_owned()),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "current-session".into(),
            auth_capabilities: crate::agent::model::McpAuthCapabilities {
                authenticate: true,
                clear_auth: true,
                submit_oauth_callback_url: true,
            },
            servers: vec![
                crate::agent::model::McpServerStatus {
                    name: "notion".into(),
                    status: crate::agent::model::McpServerConnectionStatus::Connected,
                    server_info: None,
                    error: None,
                    config: None,
                    scope: Some("user".into()),
                    tools: Vec::new(),
                },
                crate::agent::model::McpServerStatus {
                    name: "fff".into(),
                    status: crate::agent::model::McpServerConnectionStatus::Connected,
                    server_info: None,
                    error: None,
                    config: None,
                    scope: Some("user".into()),
                    tools: Vec::new(),
                },
            ],
            source: Some(crate::agent::types::McpSnapshotSource::McpStatus),
            error: None,
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "fff");
    assert_eq!(app.mcp.removed_config_servers.len(), 1);
    assert_eq!(
        app.mcp.auth_capabilities,
        crate::agent::model::McpAuthCapabilities {
            authenticate: true,
            clear_auth: true,
            submit_oauth_callback_url: true,
        }
    );
}

#[test]
fn removed_config_mcp_guard_clears_after_matching_source_snapshot_proves_absence() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new("user".to_owned(), "notion".to_owned()),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "current-session".into(),
            auth_capabilities: crate::agent::model::McpAuthCapabilities::default(),
            servers: vec![crate::agent::model::McpServerStatus {
                name: "fff".into(),
                status: crate::agent::model::McpServerConnectionStatus::Connected,
                server_info: None,
                error: None,
                config: None,
                scope: Some("user".into()),
                tools: Vec::new(),
            }],
            source: Some(crate::agent::types::McpSnapshotSource::ReloadPlugins),
            error: None,
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "fff");
    assert!(app.mcp.removed_config_servers.is_empty());
}

#[test]
fn removed_config_mcp_guard_stays_after_matching_source_snapshot_error() {
    let mut app = make_test_app();
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.mcp.removed_config_servers.insert(
        crate::app::state::types::RemovedMcpServerKey::new("user".to_owned(), "notion".to_owned()),
        crate::app::state::types::RemovedMcpServerGuard {
            expected_source: crate::agent::types::McpSnapshotSource::ReloadPlugins,
        },
    );

    handle_client_event(
        &mut app,
        ClientEvent::McpSnapshotReceived {
            session_id: "current-session".into(),
            auth_capabilities: crate::agent::model::McpAuthCapabilities::default(),
            servers: vec![crate::agent::model::McpServerStatus {
                name: "fff".into(),
                status: crate::agent::model::McpServerConnectionStatus::Connected,
                server_info: None,
                error: None,
                config: None,
                scope: Some("user".into()),
                tools: Vec::new(),
            }],
            source: Some(crate::agent::types::McpSnapshotSource::ReloadPlugins),
            error: Some("reload failed".to_owned()),
        },
    );

    assert_eq!(app.mcp.servers.len(), 1);
    assert_eq!(app.mcp.servers[0].name, "fff");
    assert_eq!(app.mcp.removed_config_servers.len(), 1);
}

#[test]
fn stale_usage_refresh_result_for_old_epoch_is_ignored() {
    let mut app = make_test_app();
    app.session_runtime.session_scope_epoch = 5;

    handle_client_event(
        &mut app,
        ClientEvent::UsageSnapshotReceived {
            epoch: 4,
            snapshot: UsageSnapshot {
                source: UsageSourceKind::Oauth,
                fetched_at: std::time::SystemTime::now(),
                five_hour: None,
                seven_day: None,
                seven_day_opus: None,
                seven_day_sonnet: None,
                extra_usage: None,
            },
        },
    );

    assert!(app.usage.snapshot.is_none());
}

#[test]
fn pending_limits_response_prints_summary_on_usage_refresh_success() {
    let mut app = make_test_app();
    app.session_runtime.session_scope_epoch = 5;
    app.usage.pending_limits_response = true;

    handle_client_event(
        &mut app,
        ClientEvent::UsageSnapshotReceived {
            epoch: 5,
            snapshot: UsageSnapshot {
                source: UsageSourceKind::Oauth,
                fetched_at: SystemTime::now(),
                five_hour: Some(UsageWindow {
                    label: "5-hour",
                    utilization: 47.0,
                    resets_at: None,
                    reset_description: Some("resets in 2h 14m".to_owned()),
                }),
                seven_day: Some(UsageWindow {
                    label: "7-day",
                    utilization: 62.0,
                    resets_at: None,
                    reset_description: Some("resets in 4d 11h".to_owned()),
                }),
                seven_day_opus: None,
                seven_day_sonnet: None,
                extra_usage: None,
            },
        },
    );

    assert!(!app.usage.pending_limits_response);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected limits summary");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| 5-hour | 47% | resets in 2h 14m |"));
    assert!(block.text.contains("| 7-day | 62% | resets in 4d 11h |"));
}

#[test]
fn pending_limits_response_prints_error_on_usage_refresh_failure() {
    let mut app = make_test_app();
    app.session_runtime.session_scope_epoch = 5;
    app.usage.pending_limits_response = true;

    handle_client_event(
        &mut app,
        ClientEvent::UsageRefreshFailed {
            epoch: 5,
            message: "network timeout".to_owned(),
            source: UsageSourceKind::Oauth,
        },
    );

    assert!(!app.usage.pending_limits_response);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected limits error");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Unable to get recent usage info: network timeout");
}

#[test]
fn stale_usage_refresh_result_does_not_print_pending_limits_response() {
    let mut app = make_test_app();
    app.session_runtime.session_scope_epoch = 5;
    app.usage.pending_limits_response = true;
    let message_count = app.transcript.messages.len();

    handle_client_event(
        &mut app,
        ClientEvent::UsageSnapshotReceived {
            epoch: 4,
            snapshot: UsageSnapshot {
                source: UsageSourceKind::Oauth,
                fetched_at: SystemTime::now(),
                five_hour: Some(UsageWindow {
                    label: "5-hour",
                    utilization: 47.0,
                    resets_at: None,
                    reset_description: Some("resets in 2h 14m".to_owned()),
                }),
                seven_day: None,
                seven_day_opus: None,
                seven_day_sonnet: None,
                extra_usage: None,
            },
        },
    );

    assert!(app.usage.pending_limits_response);
    assert_eq!(app.transcript.messages.len(), message_count);
}

#[test]
fn stale_plugin_inventory_result_for_old_cwd_is_ignored() {
    let mut app = make_test_app();
    app.cwd_raw = "/current".into();

    handle_client_event(
        &mut app,
        ClientEvent::PluginsInventoryUpdated {
            cwd_raw: "/old".into(),
            snapshot: crate::app::plugins::PluginsInventorySnapshot {
                installed: vec![installed_plugin_entry("stale-plugin")],
                marketplace: Vec::new(),
                marketplaces: Vec::new(),
            },
            claude_path: std::path::PathBuf::from("claude"),
        },
    );

    assert!(app.plugins.installed.is_empty());
}

#[test]
fn slash_command_error_while_resuming_returns_ready_and_clears_marker() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.resuming_session_id = Some("resume-123".into());

    handle_client_event(&mut app, slash_command_error("resume failed".into()));

    assert!(matches!(app.status, AppStatus::Ready));
    assert!(app.resuming_session_id.is_none());
}

#[test]
fn slash_command_error_clears_rewind_target_loading_state() {
    let mut app = make_test_app();
    app.sdk_inventory.rewind_targets_in_flight = true;
    app.sdk_inventory.rewind_targets_session_id = Some(model::SessionId::new("session-1"));
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".into(),
        first_text: "hello".into(),
        input_text: "hello".into(),
        index: 0,
        previous_assistant_uuid: None,
    }];

    handle_client_event(&mut app, slash_command_error("failed to load rewind targets".into()));

    assert!(!app.sdk_inventory.rewind_targets_in_flight);
    assert!(app.sdk_inventory.rewind_targets_session_id.is_none());
}

#[test]
fn slash_command_error_during_running_turn_does_not_stop_turn_status() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.turn.pending_command_label = Some("Switching mode...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentMode);

    handle_client_event(&mut app, slash_command_error("failed to set mode".into()));

    assert!(matches!(app.status, AppStatus::Running));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
}

#[test]
fn slash_command_error_during_active_turn_inserts_inline_notice() {
    let mut app = make_test_app();
    app.status = AppStatus::Running;
    app.transcript.messages.push(assistant_msg(vec![MessageBlock::Text(
        TextBlock::from_complete("streaming answer"),
    )]));
    app.bind_active_turn_assistant(0);
    app.turn.pending_command_label = Some("Switching mode...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentMode);

    handle_client_event(&mut app, slash_command_error("failed to set mode to auto".into()));

    assert!(matches!(app.status, AppStatus::Running));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
    assert_eq!(app.transcript.messages.len(), 1);
    assert!(app.turn.notice_refs.is_empty());
    let [MessageBlock::Text(text), MessageBlock::Notice(notice)] =
        app.transcript.messages[0].blocks.as_slice()
    else {
        panic!("expected assistant text followed by inline notice");
    };
    assert_eq!(text.text, "streaming answer");
    assert_eq!(notice.severity, SystemSeverity::Error);
    assert_eq!(notice.text.text, "failed to set mode to auto");
    assert!(notice.dedup_key.is_none());
}

#[test]
fn slash_command_error_without_active_turn_inserts_standalone_notice() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Switching mode...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentMode);

    handle_client_event(&mut app, slash_command_error("failed to set mode to auto".into()));

    assert!(matches!(app.status, AppStatus::Ready));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
    let Some(ChatMessage {
        role: MessageRole::System(Some(SystemSeverity::Error)), blocks, ..
    }) = app.transcript.messages.last()
    else {
        panic!("expected standalone system notice");
    };
    let [MessageBlock::Notice(notice)] = blocks.as_slice() else {
        panic!("expected standalone notice block");
    };
    assert_eq!(notice.severity, SystemSeverity::Error);
    assert_eq!(notice.text.text, "failed to set mode to auto");
    assert!(notice.dedup_key.is_none());
}

#[test]
fn slash_command_error_during_thinking_turn_does_not_stop_turn_status() {
    let mut app = make_test_app();
    app.status = AppStatus::Thinking;
    app.turn.pending_command_label = Some("Switching model...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentModel);

    handle_client_event(&mut app, slash_command_error("failed to set model".into()));

    assert!(matches!(app.status, AppStatus::Thinking));
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
}

#[test]
fn terminal_release_event_marks_child_process_lifecycle_without_redraw() {
    let mut app = make_test_app();
    app.request_chat_repaint();

    handle_client_event(
        &mut app,
        ClientEvent::TerminalReleasedToChild { reason: ReleaseReason::AuthFlow },
    );

    assert_eq!(
        app.terminal_lifecycle,
        TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow)
    );
    assert!(!app.surface_dirty.chat.repaint);
}

#[test]
fn terminal_events_are_ignored_while_released_to_child_except_resize() {
    let mut app = make_test_app();
    app.terminal_lifecycle = TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow);

    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
    );

    assert_eq!(app.input.text(), "");
}

#[test]
fn terminal_return_event_restores_chat_lifecycle_and_rebuilds_chat() {
    let mut app = make_test_app();
    app.terminal_lifecycle = TerminalLifecycleState::ReleasedToChild(ReleaseReason::AuthFlow);
    app.chat_render.live_region.anchor_valid = true;
    app.surface_dirty.chat.repaint = false;

    handle_client_event(
        &mut app,
        ClientEvent::TerminalReturnedFromChild { reason: ReleaseReason::AuthFlow },
    );

    assert_eq!(app.terminal_lifecycle, TerminalLifecycleState::Running(SurfaceMode::Chat));
    assert!(app.surface_dirty.terminal_mode);
    assert!(!app.chat_render.live_region.anchor_valid);
    assert_eq!(app.surface_dirty.chat.rebuild, ChatRebuildKind::VisibleScreen);
    assert!(app.surface_dirty.chat.repaint);
}

#[test]
fn sessions_listed_completes_pending_session_rename() {
    let mut app = make_test_app();
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "session-1".to_owned(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Rename {
                requested_title: Some("Renamed session".to_owned()),
            },
        });

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![crate::agent::types::SessionListEntry {
                session_id: "session-1".to_owned(),
                summary: "Renamed session".to_owned(),
                last_modified_ms: 1,
                file_size_bytes: 2,
                cwd: Some("/test".to_owned()),
                git_branch: None,
                custom_title: Some("Renamed session".to_owned()),
                first_prompt: Some("prompt".to_owned()),
            }],
        },
    );

    assert!(app.config.pending_session_title_change.is_none());
    assert_eq!(app.config.status_message.as_deref(), Some("Renamed session to Renamed session"));
    assert!(app.config.last_error.is_none());
    assert_eq!(app.recent_sessions.len(), 1);
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

#[test]
fn mcp_operation_error_stays_in_mcp_feedback_and_out_of_chat() {
    let mut app = make_test_app();
    app.config.active_tab = crate::app::config::ConfigTab::Mcp;
    app.config.status_message = Some("Starting MCP auth for claude.ai Google Calendar...".into());
    app.mcp.in_flight = true;

    handle_client_event(
        &mut app,
        ClientEvent::McpOperationError {
            session_id: "test-session".to_owned(),
            error: crate::agent::types::McpOperationError {
                server_name: Some("claude.ai Google Calendar".into()),
                operation: "authenticate".into(),
                message: "Server type \"claudeai-proxy\" does not support OAuth authentication"
                    .into(),
            },
        },
    );

    assert_eq!(
        app.mcp.last_error.as_deref(),
        Some(
            "Failed to authenticate MCP server claude.ai Google Calendar: Server type \"claudeai-proxy\" does not support OAuth authentication"
        )
    );
    assert_eq!(app.config.last_error, app.mcp.last_error);
    assert!(app.config.status_message.is_none());
    assert!(!app.mcp.in_flight);
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn sessions_listed_completes_pending_session_title_generation() {
    let mut app = make_test_app();
    app.config.pending_session_title_change =
        Some(crate::app::config::PendingSessionTitleChangeState {
            session_id: "session-1".to_owned(),
            kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
        });

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![crate::agent::types::SessionListEntry {
                session_id: "session-1".to_owned(),
                summary: "Generated session".to_owned(),
                last_modified_ms: 1,
                file_size_bytes: 2,
                cwd: Some("/test".to_owned()),
                git_branch: None,
                custom_title: Some("Generated session".to_owned()),
                first_prompt: Some("prompt".to_owned()),
            }],
        },
    );

    assert!(app.config.pending_session_title_change.is_none());
    assert_eq!(app.config.status_message.as_deref(), Some("Generated session title"));
    assert!(app.config.last_error.is_none());
}

#[test]
fn startup_picker_waits_for_connected_after_sessions_listed() {
    let mut app = make_test_app();
    app.startup = crate::app::state::StartupState::new(None, None, true);
    app.startup.request_connection();
    assert!(app.startup.mark_connection_started());

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![listed_session("session-1", "First Session")],
        },
    );

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.startup.startup_picker_is_ready());
    assert!(!app.startup.session_picker_resolved());

    let (connection, _rx) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));
    handle_client_event(&mut app, connected_event("claude-updated"));

    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
    assert!(app.startup.session_picker_resolved());
}

#[test]
fn startup_picker_empty_list_stays_in_chat_with_info_message() {
    let mut app = make_test_app();
    app.startup = crate::app::state::StartupState::new(None, None, true);
    app.startup.request_connection();
    assert!(app.startup.mark_connection_started());
    let (connection, _rx) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(Rc::new(connection));

    handle_client_event(&mut app, connected_event("claude-updated"));
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(!app.startup.session_picker_resolved());

    handle_client_event(&mut app, ClientEvent::SessionsListed { sessions: Vec::new() });

    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert!(app.startup.session_picker_resolved());
    let last = app.transcript.messages.last().expect("info message");
    let text = match last.blocks.first().expect("text block") {
        MessageBlock::Text(block) => block.text.as_str(),
        _ => panic!("expected text block"),
    };
    assert!(text.contains("No recent sessions found for this directory"));
}

#[test]
fn sessions_listed_refresh_preserves_picker_selection_by_session_id() {
    let mut app = make_test_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::SessionPicker);
    app.recent_sessions = vec![
        crate::app::RecentSessionInfo {
            session_id: "session-1".to_owned(),
            summary: "First".to_owned(),
            last_modified_ms: 1,
            file_size_bytes: 1,
            cwd: Some("/test".to_owned()),
            git_branch: Some("main".to_owned()),
            custom_title: Some("First".to_owned()),
            first_prompt: Some("prompt one".to_owned()),
        },
        crate::app::RecentSessionInfo {
            session_id: "session-2".to_owned(),
            summary: "Second".to_owned(),
            last_modified_ms: 2,
            file_size_bytes: 1,
            cwd: Some("/test".to_owned()),
            git_branch: Some("main".to_owned()),
            custom_title: Some("Second".to_owned()),
            first_prompt: Some("prompt two".to_owned()),
        },
    ];
    app.session_picker.selected = 1;
    app.session_picker.scroll_offset = 1;

    handle_client_event(
        &mut app,
        ClientEvent::SessionsListed {
            sessions: vec![
                listed_session("session-2", "Second"),
                listed_session("session-3", "Third"),
            ],
        },
    );

    assert_eq!(app.session_picker.selected, 0);
    assert_eq!(app.recent_sessions[app.session_picker.selected].session_id, "session-2");
    assert_eq!(app.session_picker.scroll_offset, 0);
}

#[test]
fn current_model_update_updates_state_and_clears_pending_when_expected() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Switching model...".into());
    app.turn.pending_command_ack = Some(PendingCommandAck::CurrentModel);
    app.session_runtime.current_model = Some(test_current_model("old-model"));

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
            test_current_model("sonnet"),
        ))),
    );

    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(
        app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
        Some("sonnet")
    );
    assert!(app.turn.pending_command_label.is_none());
    assert!(app.turn.pending_command_ack.is_none());
}

#[test]
fn non_matching_config_option_update_keeps_pending() {
    let mut app = make_test_app();
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_label = Some("Switching model...".into());
    app.turn.pending_command_ack =
        Some(PendingCommandAck::ConfigOption { option_id: "model".to_owned() });

    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
            option_id: "max_thinking_tokens".to_owned(),
            value: serde_json::json!(2048),
        })),
    );

    assert!(matches!(app.status, AppStatus::CommandPending));
    assert_eq!(
        app.session_runtime.config_options.get("max_thinking_tokens"),
        Some(&serde_json::json!(2048))
    );
    assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching model..."));
    assert!(matches!(
        app.turn.pending_command_ack.as_ref(),
        Some(PendingCommandAck::ConfigOption { option_id }) if option_id == "model"
    ));
}

#[test]
fn resume_does_not_add_confirmation_system_message() {
    let mut app = make_test_app();
    app.resuming_session_id = Some("requested-123".into());

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-456"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            history_updates: Vec::new(),
            restored_input: None,
        },
    );

    assert_eq!(app.transcript.messages.len(), 1);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert!(app.resuming_session_id.is_none());
    assert!(matches!(app.status, AppStatus::Ready));
}

#[test]
fn resume_history_renders_user_message_chunks() {
    let mut app = make_test_app();
    let history_updates = vec![
        model::SessionUpdate::UserMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("first user line")),
        )),
        model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new("assistant reply")),
        )),
    ];

    handle_client_event(
        &mut app,
        ClientEvent::SessionReplaced {
            session_id: model::SessionId::new("active-456"),
            cwd: "/replacement".into(),
            current_model: test_current_model("new-model"),
            available_models: Vec::new(),
            mode: None,
            fast_mode_state: model::FastModeState::Off,
            fast_mode_disabled_reason: None,
            history_updates,
            restored_input: None,
        },
    );

    assert_eq!(app.transcript.messages.len(), 3);
    assert!(matches!(app.transcript.messages[0].role, MessageRole::Welcome));
    assert!(matches!(app.transcript.messages[1].role, MessageRole::User));
    assert!(matches!(app.transcript.messages[2].role, MessageRole::Assistant));

    let Some(MessageBlock::Text(user_text)) = app.transcript.messages[1].blocks.first() else {
        panic!("expected user text block");
    };
    assert_eq!(user_text.text, "first user line");
    assert!(canonical_messages_contain_text(&app, "first user line"));
    assert!(canonical_messages_contain_text(&app, "assistant reply"));
    assert!(!session_overview_has_welcome(&app));
    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.turn.pending_cancel_origin, None);
    assert!(!app.turn.pending_auto_submit_after_cancel);

    handle_client_event(
        &mut app,
        ClientEvent::StatusSnapshotReceived {
            session_id: "active-456".into(),
            account: crate::agent::model::AccountInfo {
                email: None,
                organization: None,
                subscription_type: Some("Claude Max".into()),
                token_source: None,
                api_key_source: None,
                api_provider: None,
            },
        },
    );

    assert!(!session_overview_has_welcome(&app));
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
            history_updates,
            restored_input: Some("selected prompt".to_owned()),
        },
    );

    assert_eq!(app.input.text(), "selected prompt");
    assert!(app.turn.pending_command_label.is_none());
    assert!(matches!(app.status, AppStatus::Ready));
    assert!(canonical_messages_contain_text(&app, "assistant reply"));
}
