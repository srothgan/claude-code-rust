// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{
    client::CommandReceiver,
    events::ClientEvent,
    wire::{BridgeCommand, CommandEnvelope},
};

fn fixture() -> (App, CommandReceiver) {
    let (mut app, commands) = crate::app::test_support::app_with_connection();
    app.session_runtime.session_id = Some("control-session".into());
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/remote", "Remote workflow")];
    (app, commands)
}

fn submit(app: &mut App, text: &str) {
    app.input.set_text(text);
    crate::app::input_submit::submit_input(app);
}

async fn received(commands: &mut CommandReceiver) -> CommandEnvelope {
    tokio::time::timeout(std::time::Duration::from_secs(2), commands.recv_envelope())
        .await
        .expect("command deadline")
        .expect("command receiver")
}

fn no_command(commands: &mut CommandReceiver) {
    assert!(matches!(commands.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
}

fn update(app: &mut App, update: model::SessionUpdate) {
    crate::app::handle_client_event(
        app,
        ClientEvent::SessionUpdate { session_id: "control-session".into(), update },
    );
}

fn block_second_submission(app: &mut App, commands: &mut CommandReceiver) {
    assert_eq!(app.status, AppStatus::CommandPending);
    submit(app, "/remote keep this draft");
    assert_eq!(app.input.text(), "/remote keep this draft");
    no_command(commands);
}

async fn prove_reusable(app: &mut App, commands: &mut CommandReceiver) {
    assert_eq!(app.status, AppStatus::Ready);
    submit(app, "/remote after acknowledgement");
    let BridgeCommand::Prompt { session_id, chunks, .. } = received(commands).await.command else {
        panic!("usable Prompt path")
    };
    assert_eq!(session_id, "control-session");
    assert!(
        matches!(chunks.as_slice(), [chunk] if chunk.kind == "text" && chunk.value == json!("/remote after acknowledgement"))
    );
    assert!(app.input.is_empty());
    while let Ok(envelope) = commands.try_recv() {
        assert!(
            matches!(envelope.command, BridgeCommand::GetContextUsage { session_id } if session_id == "control-session")
        );
    }
    no_command(commands);
}

fn mode(id: &str, name: &str) -> crate::app::ModeState {
    crate::app::ModeState {
        current_mode_id: id.into(),
        current_mode_name: name.into(),
        available_modes: vec![
            crate::app::ModeInfo { id: "plan".into(), name: "Confirmed planning".into() },
            crate::app::ModeInfo { id: "acceptEdits".into(), name: "New editing choice".into() },
        ],
    }
}

fn original_control_fixture() -> (App, CommandReceiver) {
    let (mut app, commands) = fixture();
    app.session_runtime.mode = Some(crate::app::ModeState {
        current_mode_id: "default".into(),
        current_mode_name: "Old default".into(),
        available_modes: vec![
            crate::app::ModeInfo { id: "default".into(), name: "Old default".into() },
            crate::app::ModeInfo { id: "plan".into(), name: "Old plan".into() },
        ],
    });
    app.session_runtime.current_model =
        Some(model::CurrentModel::new("old-model", "Old model", "Old model").authoritative(true));
    app.session_runtime.config_options.insert("effortLevel".into(), json!("high"));
    (app, commands)
}

async fn follow_acknowledged_mode(app: &mut App, commands: &mut CommandReceiver) {
    let confirmed = app.session_runtime.mode.as_ref().unwrap();
    assert_eq!(confirmed.current_mode_id, "plan");
    assert_eq!(confirmed.current_mode_name, "Confirmed planning");
    let candidates = argument_candidates(app, "/mode", 0);
    let choices = candidates
        .iter()
        .map(|c| (c.insert_value.as_str(), c.primary.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        choices,
        std::collections::BTreeMap::from([
            ("plan", "Confirmed planning"),
            ("acceptEdits", "New editing choice")
        ])
    );
    app.input.set_text("/mode default");
    assert!(requested_slash_state(app).unwrap().candidates.is_empty());
    submit(app, "/mode acceptEdits");
    assert_eq!(
        received(commands).await.command,
        BridgeCommand::SetMode { session_id: "control-session".into(), mode: "acceptEdits".into() }
    );
    assert_eq!(app.session_runtime.mode.as_ref().unwrap().current_mode_id, "plan");
    update(app, model::SessionUpdate::ModeStateUpdate(mode("acceptEdits", "Confirmed editing")));
    assert_eq!(app.session_runtime.mode.as_ref().unwrap().current_mode_name, "Confirmed editing");
}

fn assert_original_controls(app: &App) {
    assert_eq!(app.session_runtime.mode.as_ref().unwrap().current_mode_id, "default");
    assert_eq!(app.session_runtime.current_model.as_ref().unwrap().resolved_id, "old-model");
    assert_eq!(app.session_runtime.config_options.get("effortLevel"), Some(&json!("high")));
}

#[tokio::test(flavor = "current_thread")]
async fn session_controls_dispatch_before_ack_preserve_confirmed_values_and_release_submission() {
    tokio::task::LocalSet::new()
        .run_until(Box::pin(async {
            for (input, expected, acknowledged) in [
                (
                    "/mode plan",
                    BridgeCommand::SetMode {
                        session_id: "control-session".into(),
                        mode: "plan".into(),
                    },
                    model::SessionUpdate::ModeStateUpdate(mode("plan", "Confirmed planning")),
                ),
                (
                    "/model sonnet",
                    BridgeCommand::SetModel {
                        session_id: "control-session".into(),
                        model: "sonnet".into(),
                    },
                    model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
                        model::CurrentModel::new("sonnet", "Sonnet", "Sonnet").authoritative(true),
                    )),
                ),
                (
                    "/effort xhigh",
                    BridgeCommand::SetEffort {
                        session_id: "control-session".into(),
                        effort: Some("xhigh".into()),
                    },
                    model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                        option_id: "effortLevel".into(),
                        value: json!("xhigh"),
                    }),
                ),
            ] {
                let (mut app, mut commands) = original_control_fixture();
                submit(&mut app, input);
                assert_eq!(app.displayed_status(), AppStatus::Ready);
                assert_eq!(received(&mut commands).await.command, expected);
                assert_original_controls(&app);
                block_second_submission(&mut app, &mut commands);
                crate::app::handle_client_event(
                    &mut app,
                    ClientEvent::SessionUpdate {
                        session_id: "foreign-session".into(),
                        update: acknowledged.clone(),
                    },
                );
                update(
                    &mut app,
                    model::SessionUpdate::FastModeUpdate {
                        state: model::FastModeState::Off,
                        disabled_reason: None,
                    },
                );
                assert_eq!(
                    app.status,
                    AppStatus::CommandPending,
                    "unrelated telemetry must not acknowledge {input}"
                );
                assert_original_controls(&app);
                update(&mut app, acknowledged);
                match input {
                    "/mode plan" => follow_acknowledged_mode(&mut app, &mut commands).await,
                    "/model sonnet" => assert_eq!(
                        app.session_runtime.current_model.as_ref().unwrap().resolved_id,
                        "sonnet"
                    ),
                    "/effort xhigh" => {
                        assert_eq!(
                            app.session_runtime.config_options.get("effortLevel"),
                            Some(&json!("xhigh"))
                        );
                        assert_eq!(app.session_effort(), Some(model::EffortLevel::XHigh));
                    }
                    _ => unreachable!(),
                }
                no_command(&mut commands);
                prove_reusable(&mut app, &mut commands).await;
            }
        }))
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_selection_reset_and_unadvertised_names_wait_for_bridge_confirmation() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (input, requested, acknowledged) in [
                ("/agent reviewer", Some("reviewer"), json!("reviewer")),
                ("/agent reset", None, json!(null)),
                ("/agent custom-agent", Some("custom-agent"), json!("custom-agent")),
            ] {
                let (mut app, mut commands) = fixture();
                app.sdk_inventory.available_agents =
                    vec![model::AvailableAgent::new("reviewer", "Known reviewer")];
                app.session_runtime.config_options.insert("agent".into(), json!("previous-agent"));
                submit(&mut app, input);
                assert_eq!(app.displayed_status(), AppStatus::Ready);
                assert_eq!(
                    received(&mut commands).await.command,
                    BridgeCommand::SetAgent {
                        session_id: "control-session".into(),
                        agent: requested.map(str::to_owned)
                    }
                );
                assert_eq!(
                    app.session_runtime.config_options.get("agent"),
                    Some(&json!("previous-agent"))
                );
                block_second_submission(&mut app, &mut commands);
                update(
                    &mut app,
                    model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                        option_id: "effortLevel".into(),
                        value: json!("low"),
                    }),
                );
                assert_eq!(app.status, AppStatus::CommandPending);
                update(
                    &mut app,
                    model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                        option_id: "agent".into(),
                        value: acknowledged.clone(),
                    }),
                );
                assert_eq!(app.session_runtime.config_options.get("agent"), Some(&acknowledged));
                prove_reusable(&mut app, &mut commands).await;
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn app_fast_collision_toggles_confirmed_state_only_after_the_matching_snapshot() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (initial, enabled, confirmed) in [
                (model::FastModeState::Off, true, model::FastModeState::On),
                (model::FastModeState::On, false, model::FastModeState::Off),
                (model::FastModeState::Cooldown, false, model::FastModeState::Off),
            ] {
                let (mut app, mut commands) = fixture();
                app.sdk_inventory
                    .available_commands
                    .push(model::AvailableCommand::new("/fast", "SDK fast collision"));
                app.session_runtime.fast_mode_state = initial;
                submit(&mut app, "/fast");
                assert_eq!(
                    received(&mut commands).await.command,
                    BridgeCommand::SetFastMode { session_id: "control-session".into(), enabled }
                );
                assert_eq!(app.session_runtime.fast_mode_state, initial);
                block_second_submission(&mut app, &mut commands);
                crate::app::handle_client_event(
                    &mut app,
                    ClientEvent::SessionUpdate {
                        session_id: "foreign-session".into(),
                        update: model::SessionUpdate::FastModeUpdate {
                            state: confirmed,
                            disabled_reason: None,
                        },
                    },
                );
                assert_eq!(app.status, AppStatus::CommandPending);
                assert_eq!(app.session_runtime.fast_mode_state, initial);
                update(
                    &mut app,
                    model::SessionUpdate::FastModeUpdate {
                        state: confirmed,
                        disabled_reason: None,
                    },
                );
                assert_eq!(app.session_runtime.fast_mode_state, confirmed);
                prove_reusable(&mut app, &mut commands).await;
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn reload_blocks_second_submission_until_its_session_result_and_allows_reuse() {
    use crate::agent::types::ReloadOutcome;
    tokio::task::LocalSet::new()
        .run_until(async {
            for outcome in ["success", "failure", "expired"] {
                let (mut app, mut commands) = fixture();
                submit(&mut app, "/reload");
                assert_eq!(
                    received(&mut commands).await.command,
                    BridgeCommand::ReloadSkillsAndStyles { session_id: "control-session".into() }
                );
                block_second_submission(&mut app, &mut commands);
                crate::app::handle_client_event(
                    &mut app,
                    ClientEvent::SkillsAndStylesReloaded {
                        session_id: "foreign-session".into(),
                        skills: ReloadOutcome::Reloaded { names: vec![] },
                        output_styles: ReloadOutcome::Reloaded { names: vec![] },
                    },
                );
                assert_eq!(app.status, AppStatus::CommandPending);
                let event = match outcome {
                    "success" => ClientEvent::SkillsAndStylesReloaded {
                        session_id: "control-session".into(),
                        skills: ReloadOutcome::Reloaded { names: vec!["skill-a".into()] },
                        output_styles: ReloadOutcome::Reloaded { names: vec!["style-b".into()] },
                    },
                    "failure" => ClientEvent::SlashCommandError {
                        session_id: Some("control-session".into()),
                        message: "fixture reload rejected".into(),
                    },
                    _ => ClientEvent::SkillsAndStylesReloadExpired {
                        session_id: "control-session".into(),
                        after_ms: 2000,
                    },
                };
                crate::app::handle_client_event(&mut app, event);
                prove_reusable(&mut app, &mut commands).await;
            }
        })
        .await;
}

#[test]
fn ultracode_status_distinguishes_snapshots_and_effort_without_dispatching_a_mutation() {
    for (snapshot, required, absent) in [
        (None, vec!["unknown"], vec!["xhigh"]),
        (
            model::UltracodeState::new(true, false, false),
            vec!["off", "available"],
            vec!["unavailable", "xhigh"],
        ),
        (model::UltracodeState::new(true, true, true), vec!["on", "xhigh"], vec!["unavailable"]),
        (
            model::UltracodeState::new(false, true, false),
            vec!["requested", "unavailable"],
            vec!["xhigh"],
        ),
        (
            model::UltracodeState::new(false, false, false),
            vec!["off", "unavailable"],
            vec!["requested", "xhigh"],
        ),
    ] {
        let (mut app, mut commands) = fixture();
        app.session_runtime.ultracode = snapshot;
        app.session_runtime.config_options.insert("effortLevel".into(), json!("xhigh"));
        app.status = AppStatus::Running;
        submit(&mut app, "/ultracode status");
        let message = app.transcript.messages.last().expect("status feedback");
        assert_eq!(message.role, MessageRole::System(Some(SystemSeverity::Info)));
        let MessageBlock::Text(text) = &message.blocks[0] else { panic!("status text") };
        let words = text
            .text
            .split(|c: char| !c.is_alphanumeric())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(words.contains("Ultracode"));
        for word in required {
            assert!(words.contains(word), "missing {word}: {}", text.text);
        }
        for word in absent {
            assert!(!words.contains(word), "unexpected {word}: {}", text.text);
        }
        assert_eq!(app.status, AppStatus::Running);
        assert_eq!(app.session_runtime.ultracode, snapshot);
        no_command(&mut commands);
    }
}

fn footer_words(app: &App) -> std::collections::BTreeSet<String> {
    use ratatui::{backend::TestBackend, layout::Rect, widgets::Paragraph};
    let mut terminal = ratatui::Terminal::new(TestBackend::new(124, 6)).expect("terminal");
    terminal
        .draw(|frame| {
            frame.render_widget(
                Paragraph::new(
                    crate::ui::footer_rows::serialize_footer_rows(app, 120).rows[0].clone(),
                ),
                Rect::new(2, 4, 120, 1),
            );
        })
        .expect("footer draw");
    let text = (2..122).map(|x| terminal.backend().buffer()[(x, 4)].symbol()).collect::<String>();
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn status_text(app: &App) -> &str {
    let MessageBlock::Text(text) = &app.transcript.messages.last().expect("status").blocks[0]
    else {
        panic!("status text")
    };
    &text.text
}

#[tokio::test(flavor = "current_thread")]
async fn ultracode_wire_acknowledgements_preserve_effort_and_drive_status_and_footer() {
    tokio::task::LocalSet::new().run_until(Box::pin(async {
        let (mut app, mut commands) = fixture();
        app.session_runtime.current_model = Some(model::CurrentModel::new("opus", "Opus", "Opus").supports_effort(true));
        app.session_runtime.mode = Some(mode("default", "Default"));
        app.session_runtime.config_options.insert("effortLevel".into(), json!("high"));
        let off = model::UltracodeState::new(true, false, false);
        let on = model::UltracodeState::new(true, true, true);
        app.session_runtime.ultracode = off;
        let before = app.transcript.messages.iter().map(|m| m.id).collect::<Vec<_>>();
        submit(&mut app, "/ultracode on");
        assert_eq!(app.status, AppStatus::CommandPending);
        assert_eq!(app.session_runtime.ultracode, off);
        assert_eq!(serde_json::to_value(received(&mut commands).await).expect("wire command"), json!({"command":"set_ultracode","session_id":"control-session","enabled":true}));
        block_second_submission(&mut app, &mut commands);
        let wire: crate::agent::wire::EventEnvelope = serde_json::from_value(json!({"event":"session_update","session_id":"control-session","update":{"type":"ultracode_update","ultracode":{"available":true,"requested":true,"effective":true}}})).expect("wire snapshot");
        let crate::agent::wire::BridgeEvent::SessionUpdate { session_id, update: crate::agent::types::SessionUpdate::UltracodeUpdate { ultracode } } = wire.event else { panic!("Ultracode wire event") };
        crate::app::handle_client_event(&mut app, ClientEvent::SessionUpdate { session_id, update: model::SessionUpdate::UltracodeUpdate { ultracode } });
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(app.session_runtime.ultracode, on);
        assert_eq!(app.transcript.messages.iter().map(|m| m.id).collect::<Vec<_>>(), before, "success must not create permanent transcript messages");
        let footer = footer_words(&app);
        for value in ["Opus", "High", "Ultracode"] { assert!(footer.contains(value), "footer missing {value}: {footer:?}"); }
        submit(&mut app, "/effort low");
        assert_eq!(received(&mut commands).await.command, BridgeCommand::SetEffort { session_id: "control-session".into(), effort: Some("low".into()) });
        update(&mut app, model::SessionUpdate::UltracodeUpdate { ultracode: on });
        assert_eq!(app.status, AppStatus::CommandPending);
        update(&mut app, model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate { option_id: "effortLevel".into(), value: json!("low") }));
        submit(&mut app, "/ultracode status");
        assert!(status_text(&app).split(|c: char| !c.is_alphanumeric()).any(|word| word == "on") && status_text(&app).contains("low"));
        app.status = AppStatus::Running;
        submit(&mut app, "/ultracode off");
        no_command(&mut commands);
        assert_eq!(app.session_runtime.ultracode, on);
        assert_eq!(app.input.text(), "/ultracode off");
        submit(&mut app, "/ultracode status");
        assert_eq!(app.status, AppStatus::Running);
        app.status = AppStatus::Ready;
        submit(&mut app, "/ultracode off");
        assert_eq!(received(&mut commands).await.command, BridgeCommand::SetUltracode { session_id: "control-session".into(), enabled: false });
        assert_eq!(app.status, AppStatus::CommandPending);
        assert_eq!(app.session_runtime.ultracode, on);
        update(&mut app, model::SessionUpdate::UltracodeUpdate { ultracode: off });
        assert_eq!(app.status, AppStatus::Ready);
        assert!(!footer_words(&app).contains("Ultracode"));
        submit(&mut app, "/ultracode on");
        assert_eq!(received(&mut commands).await.command, BridgeCommand::SetUltracode { session_id: "control-session".into(), enabled: true });
        crate::app::handle_client_event(&mut app, ClientEvent::SlashCommandError { session_id: Some("control-session".into()), message: "dynamic workflows disabled".into() });
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(app.session_runtime.ultracode, off);
        submit(&mut app, "/ultracode on");
        assert_eq!(received(&mut commands).await.command, BridgeCommand::SetUltracode { session_id: "control-session".into(), enabled: true });
        let unavailable = model::UltracodeState::new(false, true, false);
        update(&mut app, model::SessionUpdate::UltracodeUpdate { ultracode: unavailable });
        crate::app::handle_client_event(&mut app, ClientEvent::SlashCommandError { session_id: Some("control-session".into()), message: "Ultracode unavailable; saved request remains inactive".into() });
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(app.session_runtime.ultracode, unavailable);
        assert!(app.transcript.messages.iter().flat_map(|m| &m.blocks).any(|b| matches!(b, MessageBlock::Notice(n) if n.severity == SystemSeverity::Error && n.text.text.contains("unavailable") && n.text.text.contains("inactive"))));
        assert!(!footer_words(&app).contains("Ultracode"));
        submit(&mut app, "/ultracode status");
        assert_eq!(app.transcript.messages.last().unwrap().role, MessageRole::System(Some(SystemSeverity::Info)));
        assert!(status_text(&app).contains("requested") && status_text(&app).contains("unavailable"));
        no_command(&mut commands);
    })).await;
}

#[test]
fn rewind_submissions_use_the_session_owned_target_restore_mode_and_launch_settings() {
    use crate::agent::types::RewindRestoreMode;
    use clap::Parser;
    for (input, expected) in [
        ("/rewind chosen conversation", RewindRestoreMode::Conversation),
        ("/rewind chosen code", RewindRestoreMode::Code),
        ("/rewind chosen both", RewindRestoreMode::Both),
    ] {
        let (mut app, mut commands) = fixture();
        app.sdk_inventory.rewind_targets_session_id = Some("control-session".into());
        app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
            uuid: "chosen".into(),
            first_text: "Chosen source".into(),
            input_text: "Chosen source".into(),
            index: 0,
            previous_assistant_uuid: Some("other-assistant".into()),
            resume_anchor_uuid: Some("other-anchor".into()),
        }];
        let cli = crate::Cli::try_parse_from([
            "claude-rs",
            "--model",
            "haiku",
            "--name",
            "Restored branch",
        ])
        .expect("launch settings");
        app.startup = crate::app::state::StartupState::from_cli(&cli);
        submit(&mut app, input);
        let BridgeCommand::Rewind {
            session_id,
            target_user_message_id,
            restore_mode,
            launch_settings,
        } = commands.try_recv().expect("rewind request").command
        else {
            panic!("Rewind")
        };
        assert_eq!(session_id, "control-session");
        assert_eq!(target_user_message_id, "chosen");
        assert_eq!(restore_mode, expected);
        assert_eq!(launch_settings.model.as_deref(), Some("haiku"));
        assert_eq!(launch_settings.name.as_deref(), Some("Restored branch"));
        block_second_submission(&mut app, &mut commands);
    }
}
