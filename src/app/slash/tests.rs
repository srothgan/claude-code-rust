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

mod control_audit;
mod docs_audit;
mod export_audit;
