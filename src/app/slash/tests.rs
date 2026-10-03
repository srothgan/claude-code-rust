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
fn parse_non_slash_returns_none() {
    assert!(parse("hello world").is_none());
}

#[test]
fn ultracode_usage_policy_and_completion() {
    for input in
        ["/ultracode", "/ultracode toggle", "/ultracode on extra", "/ultracode status extra"]
    {
        let mut app = App::test_default();
        assert!(try_handle_submit(&mut app, input));
        let MessageBlock::Text(block) = &app.transcript.messages.last().expect("usage").blocks[0]
        else {
            panic!("expected usage text");
        };
        assert_eq!(block.text, "Usage: /ultracode <on|off|status>");
        assert_eq!(ResolvedSubmission::resolve(input.to_owned()).class(), SubmissionClass::Invalid);
    }
    let app = App::test_default();
    assert!(supported_command_candidates(&app).iter().any(|c| c.primary == "/ultracode"));
    assert!(!supported_command_candidates(&app).iter().any(|c| c.primary == "/ultramode"));
    assert_eq!(
        argument_candidates(&app, "/ultracode", 0)
            .iter()
            .map(|c| c.insert_value.as_str())
            .collect::<Vec<_>>(),
        vec!["on", "off", "status"]
    );
    assert_eq!(
        ResolvedSubmission::resolve("/ultracode status".to_owned()).class(),
        SubmissionClass::Informational
    );
    for operation in ["on", "off"] {
        assert_eq!(
            ResolvedSubmission::resolve(format!("/ultracode {operation}")).class(),
            SubmissionClass::TurnExclusive
        );
    }
}

#[test]
fn ultracode_reports_local_connection_errors() {
    let mut app = App::test_default();
    assert!(try_handle_submit(&mut app, "/ultracode on"));
    let MessageBlock::Text(block) = &app.transcript.messages.last().expect("error").blocks[0]
    else {
        panic!("text")
    };
    assert_eq!(block.text, "Cannot change Ultracode: not connected yet.");
    let _receiver = attach_test_connection(&mut app);
    assert!(try_handle_submit(&mut app, "/ultracode off"));
    let MessageBlock::Text(block) = &app.transcript.messages.last().expect("error").blocks[0]
    else {
        panic!("text")
    };
    assert_eq!(block.text, "Cannot change Ultracode: no active session.");
    assert!(app.turn.pending_command_ack.is_none());
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
fn parse_slash_name_and_args() {
    let parsed = parse("/mode plan").expect("slash command");
    assert_eq!(parsed.name, "/mode");
    assert_eq!(parsed.args, vec!["plan"]);
}

#[test]
fn resolved_submission_is_the_active_turn_policy_authority() {
    let classify = |input: &str| ResolvedSubmission::resolve(input.to_owned()).class();

    assert_eq!(classify("/cancel"), SubmissionClass::TurnControl);
    for input in ["/config", "/help", "/mcp", "/plugins", "/status", "/usage"] {
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
        "/docs nope",
        "/plugins extra",
        "/resume one two",
        "/rewind user-1 invalid",
    ] {
        assert_eq!(classify(input), SubmissionClass::Invalid, "unexpected class for {input}");
    }
}

#[test]
fn unsupported_command_is_handled_locally() {
    let mut app = App::test_default();
    let consumed = try_handle_submit(&mut app, "/definitely-unknown");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
}

#[test]
fn advertised_command_is_forwarded() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/remote-command", "Remote command")];
    let consumed = try_handle_submit(&mut app, "/remote-command");
    assert!(!consumed);
}

#[test]
fn advertised_command_alias_is_forwarded_and_offered() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands = vec![
        model::AvailableCommand::new("/remote-command", "Remote command")
            .aliases(vec!["/remote".to_owned()]),
    ];

    assert!(!try_handle_submit(&mut app, "/remote"));
    assert_eq!(
        find_advertised_command(&app, "/remote").map(|cmd| cmd.name.as_str()),
        Some("/remote-command")
    );
    assert!(
        supported_command_candidates(&app).iter().any(|candidate| candidate.primary == "/remote")
    );
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
fn login_logout_appear_in_candidates_as_builtins() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/agent"), "missing /agent");
    assert!(names.iter().any(|n| n == "/config"), "missing /config");
    assert!(names.iter().any(|n| n == "/docs"), "missing /docs");
    assert!(names.iter().any(|n| n == "/login"), "missing /login");
    assert!(names.iter().any(|n| n == "/logout"), "missing /logout");
    assert!(names.iter().any(|n| n == "/mcp"), "missing /mcp");
    assert!(names.iter().any(|n| n == "/plugins"), "missing /plugins");
    assert!(names.iter().any(|n| n == "/rewind"), "missing /rewind");
    assert!(names.iter().any(|n| n == "/usage"), "missing /usage");
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

/// The manual is read from disk rather than with `include_str!` so docs are not
/// embedded in the shipped binary. Only names and order are enforced; prose in
/// the other columns is free to diverge.
#[test]
fn docs_app_owned_command_table_matches_catalog() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/commands.md");
    let markdown = std::fs::read_to_string(path).expect("read docs/src/commands.md");

    let documented = documented_app_slash_commands(&markdown);
    let documented: Vec<&str> = documented.iter().map(String::as_str).collect();
    let expected: Vec<&str> = APP_SLASH_COMMANDS.iter().map(|spec| spec.name).collect();

    for name in &expected {
        assert!(
            documented.contains(name),
            "{name} is in APP_SLASH_COMMANDS but missing from the App-Owned Commands table in docs/src/commands.md"
        );
    }
    for name in &documented {
        assert!(
            expected.contains(name),
            "{name} is listed in the App-Owned Commands table in docs/src/commands.md but is not in APP_SLASH_COMMANDS"
        );
    }
    assert_eq!(
        documented, expected,
        "the App-Owned Commands table in docs/src/commands.md must list commands in APP_SLASH_COMMANDS order"
    );
}

#[test]
fn config_without_args_opens_settings_view() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/config");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
}

#[test]
fn app_config_shadows_advertised_config_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/config", "SDK config command").input_hint("<setting>")];

    let consumed = try_handle_submit(&mut app, "/config");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
}

#[test]
fn app_config_candidate_ignores_advertised_config_metadata() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/config", "SDK config command").input_hint("<setting>")];
    app.input.set_text("/config");
    let _ = app.input.set_cursor(0, "/config".chars().count());

    let slash = requested_slash_state(&app).expect("slash state");
    let config_candidates: Vec<_> =
        slash.candidates.iter().filter(|candidate| candidate.primary == "/config").collect();

    assert_eq!(config_candidates.len(), 1);
    assert_eq!(config_candidates[0].secondary.as_deref(), Some("Open settings"));
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

#[test]
fn app_fast_candidate_ignores_advertised_fast_metadata() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/fast", "SDK fast command").input_hint("<mode>")];
    app.input.set_text("/fast");
    let _ = app.input.set_cursor(0, "/fast".chars().count());

    let slash = requested_slash_state(&app).expect("slash state");
    let fast_candidates: Vec<_> =
        slash.candidates.iter().filter(|candidate| candidate.primary == "/fast").collect();

    assert_eq!(fast_candidates.len(), 1);
    assert_eq!(fast_candidates[0].secondary.as_deref(), Some("Toggle session fast mode"));
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
fn fast_rejects_invalid_arguments_without_dispatching() {
    let mut app = App::test_default();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(model::SessionId::new("sess-1"));

    let consumed = try_handle_submit(&mut app, "/fast invalid");

    assert!(consumed);
    assert!(rx.try_recv().is_err());
    assert!(!matches!(app.status, AppStatus::CommandPending));
    let last = app.transcript.messages.last().expect("usage message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /fast [on|off]");
}

#[test]
fn help_without_args_opens_help_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/help");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Help);
}

#[test]
fn config_with_extra_args_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/config extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /config");
}

#[test]
fn plugins_without_args_opens_plugins_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/plugins");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Plugins);
}

#[test]
fn mcp_opens_config_at_mcp_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/mcp");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Mcp);
}

#[test]
fn mcp_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/mcp extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /mcp");
}

#[test]
fn plugins_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/plugins extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /plugins");
}

#[tokio::test(flavor = "current_thread")]
async fn login_is_handled_as_builtin_and_sets_command_pending() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let consumed = try_handle_submit(&mut app, "/login");
            assert!(consumed, "/login should be handled locally");
            // Status becomes CommandPending (or stays Ready if claude CLI is not in PATH)
            assert!(
                matches!(app.status, AppStatus::CommandPending | AppStatus::Ready),
                "expected CommandPending or Ready, got {:?}",
                app.status
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn logout_is_handled_as_builtin_and_sets_command_pending() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let consumed = try_handle_submit(&mut app, "/logout");
            assert!(consumed, "/logout should be handled locally");
            assert!(
                matches!(app.status, AppStatus::CommandPending | AppStatus::Ready),
                "expected CommandPending or Ready, got {:?}",
                app.status
            );
        })
        .await;
}

#[test]
fn login_rejects_extra_args() {
    let mut app = App::test_default();
    let consumed = try_handle_submit(&mut app, "/login somearg");
    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    assert!(matches!(last.role, MessageRole::System(_)));
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
fn mode_argument_candidates_are_dynamic() {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(super::super::ModeState {
        current_mode_id: "plan".to_owned(),
        current_mode_name: "Plan".to_owned(),
        available_modes: vec![
            super::super::ModeInfo { id: "plan".to_owned(), name: "Plan".to_owned() },
            super::super::ModeInfo { id: "code".to_owned(), name: "Code".to_owned() },
        ],
    });

    let candidates = argument_candidates(&app, "/mode", 0);
    assert!(candidates.iter().any(|c| c.insert_value == "plan"));
    assert!(candidates.iter().any(|c| c.insert_value == "code"));
    assert!(candidates.iter().any(|c| c.primary == "Plan"));
    assert!(candidates.iter().any(|c| c.secondary.as_deref() == Some("plan")));
}

#[test]
fn model_argument_candidates_are_dynamic() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("sonnet", "Claude Sonnet")
            .description("Balanced coding model"),
        crate::agent::model::AvailableModel::new("opus", "Claude Opus"),
    ];
    let candidates = argument_candidates(&app, "/model", 0);
    assert!(candidates.iter().any(|c| c.insert_value == "sonnet"));
    assert!(candidates.iter().any(|c| c.primary == "Claude Sonnet"));
    assert!(candidates.iter().any(|c| c.secondary.as_deref() == Some("Balanced coding model")));
    assert!(candidates.iter().any(|c| c.insert_value == "opus"));
}

#[test]
fn model_argument_candidates_include_sdk_default_option() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("default", "Default")
            .description("Default (recommended)"),
        crate::agent::model::AvailableModel::new("sonnet", "Claude Sonnet"),
        crate::agent::model::AvailableModel::new("opus", "Claude Opus"),
    ];

    let candidates = argument_candidates(&app, "/model", 0);

    assert!(candidates.iter().any(|c| c.insert_value == "default"));
    assert!(candidates.iter().any(|c| c.primary == "Default"));
    assert!(candidates.iter().any(|c| c.secondary.as_deref() == Some("Default (recommended)")));
    assert!(candidates.iter().any(|c| c.insert_value == "sonnet"));
    assert!(candidates.iter().any(|c| c.insert_value == "opus"));
}

#[test]
fn model_argument_candidates_keep_sdk_opus_description_when_unpinned() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("opus", "Opus")
            .description("Opus 4.7 · Most capable for complex work"),
    ];

    let candidates = argument_candidates(&app, "/model", 0);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].insert_value, "opus");
    assert_eq!(
        candidates[0].secondary.as_deref(),
        Some("Opus 4.7 · Most capable for complex work")
    );
}

#[test]
fn agent_argument_candidates_include_reset_and_available_agents() {
    let mut app = App::test_default();
    app.sdk_inventory.available_agents = vec![
        crate::agent::model::AvailableAgent::new("reviewer", "Review code").model("claude-opus"),
        crate::agent::model::AvailableAgent::new("planner", "Plan work"),
    ];

    let candidates = argument_candidates(&app, "/agent", 0);

    assert!(candidates.iter().any(|candidate| {
        candidate.insert_value == "reset"
            && candidate.secondary.as_deref() == Some("Clear active agent")
    }));
    assert!(candidates.iter().any(|candidate| {
        candidate.insert_value == "reviewer"
            && candidate.primary == "reviewer"
            && candidate.secondary.as_deref() == Some("Review code - claude-opus")
    }));
    assert!(candidates.iter().any(|candidate| candidate.insert_value == "planner"));
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
fn rewind_argument_context_requests_targets_when_cache_is_stale() {
    let mut app = App::test_default();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.input.set_text("/rewind ");
    let _ = app.input.set_cursor(0, "/rewind ".chars().count());

    sync_with_cursor(&mut app);

    assert!(app.sdk_inventory.rewind_targets_in_flight);
    let envelope = rx.try_recv().expect("rewind target request");
    assert!(matches!(
        envelope.command,
        crate::agent::wire::BridgeCommand::GetRewindTargets { session_id }
            if session_id == "session-1"
    ));
}

#[test]
fn rewind_argument_context_shows_loading_while_request_is_in_flight() {
    let mut app = App::test_default();
    let _rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.input.set_text("/rewind ");
    let _ = app.input.set_cursor(0, "/rewind ".chars().count());

    sync_with_cursor(&mut app);

    let slash = app.slash.visible().expect("slash state");
    assert!(slash.candidates.is_empty());
    assert_eq!(slash.placeholder.as_deref(), Some("Loading messages"));
}

#[test]
fn rewind_argument_context_shows_no_previous_messages_when_loaded_empty() {
    let mut app = App::test_default();
    let session_id = model::SessionId::new("session-1");
    app.session_runtime.session_id = Some(session_id.clone());
    app.sdk_inventory.rewind_targets_session_id = Some(session_id);
    app.input.set_text("/rewind ");
    let _ = app.input.set_cursor(0, "/rewind ".chars().count());

    let slash = requested_slash_state(&app).expect("slash state");

    assert!(slash.candidates.is_empty());
    assert_eq!(slash.placeholder.as_deref(), Some("No previous user messages"));
}

#[test]
fn rewind_argument_context_shows_no_matching_messages_for_filtered_empty_result() {
    let mut app = App::test_default();
    let session_id = model::SessionId::new("session-1");
    app.session_runtime.session_id = Some(session_id.clone());
    app.sdk_inventory.rewind_targets_session_id = Some(session_id);
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }];
    app.input.set_text("/rewind missing");
    let _ = app.input.set_cursor(0, "/rewind missing".chars().count());

    let slash = requested_slash_state(&app).expect("slash state");

    assert!(slash.candidates.is_empty());
    assert_eq!(slash.placeholder.as_deref(), Some("No matching messages"));
}

#[test]
fn effort_argument_candidates_include_session_only_max() {
    let mut app = App::test_default();
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
            .supports_effort(true)
            .supported_effort_levels(vec![
                crate::agent::model::EffortLevel::Low,
                crate::agent::model::EffortLevel::Medium,
                crate::agent::model::EffortLevel::High,
                crate::agent::model::EffortLevel::XHigh,
                crate::agent::model::EffortLevel::Max,
            ]),
    );

    let candidates = argument_candidates(&app, "/effort", 0);

    assert_eq!(
        candidates.iter().map(|candidate| candidate.insert_value.as_str()).collect::<Vec<_>>(),
        vec!["low", "medium", "high", "xhigh", "max", "reset"]
    );
    assert!(candidates.iter().any(|candidate| {
        candidate.insert_value == "max"
            && candidate.secondary.as_deref() == Some("Max - Maximum effort")
    }));
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
fn docs_argument_candidates_are_static_topics() {
    let app = App::test_default();
    let candidates = argument_candidates(&app, "/docs", 0);
    assert!(candidates.iter().any(|c| c.insert_value == "mode"));
    assert!(candidates.iter().any(|c| c.insert_value == "models"));
    assert!(candidates.iter().any(|c| c.insert_value == "shortcuts"));
    assert!(candidates.iter().any(|c| c.insert_value == "commands"));
    assert!(candidates.iter().any(|c| c.insert_value == "agents"));
}

#[test]
fn docs_without_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /docs <mode|models|shortcuts|commands|agents>");
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
fn docs_with_unknown_topic_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs nope");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /docs <mode|models|shortcuts|commands|agents>");
}

#[test]
fn docs_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs commands extra");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /docs <mode|models|shortcuts|commands|agents>");
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
fn variable_command_argument_mode_stays_active_without_matches() {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(super::super::ModeState {
        current_mode_id: "plan".to_owned(),
        current_mode_name: "Plan".to_owned(),
        available_modes: vec![super::super::ModeInfo {
            id: "plan".to_owned(),
            name: "Plan".to_owned(),
        }],
    });
    app.input.set_text("/mode xyz");
    let _ = app.input.set_cursor(0, "/mode xyz".chars().count());
    sync_with_cursor(&mut app);
    let slash = app.slash.visible().expect("slash state should stay active for empty result hint");
    assert!(slash.candidates.is_empty());
}

#[test]
fn confirm_selection_replaces_only_active_argument_token() {
    let mut app = App::test_default();
    app.input.set_text("/resume old-id trailing");
    let _ = app.input.set_cursor(0, "/resume old-id".chars().count());
    app.slash.show(SlashState {
        trigger_row: 0,
        trigger_col: 8,
        query: "old-id".to_owned(),
        context: SlashContext::Argument {
            command: "/resume".to_owned(),
            arg_index: 0,
            token_range: (8, 14),
        },
        candidates: vec![SlashCandidate {
            insert_value: "new-id".to_owned(),
            primary: "New".to_owned(),
            secondary: None,
        }],
        placeholder: None,
        dialog: DialogState::default(),
    });

    confirm_selection(&mut app);

    assert_eq!(app.input.text(), "/resume new-id trailing");
}

#[tokio::test(flavor = "current_thread")]
async fn login_is_handled_as_builtin_even_when_advertised() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            app.sdk_inventory.available_commands =
                vec![model::AvailableCommand::new("/login", "Login")];

            let consumed = try_handle_submit(&mut app, "/login");
            assert!(consumed, "/login should be handled locally even when SDK advertises it");
        })
        .await;
}

#[test]
fn new_session_command_is_rendered_as_user_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/new-session");
    assert!(consumed);
    assert!(app.transcript.messages.len() >= 2);

    let Some(first) = app.transcript.messages.first() else {
        panic!("expected first message");
    };
    assert!(matches!(first.role, MessageRole::User));
    let Some(MessageBlock::Text(block)) = first.blocks.first() else {
        panic!("expected user text block");
    };
    assert_eq!(block.text, "/new-session");
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
    let MessageBlock::Text(block) = last.blocks.first().expect("text block") else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "/resume");
}

#[test]
fn resume_picker_preselects_current_session_when_listed() {
    let mut app = App::test_default();
    let _receiver = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
    app.recent_sessions = vec![
        crate::app::RecentSessionInfo {
            session_id: "other-session".into(),
            summary: "other".into(),
            last_modified_ms: 2,
            file_size_bytes: 1,
            cwd: None,
            git_branch: None,
            custom_title: None,
            first_prompt: Some("other".into()),
        },
        crate::app::RecentSessionInfo {
            session_id: "current-session".into(),
            summary: "current".into(),
            last_modified_ms: 1,
            file_size_bytes: 1,
            cwd: None,
            git_branch: None,
            custom_title: None,
            first_prompt: Some("current".into()),
        },
    ];

    assert!(try_handle_submit(&mut app, "/resume"));

    assert_eq!(app.session_picker.selected, 1);
}

#[test]
fn resume_with_extra_args_returns_usage() {
    let mut app = App::test_default();
    let consumed = try_handle_submit(&mut app, "/resume abc-123 extra");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /resume [session_id]");
}

#[test]
fn rewind_with_missing_target_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/rewind");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /rewind <user_message_uuid> <both|conversation|code>");
}

#[test]
fn rewind_with_cached_target_requires_connection() {
    let mut app = App::test_default();
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }];

    let consumed = try_handle_submit(&mut app, "/rewind user-1 conversation");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected selection message");
    };
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Error))));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Cannot rewind: not connected yet.");
}

#[test]
fn rewind_with_cached_target_sends_bridge_command() {
    let mut app = App::test_default();
    let mut rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }];

    let consumed = try_handle_submit(&mut app, "/rewind user-1 conversation");

    assert!(consumed);
    assert_eq!(app.turn.pending_command_label.as_deref(), Some("Rewinding conversation..."));
    let envelope = rx.try_recv().expect("rewind command");
    let crate::agent::wire::BridgeCommand::Rewind {
        session_id,
        target_user_message_id,
        restore_mode,
        ..
    } = envelope.command
    else {
        panic!("expected rewind command");
    };
    assert_eq!(session_id, "session-1");
    assert_eq!(target_user_message_id, "user-1");
    assert_eq!(restore_mode, crate::agent::types::RewindRestoreMode::Conversation);
}

#[test]
fn resume_command_is_rendered_as_user_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/resume abc-123");
    assert!(consumed);
    assert!(app.transcript.messages.len() >= 2);

    let Some(first) = app.transcript.messages.first() else {
        panic!("expected user message");
    };
    assert!(matches!(first.role, MessageRole::User));
    let Some(MessageBlock::Text(block)) = first.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "/resume abc-123");
}

#[tokio::test(flavor = "current_thread")]
async fn resume_sets_command_pending_when_connected() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let mut rx = attach_test_connection(&mut app);

            let consumed = try_handle_submit(&mut app, "/resume abc-123");
            assert!(consumed);
            assert!(matches!(app.status, AppStatus::CommandPending));
            assert_eq!(app.pending_session_resume_id(), Some("abc-123"));

            tokio::task::yield_now().await;
            assert!(rx.try_recv().is_ok());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn mode_sets_command_pending_and_mode_update_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let _rx = attach_test_connection(&mut app);
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.mode = Some(super::super::ModeState {
                current_mode_id: "default".to_owned(),
                current_mode_name: "Default".to_owned(),
                available_modes: vec![
                    super::super::ModeInfo { id: "plan".to_owned(), name: "Plan".to_owned() },
                    super::super::ModeInfo { id: "default".to_owned(), name: "Default".to_owned() },
                ],
            });

            let consumed = try_handle_submit(&mut app, "/mode plan");
            assert!(consumed);
            assert!(
                matches!(app.status, AppStatus::CommandPending),
                "expected CommandPending, got {:?}",
                app.status
            );
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching mode..."));

            // Simulate mode-update ack arriving from bridge.
            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::ModeStateUpdate(
                    super::super::ModeState {
                        current_mode_id: "plan".to_owned(),
                        current_mode_name: "Plan".to_owned(),
                        available_modes: vec![super::super::ModeInfo {
                            id: "plan".to_owned(),
                            name: "Plan".to_owned(),
                        }],
                    },
                )),
            );
            assert!(
                matches!(app.status, AppStatus::Ready),
                "expected Ready after ModeStateUpdate ack, got {:?}",
                app.status
            );
            assert!(app.turn.pending_command_label.is_none());
        })
        .await;
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
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching model..."));
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
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching effort..."));
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
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching agent..."));
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

#[test]
fn effort_invalid_arguments_return_usage() {
    for input in ["/effort", "/effort banana", "/effort high extra"] {
        let mut app = App::test_default();

        let consumed = try_handle_submit(&mut app, input);

        assert!(consumed);
        let Some(last) = app.transcript.messages.last() else {
            panic!("expected system usage message for {input}");
        };
        let Some(MessageBlock::Text(block)) = last.blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Usage: /effort <low|medium|high|xhigh|max|reset>");
        assert!(!matches!(app.status, AppStatus::CommandPending));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn new_session_sets_command_pending() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let _rx = attach_test_connection(&mut app);

            let consumed = try_handle_submit(&mut app, "/new-session");
            assert!(consumed);
            assert!(
                matches!(app.status, AppStatus::CommandPending),
                "expected CommandPending, got {:?}",
                app.status
            );
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Starting new session..."));
        })
        .await;
}

#[test]
fn compact_without_connection_is_handled_locally() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/compact");
    assert!(consumed);
    assert!(!app.turn.compaction.is_active());
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Cannot compact: not connected yet.");
}

#[test]
fn compact_with_active_session_starts_manual_compaction() {
    let mut app = App::test_default();
    let _rx = attach_test_connection(&mut app);
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));

    let consumed = try_handle_submit(&mut app, "/compact");
    assert!(!consumed);
    assert!(app.turn.compaction.is_active());
}

#[test]
fn compact_with_args_returns_usage_message() {
    let mut app = App::test_default();
    app.transcript.messages.push(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete("keep"))],
        None,
    ));

    let consumed = try_handle_submit(&mut app, "/compact now");
    assert!(consumed);
    assert!(app.transcript.messages.len() >= 2);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /compact");
}

#[test]
fn mode_with_extra_args_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/mode plan extra");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /mode <id>");
}

#[test]
fn model_with_missing_id_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/model");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /model <id>");
}

#[test]
fn model_with_extra_args_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/model sonnet extra");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /model <id>");
}

#[test]
fn confirm_selection_with_invalid_trigger_row_is_noop() {
    let mut app = App::test_default();
    app.input.set_text("/mode");
    app.slash.show(SlashState {
        trigger_row: 99,
        trigger_col: 0,
        query: "m".into(),
        context: SlashContext::CommandName,
        candidates: vec![SlashCandidate {
            insert_value: "/mode".into(),
            primary: "/mode".into(),
            secondary: None,
        }],
        placeholder: None,
        dialog: DialogState::default(),
    });

    confirm_selection(&mut app);

    assert_eq!(app.input.text(), "/mode");
}

#[test]
fn docs_command_confirm_enters_argument_mode() {
    let mut app = App::test_default();
    app.input.set_text("/do");
    let _ = app.input.set_cursor(0, "/do".chars().count());
    app.slash.show(SlashState {
        trigger_row: 0,
        trigger_col: 0,
        query: "do".into(),
        context: SlashContext::CommandName,
        candidates: vec![SlashCandidate {
            insert_value: "/docs".into(),
            primary: "/docs".into(),
            secondary: Some("Show in-chat help topics".into()),
        }],
        placeholder: None,
        dialog: DialogState::default(),
    });

    confirm_selection(&mut app);

    assert_eq!(app.input.text(), "/docs ");
    let slash = app.slash.visible().expect("topic autocomplete should activate");
    match &slash.context {
        SlashContext::Argument { command, arg_index, .. } => {
            assert_eq!(command, "/docs");
            assert_eq!(*arg_index, 0);
        }
        SlashContext::CommandName => panic!("expected argument autocomplete"),
    }
    assert!(slash.candidates.iter().any(|candidate| candidate.insert_value == "mode"));
}

#[test]
fn single_argument_builtin_selection_closes_autocomplete() {
    for (command, value) in [
        ("/docs", "commands"),
        ("/agent", "reviewer"),
        ("/effort", "xhigh"),
        ("/mode", "plan"),
        ("/model", "sonnet"),
        ("/resume", "session-1"),
    ] {
        let mut app = App::test_default();
        let input = format!("{command} ");
        app.input.set_text(&input);
        let _ = app.input.set_cursor(0, input.chars().count());
        app.slash.show(SlashState {
            trigger_row: 0,
            trigger_col: input.chars().count(),
            query: String::new(),
            context: SlashContext::Argument {
                command: command.to_owned(),
                arg_index: 0,
                token_range: (input.chars().count(), input.chars().count()),
            },
            candidates: vec![SlashCandidate {
                insert_value: value.to_owned(),
                primary: value.to_owned(),
                secondary: None,
            }],
            placeholder: None,
            dialog: DialogState::default(),
        });

        confirm_selection(&mut app);

        assert_eq!(app.input.text(), format!("{command} {value} "));
        assert!(app.slash.visible().is_none(), "{command} should close after first argument");
    }
}

#[test]
fn status_opens_config_at_status_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/status");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Status);
}

#[test]
fn usage_opens_config_at_usage_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/usage");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Usage);
}

#[test]
fn status_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/status extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /status");
}

#[test]
fn usage_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/usage extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /usage");
}

#[test]
fn status_appears_in_candidates() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/status"), "missing /status");
}

#[test]
fn usage_appears_in_candidates() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/usage"), "missing /usage");
}

#[test]
fn mcp_appears_in_candidates() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/mcp"), "missing /mcp");
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
        assert!(row.contains("[Opus/High \u{b7} Ultracode]"), "{row}");

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
