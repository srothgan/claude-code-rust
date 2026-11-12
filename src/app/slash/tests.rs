// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::App;
use serde_json::json;

// Re-import submodule items needed by tests
use super::candidates::{
    argument_candidates, detect_slash_at_cursor, find_advertised_command,
    supported_command_candidates,
};

fn requested_slash_state(app: &App) -> Option<SlashState> {
    let detection =
        detect_slash_at_cursor(app.input.lines(), app.input.cursor_row(), app.input.cursor_col())?;
    super::candidates::build_slash_state(app, detection, CompletionRequest::Arguments)
}

fn attach_test_connection(app: &mut App) -> crate::agent::client::CommandReceiver {
    let (connection, receiver) = crate::agent::client::AgentConnection::test_channel();
    app.session_runtime.conn = Some(std::rc::Rc::new(connection));
    receiver
}

fn session_update(update: model::SessionUpdate) -> crate::agent::events::ClientEvent {
    crate::agent::events::ClientEvent::SessionUpdate { session_id: "sess-1".to_owned(), update }
}

#[test]
fn ultracode_status_uses_verified_snapshot_and_effort() {
    for (state, expected) in [
        (None, "Ultracode status is unknown for this session."),
        (
            model::UltracodeState::new(true, false, false),
            "Ultracode is off and available for this session.",
        ),
        (
            model::UltracodeState::new(true, true, true),
            "Ultracode is on for this session. Effort remains xhigh.",
        ),
        (
            model::UltracodeState::new(false, true, false),
            "Ultracode is requested but unavailable for this session.",
        ),
        (
            model::UltracodeState::new(false, false, false),
            "Ultracode is off and unavailable for this session.",
        ),
    ] {
        let mut app = App::test_default();
        app.session_runtime.ultracode = state;
        app.session_runtime.config_options.insert("effortLevel".to_owned(), json!("xhigh"));
        app.status = AppStatus::Running;
        assert!(try_handle_submit(&mut app, "/ultracode status"));
        let MessageBlock::Text(block) = &app.transcript.messages.last().expect("status").blocks[0]
        else {
            panic!("text")
        };
        assert_eq!(block.text, expected);
        assert_eq!(app.status, AppStatus::Running);
    }
}

#[test]
fn resolved_submission_is_the_active_turn_policy_authority() {
    let classify = |input: &str| ResolvedSubmission::resolve(input.to_owned()).class();

    assert_eq!(classify("/cancel"), SubmissionClass::TurnControl);
    for input in [
        "/config",
        "/diff",
        "/memory",
        "/permissions",
        "/sandbox",
        "/hooks",
        "/help",
        "/mcp",
        "/plugins",
        "/status",
        "/usage",
        "/agents",
    ] {
        assert_eq!(classify(input), SubmissionClass::Fullscreen, "unexpected class for {input}");
    }
    assert_eq!(classify("/docs commands"), SubmissionClass::Informational);
    for input in [
        "next prompt",
        "/remote-command",
        "/compact",
        "/agent reviewer",
        "/effort high",
        "/fast",
        "/login",
        "/logout",
        "/mode plan",
        "/model sonnet",
        "/new-session",
        "/resume",
        "/rewind user-1 conversation",
    ] {
        assert_eq!(classify(input), SubmissionClass::TurnExclusive, "unexpected class for {input}");
    }
    for input in [
        "/cancel extra",
        "/diff extra",
        "/docs nope",
        "/plugins extra",
        "/resume one two",
        "/rewind user-1 invalid",
    ] {
        assert_eq!(classify(input), SubmissionClass::Invalid, "unexpected class for {input}");
    }
}

#[test]
fn advertised_command_collision_prefers_canonical_then_builtin() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands = vec![
        model::AvailableCommand::new("/deploy", "Plugin deploy").aliases(vec!["/ship".to_owned()]),
        model::AvailableCommand::new("/ship", "User ship"),
        model::AvailableCommand::new("/deploy", "Built-in deploy").builtin(true),
    ];

    assert_eq!(
        find_advertised_command(&app, "/ship").map(|cmd| cmd.description.as_str()),
        Some("User ship")
    );
    assert_eq!(
        find_advertised_command(&app, "/deploy").map(|cmd| cmd.description.as_str()),
        Some("Built-in deploy")
    );
}

#[test]
fn app_slash_catalog_roundtrips_command_names() {
    for spec in APP_SLASH_COMMANDS {
        assert_eq!(AppSlashCommand::from_name(spec.name), Some(spec.command));
        assert_eq!(spec.command.name(), spec.name);
    }
}

/// Collect the first column of the App-Owned Commands table in the manual.
///
/// The header and separator rows carry no backticked cell, so they are skipped.
fn documented_app_slash_commands(markdown: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_table_section = false;

    for line in markdown.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            in_table_section = heading == "App-Owned Commands";
            continue;
        }
        if !in_table_section || !line.starts_with('|') {
            continue;
        }
        let Some(cell) = line.split('|').nth(1) else {
            continue;
        };
        if let Some(name) = cell.trim().strip_prefix('`').and_then(|inner| inner.strip_suffix('`'))
        {
            names.push(name.to_owned());
        }
    }

    names
}

#[test]
fn app_config_does_not_enter_advertised_argument_mode() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/config", "SDK config command").input_hint("<setting>")];
    app.input.set_text("/config ");
    let _ = app.input.set_cursor(0, "/config ".chars().count());

    assert!(requested_slash_state(&app).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn app_fast_shadows_advertised_command_and_toggles_authoritative_state() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let cases = [
                (model::FastModeState::Off, true, model::FastModeState::On),
                (model::FastModeState::On, false, model::FastModeState::Off),
                (model::FastModeState::Cooldown, false, model::FastModeState::Off),
            ];

            for (initial, expected_enabled, acknowledged) in cases {
                let mut app = App::test_default();
                let mut rx = attach_test_connection(&mut app);
                app.session_runtime.session_id = Some(model::SessionId::new("sess-1"));
                app.session_runtime.fast_mode_state = initial;
                app.sdk_inventory.available_commands = vec![
                    model::AvailableCommand::new("/fast", "SDK fast command").input_hint("<mode>"),
                ];

                let consumed = try_handle_submit(&mut app, "/fast");

                assert!(consumed);
                assert!(matches!(app.status, AppStatus::CommandPending));
                assert!(matches!(
                    app.turn.pending_command_ack,
                    Some(super::super::PendingCommandAck::FastMode)
                ));

                tokio::task::yield_now().await;
                let envelope = rx.try_recv().expect("set fast mode command");
                assert_eq!(
                    envelope.command,
                    crate::agent::wire::BridgeCommand::SetFastMode {
                        session_id: "sess-1".to_owned(),
                        enabled: expected_enabled,
                    }
                );

                super::super::events::handle_client_event(
                    &mut app,
                    session_update(model::SessionUpdate::FastModeUpdate {
                        state: acknowledged,
                        disabled_reason: None,
                    }),
                );
                assert_eq!(app.session_runtime.fast_mode_state, acknowledged);
                assert!(matches!(app.status, AppStatus::Ready));
                assert!(app.turn.pending_command_ack.is_none());
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn fast_capability_check_still_allows_disable() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let unsupported_model = model::CurrentModel::new("model", "Model", "Model")
                .supports_fast_mode(Some(false))
                .authoritative(true);
            let mut app = App::test_default();
            let mut rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some(model::SessionId::new("sess-1"));
            app.session_runtime.current_model = Some(unsupported_model);
            app.session_runtime.fast_mode_state = model::FastModeState::On;

            assert!(try_handle_submit(&mut app, "/fast"));
            tokio::task::yield_now().await;
            assert_eq!(
                rx.try_recv().expect("disable fast mode command").command,
                crate::agent::wire::BridgeCommand::SetFastMode {
                    session_id: "sess-1".to_owned(),
                    enabled: false,
                }
            );
        })
        .await;
}

#[test]
fn detect_slash_argument_context_after_first_space() {
    let lines = vec!["/mode pla".to_owned()];
    let detection =
        detect_slash_at_cursor(&lines, 0, "/mode pla".chars().count()).expect("slash detection");

    match detection.context {
        SlashContext::Argument { command, arg_index, token_range } => {
            assert_eq!(command, "/mode");
            assert_eq!(arg_index, 0);
            assert_eq!(token_range, (6, 9));
        }
        SlashContext::CommandName => panic!("expected argument context"),
    }
    assert_eq!(detection.query, "pla");
}

#[test]
fn agent_argument_candidates_filter_by_query() {
    let mut app = App::test_default();
    app.sdk_inventory.available_agents = vec![
        crate::agent::model::AvailableAgent::new("reviewer", "Review code"),
        crate::agent::model::AvailableAgent::new("planner", "Plan work"),
    ];
    app.input.set_text("/agent rev");
    let _ = app.input.set_cursor(0, "/agent rev".chars().count());

    let slash = requested_slash_state(&app).expect("slash state");

    assert!(matches!(slash.context, SlashContext::Argument { .. }));
    assert_eq!(
        slash
            .candidates
            .iter()
            .map(|candidate| candidate.insert_value.as_str())
            .collect::<Vec<_>>(),
        vec!["reviewer"]
    );
}

#[test]
fn rewind_argument_candidates_use_cached_targets() {
    let mut app = App::test_default();
    let session_id = model::SessionId::new("session-1");
    app.session_runtime.session_id = Some(session_id.clone());
    app.sdk_inventory.rewind_targets_session_id = Some(session_id);
    app.sdk_inventory.rewind_targets = vec![
        model::RewindTarget {
            uuid: "user-1".to_owned(),
            first_text: "first prompt".to_owned(),
            input_text: "first prompt".to_owned(),
            index: 0,
            previous_assistant_uuid: None,
            resume_anchor_uuid: None,
        },
        model::RewindTarget {
            uuid: "user-2".to_owned(),
            first_text: "second prompt".to_owned(),
            input_text: "second prompt".to_owned(),
            index: 3,
            previous_assistant_uuid: Some("assistant-1".to_owned()),
            resume_anchor_uuid: Some("assistant-1".to_owned()),
        },
    ];
    app.input.set_text("/rewind second");
    let _ = app.input.set_cursor(0, "/rewind second".chars().count());

    let slash = requested_slash_state(&app).expect("slash state");

    assert!(matches!(slash.context, SlashContext::Argument { .. }));
    assert_eq!(
        slash
            .candidates
            .iter()
            .map(|candidate| (
                candidate.insert_value.as_str(),
                candidate.primary.as_str(),
                candidate.secondary.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![("user-2", "second prompt", Some("user-2"))]
    );
}

#[test]
fn rewind_argument_candidates_hide_stale_targets() {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.sdk_inventory.rewind_targets_session_id = Some(model::SessionId::new("old-session"));
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }];

    let candidates = argument_candidates(&app, "/rewind", 0);

    assert!(candidates.is_empty());
}

#[test]
fn effort_argument_candidates_filter_by_query() {
    let mut app = App::test_default();
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
            .supports_effort(true)
            .supported_effort_levels(crate::agent::model::EffortLevel::ALL.to_vec()),
    );
    app.input.set_text("/effort xh");
    let _ = app.input.set_cursor(0, "/effort xh".chars().count());

    let slash = requested_slash_state(&app).expect("slash state");

    assert!(matches!(slash.context, SlashContext::Argument { .. }));
    assert_eq!(
        slash
            .candidates
            .iter()
            .map(|candidate| candidate.insert_value.as_str())
            .collect::<Vec<_>>(),
        vec!["xhigh"]
    );
}

#[test]
fn docs_models_show_advertised_effort_levels() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("sonnet", "Claude Sonnet")
            .description("Balanced model")
            .supports_effort(true)
            .supported_effort_levels(vec![
                crate::agent::model::EffortLevel::Low,
                crate::agent::model::EffortLevel::Medium,
                crate::agent::model::EffortLevel::High,
                crate::agent::model::EffortLevel::XHigh,
                crate::agent::model::EffortLevel::Max,
            ])
            .supports_fast_mode(Some(true)),
    ];

    let consumed = try_handle_submit(&mut app, "/docs models");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Docs: Models"));
    assert!(block.text.contains("Effort: Low, Medium, High, XHigh, Max"));
    assert!(block.text.contains("Fast mode"));
}

#[test]
fn docs_commands_reuse_help_rows() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![crate::agent::model::AvailableCommand::new("/help", "Open help")];

    let consumed = try_handle_submit(&mut app, "/docs commands");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| Command | Description |"));
    assert!(block.text.contains("Ask one contextual question"));
    assert!(block.text.contains("/cancel"));
    assert!(block.text.contains("/compact"));
    assert!(block.text.contains("/config"));
    assert!(block.text.contains("/docs"));
    assert!(block.text.contains("/help"));
    assert!(block.text.contains("/mode"));
    assert!(block.text.contains("/model"));
    assert!(block.text.contains("/new-session"));
    assert!(block.text.contains("/resume"));
    assert!(block.text.contains("/rewind"));
}

#[test]
fn docs_commands_do_not_show_advertised_command_shadowed_by_app_command() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands = vec![
        crate::agent::model::AvailableCommand::new("/config", "SDK config command")
            .input_hint("<setting>"),
    ];

    let consumed = try_handle_submit(&mut app, "/docs commands");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| /config | Open the fullscreen settings tab. |"));
    assert!(!block.text.contains("SDK config command"));
}

#[test]
fn docs_commands_show_app_fast_instead_of_advertised_fast() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![crate::agent::model::AvailableCommand::new("/fast", "SDK fast command")];

    let consumed = try_handle_submit(&mut app, "/docs commands");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| /fast | Enable or disable fast mode for the active session. |"));
    assert!(!block.text.contains("SDK fast command"));
}

#[test]
fn docs_shortcuts_use_live_help_state() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs shortcuts");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| Shortcut | Action |"));
    assert!(block.text.contains("Send message"));
    assert!(!block.text.contains("Toggle todo"));
}

#[test]
fn non_variable_command_argument_mode_is_disabled() {
    let mut app = App::test_default();
    app.input.set_text("/cancel now");
    let _ = app.input.set_cursor(0, "/cancel now".chars().count());
    sync_with_cursor(&mut app);
    assert!(app.slash.visible().is_none());
}

#[test]
fn resume_without_id_opens_fullscreen_picker() {
    let mut app = App::test_default();
    let _receiver = attach_test_connection(&mut app);
    let consumed = try_handle_submit(&mut app, "/resume");
    assert!(consumed);
    assert!(matches!(
        app.surface_mode,
        crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::SessionPicker)
    ));
    let last = app.transcript.messages.last().expect("resume command message");
    assert!(matches!(last.role, MessageRole::User));
    let MessageBlock::Text(block) = last.blocks.first().expect("text block") else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "/resume");
}

#[test]
fn rewind_with_cached_target_sends_bridge_command() {
    use crate::agent::types::RewindRestoreMode;
    use clap::Parser;
    for (token, expected) in [
        ("conversation", RewindRestoreMode::Conversation),
        ("code", RewindRestoreMode::Code),
        ("both", RewindRestoreMode::Both),
    ] {
        let mut app = App::test_default();
        let mut rx = attach_test_connection(&mut app);
        app.session_runtime.activate_session("rewind-session".into());
        let cli = crate::Cli::try_parse_from([
            "claude-rs",
            "--model",
            "haiku",
            "--name",
            "Restored branch",
        ])
        .unwrap();
        app.startup = crate::app::state::StartupState::from_cli(&cli);
        app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
            uuid: "selected-user".into(),
            first_text: "Chosen source".into(),
            input_text: "Chosen source".into(),
            index: 0,
            previous_assistant_uuid: Some("different-assistant".into()),
            resume_anchor_uuid: Some("different-anchor".into()),
        }];
        assert!(try_handle_submit(&mut app, &format!("/rewind selected-user {token}")));
        let envelope = rx.try_recv().expect("rewind command");
        let crate::agent::wire::BridgeCommand::Rewind {
            session_id,
            target_user_message_id,
            restore_mode,
            launch_settings,
        } = envelope.command
        else {
            panic!("rewind")
        };
        assert_eq!(session_id, "rewind-session");
        assert_eq!(target_user_message_id, "selected-user");
        assert_eq!(restore_mode, expected);
        assert_eq!(launch_settings.model.as_deref(), Some("haiku"));
        assert_eq!(launch_settings.name.as_deref(), Some("Restored branch"));
        assert!(!app.composer_access().can_submit());
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn mode_sets_command_pending_and_mode_update_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.mode = Some(crate::app::ModeState {
                current_mode_id: "default".into(),
                current_mode_name: "Old default".into(),
                available_modes: vec![
                    crate::app::ModeInfo { id: "default".into(), name: "Old default".into() },
                    crate::app::ModeInfo { id: "plan".into(), name: "Old plan".into() },
                ],
            });
            assert!(try_handle_submit(&mut app, "/mode plan"));
            assert_eq!(app.status, AppStatus::CommandPending);
            assert_eq!(app.displayed_status(), AppStatus::Ready);
            expect_mode_command(&mut commands, "plan").await;
            crate::app::handle_client_event(
                &mut app,
                session_update(model::SessionUpdate::ModeStateUpdate(crate::app::ModeState {
                    current_mode_id: "plan".into(),
                    current_mode_name: "Confirmed planning".into(),
                    available_modes: vec![
                        crate::app::ModeInfo {
                            id: "plan".into(),
                            name: "Confirmed planning".into(),
                        },
                        crate::app::ModeInfo {
                            id: "acceptEdits".into(),
                            name: "New editing choice".into(),
                        },
                    ],
                })),
            );
            assert_eq!(app.status, AppStatus::Ready);
            assert!(app.turn.pending_command_label.is_none());
            let confirmed = app.session_runtime.mode.as_ref().unwrap();
            assert_eq!(confirmed.current_mode_id, "plan");
            assert_eq!(confirmed.current_mode_name, "Confirmed planning");
            let candidates = argument_candidates(&app, "/mode", 0);
            let choices: std::collections::BTreeMap<_, _> =
                candidates.iter().map(|c| (c.insert_value.as_str(), c.primary.as_str())).collect();
            assert_eq!(
                choices,
                std::collections::BTreeMap::from([
                    ("plan", "Confirmed planning"),
                    ("acceptEdits", "New editing choice")
                ])
            );
            // Removed choices cannot be selected from completion; typed IDs remain bridge-validated.
            app.input.set_text("/mode default");
            let removed = requested_slash_state(&app).unwrap();
            assert!(removed.candidates.is_empty());
            assert!(commands.try_recv().is_err());
            assert!(try_handle_submit(&mut app, "/mode acceptEdits"));
            expect_mode_command(&mut commands, "acceptEdits").await;
            assert_eq!(
                app.session_runtime.mode.as_ref().unwrap().current_mode_id,
                "plan",
                "sending does not change confirmed state"
            );
            crate::app::handle_client_event(
                &mut app,
                session_update(model::SessionUpdate::ModeStateUpdate(crate::app::ModeState {
                    current_mode_id: "acceptEdits".into(),
                    current_mode_name: "Confirmed editing".into(),
                    available_modes: vec![crate::app::ModeInfo {
                        id: "acceptEdits".into(),
                        name: "Confirmed editing".into(),
                    }],
                })),
            );
            assert_eq!(app.status, AppStatus::Ready);
            assert_eq!(
                app.session_runtime.mode.as_ref().unwrap().current_mode_name,
                "Confirmed editing"
            );
            assert!(commands.try_recv().is_err());
        })
        .await;
}

async fn expect_mode_command(commands: &mut crate::agent::client::CommandReceiver, expected: &str) {
    let envelope =
        tokio::time::timeout(std::time::Duration::from_secs(2), commands.recv_envelope())
            .await
            .expect("mode request deadline")
            .expect("mode request");
    assert!(
        matches!(envelope.command,crate::agent::wire::BridgeCommand::SetMode{ref session_id,ref mode} if session_id=="sess-1"&&mode==expected)
    );
    assert!(commands.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn model_sets_command_pending_and_current_model_ack_updates_model_and_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let _rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("old-model", "old-model", "old-model")
                    .authoritative(true),
            );

            let consumed = try_handle_submit(&mut app, "/model sonnet");
            assert!(consumed);
            assert!(
                matches!(app.status, AppStatus::CommandPending),
                "expected CommandPending, got {:?}",
                app.status
            );
            assert_eq!(app.displayed_status(), AppStatus::Ready);
            assert_eq!(
                app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
                Some("old-model")
            );

            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::CurrentModelUpdate(
                    crate::agent::model::CurrentModelUpdate::new(
                        crate::agent::model::CurrentModel::new("sonnet", "sonnet", "sonnet")
                            .authoritative(true),
                    ),
                )),
            );
            assert!(
                matches!(app.status, AppStatus::Ready),
                "expected Ready after current model ack, got {:?}",
                app.status
            );
            assert_eq!(
                app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
                Some("sonnet")
            );
            assert!(app.turn.pending_command_label.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_sets_command_pending_and_config_option_ack_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
                    .supports_effort(true)
                    .supported_effort_levels(crate::agent::model::EffortLevel::ALL.to_vec()),
            );

            let consumed = try_handle_submit(&mut app, "/effort xhigh");
            assert!(consumed);
            assert!(matches!(app.status, AppStatus::CommandPending));
            assert_eq!(app.displayed_status(), AppStatus::Ready);
            assert!(matches!(
                app.turn.pending_command_ack.as_ref(),
                Some(super::super::PendingCommandAck::ConfigOption { option_id })
                    if option_id == "effortLevel"
            ));

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set effort command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetEffort {
                    session_id: "sess-1".to_owned(),
                    effort: Some("xhigh".to_owned()),
                }
            );

            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::ConfigOptionUpdate(
                    crate::agent::model::ConfigOptionUpdate {
                        option_id: "effortLevel".to_owned(),
                        value: serde_json::json!("xhigh"),
                    },
                )),
            );
            assert!(matches!(app.status, AppStatus::Ready));
            assert_eq!(
                app.session_runtime.config_options.get("effortLevel"),
                Some(&serde_json::json!("xhigh"))
            );
            assert_eq!(app.session_effort(), Some(crate::agent::model::EffortLevel::XHigh));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_accepts_session_only_max() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
                    .supports_effort(true)
                    .supported_effort_levels(vec![
                        crate::agent::model::EffortLevel::Low,
                        crate::agent::model::EffortLevel::Medium,
                        crate::agent::model::EffortLevel::High,
                    ]),
            );

            let consumed = try_handle_submit(&mut app, "/effort max");
            assert!(consumed);

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set effort command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetEffort {
                    session_id: "sess-1".to_owned(),
                    effort: Some("max".to_owned()),
                }
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_sets_command_pending_and_config_option_ack_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.sdk_inventory.available_agents =
                vec![crate::agent::model::AvailableAgent::new("reviewer", "Review code")];

            let consumed = try_handle_submit(&mut app, "/agent reviewer");
            assert!(consumed);
            assert!(matches!(app.status, AppStatus::CommandPending));
            assert_eq!(app.displayed_status(), AppStatus::Ready);
            assert!(matches!(
                app.turn.pending_command_ack.as_ref(),
                Some(super::super::PendingCommandAck::ConfigOption { option_id })
                    if option_id == "agent"
            ));

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set agent command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetAgent {
                    session_id: "sess-1".to_owned(),
                    agent: Some("reviewer".to_owned()),
                }
            );

            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::ConfigOptionUpdate(
                    crate::agent::model::ConfigOptionUpdate {
                        option_id: "agent".to_owned(),
                        value: serde_json::json!("reviewer"),
                    },
                )),
            );
            assert!(matches!(app.status, AppStatus::Ready));
            assert_eq!(
                app.session_runtime.config_options.get("agent"),
                Some(&serde_json::json!("reviewer"))
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_reset_sends_null_agent() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());

            let consumed = try_handle_submit(&mut app, "/agent reset");
            assert!(consumed);

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("reset agent command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetAgent {
                    session_id: "sess-1".to_owned(),
                    agent: None,
                }
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_command_routes_selection_to_the_bridge_for_validation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());

            let consumed = try_handle_submit(&mut app, "/agent custom-agent");
            assert!(consumed);

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set agent command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetAgent {
                    session_id: "sess-1".to_owned(),
                    agent: Some("custom-agent".to_owned()),
                }
            );
        })
        .await;
}

#[test]
fn agent_invalid_arguments_return_usage() {
    for input in ["/agent", "/agent reviewer extra"] {
        let mut app = App::test_default();

        let consumed = try_handle_submit(&mut app, input);

        assert!(consumed);
        let Some(last) = app.transcript.messages.last() else {
            panic!("expected system usage message for {input}");
        };
        let Some(MessageBlock::Text(block)) = last.blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Usage: /agent <name|reset>");
        assert!(!matches!(app.status, AppStatus::CommandPending));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ultracode_composer_wire_events_status_and_footer_workflow() {
    tokio::task::LocalSet::new().run_until(Box::pin(async {
        use crate::agent::{types, wire};
            use crate::agent::events::ClientEvent;
        let mut app = App::test_default();
        let mut commands = attach_test_connection(&mut app);
        app.session_runtime.session_id = Some("sess-1".into());
        app.session_runtime.current_model = Some(model::CurrentModel::new("opus", "Opus", "Opus").supports_effort(true));
        app.session_runtime.mode = Some(crate::app::ModeState {
            current_mode_id: "default".to_owned(), current_mode_name: "default".to_owned(), available_modes: vec![],
        });
        app.session_runtime.config_options.insert("effortLevel".to_owned(), json!("high"));
        let off = model::UltracodeState::new(true, false, false);
        let on = model::UltracodeState::new(true, true, true);
        app.session_runtime.ultracode = off;
        let transcript_before = app.transcript.messages.len();
        app.input.set_text("/ultracode on");
        crate::app::input_submit::submit_input(&mut app);
        assert_eq!(app.status, AppStatus::CommandPending);
        assert_eq!(app.session_runtime.ultracode, off, "submission must not optimistically change state");
        tokio::task::yield_now().await;
        let command = commands.try_recv().expect("set command");
        assert_eq!(serde_json::to_value(command).expect("wire command"), json!({"command":"set_ultracode","session_id":"sess-1","enabled":true}));

        // Cross the actual wire decoder before applying a received snapshot.
        let envelope: wire::EventEnvelope = serde_json::from_value(json!({"event":"session_update","session_id":"sess-1","update":{"type":"ultracode_update","ultracode":{"available":true,"requested":true,"effective":true}}})).expect("NDJSON event");
        let wire::BridgeEvent::SessionUpdate { session_id, update: types::SessionUpdate::UltracodeUpdate { ultracode } } = envelope.event else { panic!("expected Ultracode update") };
        crate::app::handle_client_event(&mut app, ClientEvent::SessionUpdate { session_id, update: model::SessionUpdate::UltracodeUpdate { ultracode } });
        assert_eq!(app.status, AppStatus::Ready);
        assert!(app.turn.pending_command_ack.is_none());
        assert_eq!(app.session_runtime.ultracode, on);
        assert_eq!(app.transcript.messages.len(), transcript_before, "successful change must not add permanent messages");
        let footer = crate::ui::footer_rows::serialize_footer_rows(&app, 120);
        let row: String = footer.rows[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(row.contains("[Opus]") && row.contains("[High]") && row.contains("[Ultracode]"), "{row}");

        app.input.set_text("/effort low");
        crate::app::input_submit::submit_input(&mut app);
        tokio::task::yield_now().await;
        assert!(matches!(commands.try_recv().expect("effort").command, wire::BridgeCommand::SetEffort { effort, .. } if effort.as_deref() == Some("low")));
        crate::app::handle_client_event(&mut app, session_update(model::SessionUpdate::UltracodeUpdate { ultracode: on }));
        assert_eq!(app.status, AppStatus::CommandPending, "Ultracode telemetry must not acknowledge effort");
        crate::app::handle_client_event(&mut app, session_update(model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate { option_id: "effortLevel".to_owned(), value: json!("low") })));
        app.input.set_text("/ultracode status");
        crate::app::input_submit::submit_input(&mut app);
        let MessageBlock::Text(status) = &app.transcript.messages.last().expect("status").blocks[0] else { panic!("text") };
        assert_eq!(status.text, "Ultracode is on for this session. Effort remains low.");

        app.status = AppStatus::Running;
        app.input.set_text("/ultracode off");
        crate::app::input_submit::submit_input(&mut app);
        tokio::task::yield_now().await;
        assert!(commands.try_recv().is_err(), "state changes during an active turn must be blocked");
        assert_eq!(app.session_runtime.ultracode, on);
        app.input.set_text("/ultracode status");
        crate::app::input_submit::submit_input(&mut app);
        assert_eq!(app.status, AppStatus::Running);

        app.status = AppStatus::Ready;
        app.input.set_text("/ultracode off");
        crate::app::input_submit::submit_input(&mut app);
        tokio::task::yield_now().await;
        assert!(matches!(commands.try_recv().expect("off").command, wire::BridgeCommand::SetUltracode { enabled: false, .. }));
        crate::app::handle_client_event(&mut app, session_update(model::SessionUpdate::UltracodeUpdate { ultracode: off }));
        assert_eq!(app.status, AppStatus::Ready);
        let footer = crate::ui::footer_rows::serialize_footer_rows(&app, 120);
        assert!(!footer.rows[0].spans.iter().any(|s| s.content.contains("Ultracode")));

        app.input.set_text("/ultracode on");
        crate::app::input_submit::submit_input(&mut app);
        tokio::task::yield_now().await;
        commands.try_recv().expect("on");
        crate::app::handle_client_event(&mut app, ClientEvent::SlashCommandError { session_id: Some("sess-1".to_owned()), message: "Cannot enable Ultracode: dynamic workflows are disabled for this session.".to_owned() });
        assert_eq!(app.status, AppStatus::Ready);
        assert!(app.turn.pending_command_ack.is_none());
        assert_eq!(app.session_runtime.ultracode, off);

        app.input.set_text("/ultracode on");
        crate::app::input_submit::submit_input(&mut app);
        tokio::task::yield_now().await;
        commands.try_recv().expect("unavailable on");
        let unavailable = model::UltracodeState::new(false, true, false);
        crate::app::handle_client_event(&mut app, session_update(model::SessionUpdate::UltracodeUpdate { ultracode: unavailable }));
        let error_message = "Cannot enable Ultracode: unavailable for this session. The SDK saved the request, but Ultracode remains inactive.";
        crate::app::handle_client_event(&mut app, ClientEvent::SlashCommandError { session_id: Some("sess-1".to_owned()), message: error_message.to_owned() });
        assert_eq!(app.status, AppStatus::Ready);
        assert!(app.turn.pending_command_ack.is_none());
        assert_eq!(app.session_runtime.ultracode, unavailable);
        let error = app.transcript.messages.last().expect("activation error");
        assert!(matches!(error.role, MessageRole::System(Some(SystemSeverity::Error))));
        let MessageBlock::Notice(error) = &error.blocks[0] else { panic!("error notice") };
        assert_eq!(error.severity, SystemSeverity::Error);
        assert_eq!(error.text.text, error_message);
        let footer = crate::ui::footer_rows::serialize_footer_rows(&app, 120);
        assert!(!footer.rows[0].spans.iter().any(|s| s.content.contains("Ultracode")));

        app.input.set_text("/ultracode status");
        crate::app::input_submit::submit_input(&mut app);
        let status = app.transcript.messages.last().expect("unavailable status");
        assert!(matches!(status.role, MessageRole::System(Some(SystemSeverity::Info))));
        let MessageBlock::Text(status) = &status.blocks[0] else { panic!("text") };
        assert_eq!(status.text, "Ultracode is requested but unavailable for this session.");
        assert!(commands.try_recv().is_err(), "status must not retry activation");
    })).await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_reset_and_thinking_wait_for_session_acknowledgement() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            use crate::agent::wire::BridgeCommand;
            let mut app = App::test_default();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.config_options.insert("effortLevel".to_owned(), json!("max"));
            app.session_runtime
                .config_options
                .insert("alwaysThinkingEnabled".to_owned(), json!(true));
            for (input, expected, option, acknowledged) in [
                (
                    "/effort reset",
                    BridgeCommand::SetEffort { session_id: "sess-1".to_owned(), effort: None },
                    "effortLevel",
                    json!("high"),
                ),
                (
                    "/thinking off",
                    BridgeCommand::SetThinking {
                        session_id: "sess-1".to_owned(),
                        enabled: Some(false),
                    },
                    "alwaysThinkingEnabled",
                    json!(false),
                ),
                (
                    "/thinking reset",
                    BridgeCommand::SetThinking { session_id: "sess-1".to_owned(), enabled: None },
                    "alwaysThinkingEnabled",
                    json!(true),
                ),
            ] {
                let previous = app.session_runtime.config_options.get(option).cloned();
                app.input.set_text(input);
                crate::app::input_submit::submit_input(&mut app);
                assert_eq!(app.status, AppStatus::CommandPending);
                assert_eq!(app.session_runtime.config_options.get(option), previous.as_ref());
                tokio::task::yield_now().await;
                assert_eq!(commands.try_recv().expect("session choice").command, expected);
                crate::app::handle_client_event(
                    &mut app,
                    session_update(model::SessionUpdate::ConfigOptionUpdate(
                        model::ConfigOptionUpdate {
                            option_id: option.to_owned(),
                            value: acknowledged.clone(),
                        },
                    )),
                );
                assert_eq!(app.status, AppStatus::Ready);
                assert_eq!(app.session_runtime.config_options.get(option), Some(&acknowledged));
            }
        }))
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn explicit_fast_retry_recovers_unknown_state_after_acknowledgement() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.fast_mode_state = model::FastModeState::Unknown;
            assert!(try_handle_submit(&mut app, "/fast off"));
            assert_eq!(app.status, AppStatus::CommandPending);
            assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Unknown);
            tokio::task::yield_now().await;
            assert!(matches!(
                commands.try_recv().expect("retry").command,
                crate::agent::wire::BridgeCommand::SetFastMode { enabled: false, .. }
            ));
            crate::app::handle_client_event(
                &mut app,
                session_update(model::SessionUpdate::FastModeUpdate {
                    state: model::FastModeState::Off,
                    disabled_reason: None,
                }),
            );
            assert_eq!(app.status, AppStatus::Ready);
            assert_eq!(app.session_runtime.fast_mode_state, model::FastModeState::Off);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reload_sends_one_bridge_command_and_blocks_input_until_acknowledged() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());

            assert!(try_handle_submit(&mut app, "/reload"));

            assert_eq!(app.status, AppStatus::CommandPending);
            assert!(matches!(
                app.turn.pending_command_ack,
                Some(super::super::PendingCommandAck::SkillsAndStylesReload)
            ));
            tokio::task::yield_now().await;
            assert_eq!(
                commands.try_recv().expect("reload command").command,
                crate::agent::wire::BridgeCommand::ReloadSkillsAndStyles {
                    session_id: "sess-1".to_owned()
                }
            );
            assert!(commands.try_recv().is_err());
        })
        .await;
}

#[test]
fn export_without_a_connection_is_refused() {
    let mut app = App::test_default();

    assert!(try_handle_submit(&mut app, "/export"));

    assert_eq!(app.status, AppStatus::Ready);
    assert!(app.history_export.is_none());
    let MessageBlock::Text(block) = &app.transcript.messages.last().expect("notice").blocks[0]
    else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Cannot export: not connected yet.");
}

#[tokio::test(flavor = "current_thread")]
async fn export_requests_the_history_under_a_request_id_and_blocks_input() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut app = App::test_default();
            app.cwd_raw = dir.path().display().to_string();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());

            // The path after the command is taken whole, spaces included.
            assert!(try_handle_submit(&mut app, "/export notes for review.md"));

            assert_eq!(app.status, AppStatus::CommandPending);
            assert!(matches!(
                app.turn.pending_command_ack,
                Some(super::super::PendingCommandAck::HistoryExport)
            ));
            tokio::task::yield_now().await;
            let envelope = commands.try_recv().expect("history request");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::ExportSessionHistory {
                    session_id: "sess-1".to_owned()
                }
            );
            let request_id = envelope.request_id.expect("request id");

            // Both envelope session and request must match before the input lock is released.
            for (session_id, id) in
                [("stale-session", request_id.as_str()), ("sess-1", "other-request")]
            {
                crate::app::handle_client_event(
                    &mut app,
                    crate::agent::events::ClientEvent::SessionHistoryChunk {
                        session_id: session_id.to_owned(),
                        chunk: crate::agent::events::HistoryChunk {
                            request_id: Some(id.to_owned()),
                            updates: vec![model::SessionUpdate::UserMessageChunk(
                                model::ContentChunk::new(model::ContentBlock::Text(
                                    model::TextContent::new("foreign content"),
                                )),
                            )],
                            last: true,
                            omitted_updates: 0,
                            unmapped_updates: 0,
                            error: None,
                        },
                    },
                );
                assert!(!dir.path().join("notes for review.md").exists());
                assert_eq!(app.status, AppStatus::CommandPending);
                assert!(app.history_export.is_some());
            }

            // The chunk that answers it writes the file at the spelled path.
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::SessionHistoryChunk {
                    session_id: "sess-1".to_owned(),
                    chunk: crate::agent::events::HistoryChunk {
                        request_id: Some(request_id),
                        updates: Vec::new(),
                        last: true,
                        omitted_updates: 0,
                        unmapped_updates: 0,
                        error: None,
                    },
                },
            );
            assert_eq!(app.status, AppStatus::Ready);
            assert!(dir.path().join("notes for review.md").is_file());
            assert!(
                !std::fs::read_to_string(dir.path().join("notes for review.md"))
                    .expect("export")
                    .contains("foreign content")
            );
            crate::app::events::handle_terminal_event(
                &mut app,
                crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                    crossterm::event::KeyCode::Char('r'),
                    crossterm::event::KeyModifiers::NONE,
                )),
            );
            assert_eq!(app.input.text(), "r");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn export_to_an_existing_file_is_refused_without_asking_the_bridge() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::write(dir.path().join("out.md"), "original").expect("seed");
            let mut app = App::test_default();
            app.cwd_raw = dir.path().display().to_string();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());

            assert!(try_handle_submit(&mut app, "/export out.md"));

            assert_eq!(app.status, AppStatus::Ready);
            assert!(app.history_export.is_none());
            tokio::task::yield_now().await;
            assert!(commands.try_recv().is_err(), "no history request for a refused target");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("out.md")).expect("read"),
                "original"
            );
            let MessageBlock::Text(block) =
                &app.transcript.messages.last().expect("refusal").blocks[0]
            else {
                panic!("expected text block");
            };
            assert!(block.text.contains("already exists"), "{}", block.text);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn only_the_exports_own_error_ends_the_export() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut app = App::test_default();
            app.cwd_raw = dir.path().display().to_string();
            let mut commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            assert!(try_handle_submit(&mut app, "/export out.md"));
            tokio::task::yield_now().await;
            let request_id = commands.try_recv().expect("history request").request_id;
            // A rename waiting for its answer takes the next generic slash error for itself.
            app.config.pending_session_title_change =
                Some(crate::app::config::PendingSessionTitleChangeState {
                    session_id: "sess-1".to_owned(),
                    kind: crate::app::config::PendingSessionTitleChangeKind::Generate,
                });

            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::SlashCommandError {
                    session_id: Some("sess-1".to_owned()),
                    message: "title generation failed".to_owned(),
                },
            );

            assert!(app.history_export.is_some(), "an unrelated error must not cancel the export");
            assert_eq!(app.status, AppStatus::CommandPending);

            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::SessionHistoryChunk {
                    session_id: "sess-1".to_owned(),
                    chunk: crate::agent::events::HistoryChunk {
                        request_id,
                        updates: Vec::new(),
                        last: true,
                        omitted_updates: 0,
                        unmapped_updates: 0,
                        error: Some("unknown session: sess-1".to_owned()),
                    },
                },
            );

            assert!(app.history_export.is_none());
            assert_eq!(app.status, AppStatus::Ready);
            let MessageBlock::Notice(notice) =
                &app.transcript.messages.last().expect("failure notice").blocks[0]
            else {
                panic!("expected notice block");
            };
            assert_eq!(notice.text.text, "unknown session: sess-1");
            assert!(!dir.path().join("out.md").exists());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_that_cannot_be_sent_fails_the_export_itself() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut app = App::test_default();
            app.cwd_raw = dir.path().display().to_string();
            let commands = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            // With the bridge gone the send fails, as it does after the bridge exits.
            drop(commands);
            let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(8);
            app.event_tx = event_tx;

            assert!(try_handle_submit(&mut app, "/export out.md"));
            let event = event_rx.recv().await.expect("failure event");

            let crate::agent::events::ClientEvent::SessionHistoryChunk { chunk, .. } = event else {
                panic!("the failure must travel on the export's own event");
            };
            assert!(chunk.last);
            assert!(chunk.request_id.is_some());
            assert!(chunk.error.is_some_and(|error| error.contains("Failed to run /export")));
        })
        .await;
}

#[test]
fn invalid_login_submission_preserves_conversation_without_starting_authentication() {
    use crate::app::input_submit::submit_input;
    let (mut app, mut rx) = crate::app::test_support::app_with_connection();
    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(crate::app::TextBlock::from_complete("earlier user message"))],
        None,
    ));
    let previous_id = app.transcript.messages[0].id;
    app.input.set_text("/login extra");
    submit_input(&mut app);
    assert!(app.input.is_empty());
    assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
    assert!(matches!(app.status, AppStatus::Ready));
    assert_eq!(app.session_runtime.session_id.as_ref().unwrap().to_string(), "session-1");
    assert_eq!(app.transcript.messages[0].id, previous_id);
    assert!(
        matches!(&app.transcript.messages[0].blocks[..], [MessageBlock::Text(t)] if t.text == "earlier user message")
    );
    let last = app.transcript.messages.last().unwrap();
    assert!(matches!(last.role, MessageRole::System(Some(crate::app::SystemSeverity::Error))));
    assert!(matches!(&last.blocks[..], [MessageBlock::Text(t)] if t.text == "Usage: /login"));
    assert!(
        rx.try_recv().is_err(),
        "invalid arguments must not send authentication or prompt commands"
    );
}

#[test]
fn advertised_command_and_alias_submit_exact_source_while_unknown_command_stays_local() {
    use crate::agent::wire::BridgeCommand;
    use crate::app::input_submit::submit_input;
    for command in
        ["/remote-command exact arguments", "/remote alias arguments", "/unknown-command"]
    {
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        app.sdk_inventory.available_commands = vec![
            model::AvailableCommand::new("/remote-command", "Remote command")
                .aliases(vec!["/remote".into()]),
        ];
        app.session_runtime.prompt_suggestion = Some("previous suggestion".into());
        app.input.set_text(command);
        submit_input(&mut app);
        assert!(app.input.is_empty());
        if command == "/unknown-command" {
            assert!(rx.try_recv().is_err());
            assert!(matches!(app.status, AppStatus::Ready));
            let last = app.transcript.messages.last().unwrap();
            assert!(last.blocks.iter().any(
                |b| matches!(b, MessageBlock::Text(t) if t.text.contains("/unknown-command") && t.text.contains("not yet supported"))
            ));
            assert_eq!(
                app.session_runtime.prompt_suggestion.as_deref(),
                Some("previous suggestion")
            );
            assert!(app.active_turn_assistant_idx().is_none());
        } else {
            let BridgeCommand::Prompt { session_id, message_uuid, chunks, inline_pastes } =
                rx.try_recv().expect("advertised prompt").command
            else {
                panic!("prompt")
            };
            assert_eq!(session_id, "session-1");
            assert!(inline_pastes.is_empty());
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].kind, "text");
            assert_eq!(chunks[0].value, json!(command));
            assert!(app.session_runtime.prompt_suggestion.is_none());
            let user = app
                .transcript
                .messages
                .iter()
                .find(|m| matches!(m.role, MessageRole::User))
                .unwrap();
            assert!(
                matches!(&user.blocks[..], [MessageBlock::Text(t)] if t.text == command && t.source_message_uuids == [message_uuid])
            );
            let owner = app.active_turn_assistant_idx().unwrap();
            crate::app::test_support::deliver_update(
                &mut app,
                model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                    model::ContentBlock::Text(model::TextContent::new("Advertised command result")),
                )),
            );
            assert!(
                matches!(&app.transcript.messages[owner].blocks[..], [MessageBlock::Text(t)] if t.text == "Advertised command result")
            );
            while let Ok(envelope) = rx.try_recv() {
                assert!(
                    matches!(envelope.command, BridgeCommand::GetContextUsage { session_id } if session_id == "session-1")
                );
            }
        }
    }
}

#[test]
fn live_turn_view_submissions_open_the_requested_pane_and_preserve_response_ownership() {
    use crate::agent::{events::ClientEvent, wire::BridgeCommand};
    use crate::app::{ConfigTab, FullscreenView, SurfaceMode, input_submit::submit_input};
    for (command, tab) in
        [("/status", ConfigTab::Status), ("/mcp", ConfigTab::Mcp), ("/plugins", ConfigTab::Plugins)]
    {
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        let dir = tempfile::tempdir().unwrap();
        app.cwd_raw = dir.path().to_string_lossy().into_owned();
        app.settings_home_override = Some(dir.path().to_path_buf());
        app.input.set_text("main request");
        submit_input(&mut app);
        assert!(
            matches!(rx.try_recv().unwrap().command, BridgeCommand::Prompt { session_id, .. } if session_id == "session-1")
        );
        while let Ok(envelope) = rx.try_recv() {
            assert!(matches!(envelope.command, BridgeCommand::GetContextUsage { .. }));
        }
        crate::app::test_support::deliver_update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("Before opening view")),
            )),
        );
        let owner = app.active_turn_assistant_idx().unwrap();
        app.config.active_tab = ConfigTab::Settings;
        // A fresh controlled inventory keeps this navigation test independent of host plugins.
        let cwd_raw = app.cwd_raw.clone();
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::PluginsInventoryUpdated {
                cwd_raw,
                snapshot: crate::app::plugins::PluginsInventorySnapshot {
                    installed: vec![crate::app::plugins::InstalledPluginEntry {
                        id: "audit-fixture@local".into(),
                        version: Some("1.0".into()),
                        scope: "user".into(),
                        enabled: true,
                        installed_at: None,
                        last_updated: None,
                        project_path: None,
                        mcp_server_names: vec![],
                    }],
                    marketplace: vec![],
                    marketplaces: vec![],
                },
                claude_path: dir.path().join("controlled-cli"),
            },
        );
        app.input.set_text(command);
        submit_input(&mut app);
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
        assert_eq!(app.config.active_tab, tab);
        assert!(app.input.is_empty());
        assert!(matches!(app.status, AppStatus::Running));
        assert!(!app.turn.cancel_requested);
        match tab {
            ConfigTab::Status => assert!(
                matches!(rx.try_recv().unwrap().command, BridgeCommand::GetStatusSnapshot {session_id} if session_id == "session-1")
            ),
            ConfigTab::Mcp => assert!(
                matches!(rx.try_recv().unwrap().command, BridgeCommand::GetMcpSnapshot {session_id} if session_id == "session-1")
            ),
            _ => {}
        }
        assert!(rx.try_recv().is_err());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let body: Vec<String> =
            (3..35).map(|y| (2..118).map(|x| buffer[(x, y)].symbol()).collect()).collect();
        match tab {
            ConfigTab::Status => {
                assert!(body.iter().any(|r| r.contains("Session ID") && r.contains("session-1")));
            }
            ConfigTab::Mcp => assert!(body.iter().any(|r| r.contains("Loading MCP status"))),
            ConfigTab::Plugins => assert!(body.iter().any(|r| r.contains("audit-fixture"))),
            _ => unreachable!(),
        }
        crate::app::test_support::press(&mut app, crossterm::event::KeyCode::Esc);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
        crate::app::test_support::deliver_update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new(" and after closing it")),
            )),
        );
        assert_eq!(app.active_turn_assistant_idx(), Some(owner));
        let response: String = app.transcript.messages[owner]
            .blocks
            .iter()
            .filter_map(|b| match b {
                MessageBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(response, "Before opening view and after closing it");
        assert!(rx.try_recv().is_err());
    }
}

#[path = "tests/completion_audit.rs"]
mod completion_audit;

mod submission_audit;
