// SPDX-License-Identifier: Apache-2.0
//! Terminal input -> deferred submission -> bridge command -> client event flows.

use super::{
    app_with_bridge_connection, canonical_messages_contain_text, session_update, test_current_model,
};
use crate::agent::wire::BridgeCommand;
use crate::agent::{events::ClientEvent, model};
use crate::app::keymap::{
    AppAction, AutocompleteAction, InputAction, KeyAction, KeyBinding, KeyBindingSource,
    KeyContext, ResolvedKeymap,
};
use crate::app::{
    App, AppStatus, ChatMessage, FocusOwner, FullscreenView, MessageBlock, MessageRole,
    RecentSessionInfo, SurfaceMode, TextBlock, handle_client_event, handle_terminal_event,
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;

fn draft(app: &mut App, text: &str) {
    app.input.set_text(text);
    app.input.set_cursor(0, 0);
    key(app, KeyCode::End);
}

fn key(app: &mut App, code: KeyCode) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn enter(app: &mut App) {
    key(app, KeyCode::Enter);
    crate::app::finalize_deferred_submit(app);
}

fn advertise(app: &mut App, commands: Vec<model::AvailableCommand>) {
    handle_client_event(
        app,
        session_update(model::SessionUpdate::AvailableCommandsUpdate(
            model::AvailableCommandsUpdate::new(commands),
        )),
    );
}

fn commands(rx: &mut crate::agent::client::CommandReceiver) -> Vec<BridgeCommand> {
    let mut commands = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if !matches!(envelope.command, BridgeCommand::GetContextUsage { .. }) {
            commands.push(envelope.command);
        }
    }
    commands
}

#[test]
fn typed_new_prefix_sends_one_new_session_command_on_one_enter() {
    let (mut app, mut rx) = app_with_bridge_connection();
    type_text(&mut app, "/new");
    enter(&mut app);
    assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::NewSession { .. }]));
    assert!(app.input.is_empty());
    assert_eq!(app.status, AppStatus::CommandPending);
    finish_input_cycle(&mut app);
    assert!(commands(&mut rx).is_empty());
}

#[test]
fn clear_with_an_empty_argument_menu_dispatches_and_resets_the_conversation() {
    let (mut app, mut rx) = app_with_bridge_connection();
    advertise(
        &mut app,
        vec![
            model::AvailableCommand::new("clear", "Clear conversation")
                .input_hint("[audit-name]")
                .builtin(true),
        ],
    );
    for (old, replacement, input) in [
        ("old first question", "new-1", "/clear "),
        ("old second question", "new-2", "/clear  archive"),
    ] {
        app.push_message_tracked(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete(old))],
            None,
        ));
        draft(&mut app, input);
        key(&mut app, KeyCode::Tab);
        assert!(app.slash.visible().is_some_and(|m| m.candidates.is_empty()));
        assert!(hint_rows(&app).iter().any(|r| r.contains("[audit-name]")));
        enter(&mut app);
        let sent = commands(&mut rx);
        assert_eq!(sent.len(), 1);
        assert_prompt(&sent[0], "test-session", input);
        let BridgeCommand::Prompt { message_uuid, .. } = &sent[0] else { unreachable!() };
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ConversationReset {
                new_conversation_id: replacement.into(),
                trigger: Some("clear".into()),
                timestamp: None,
                user_message_uuid: Some(message_uuid.clone()),
            }),
        );
        assert_eq!(app.status, AppStatus::Ready);
        assert!(!canonical_messages_contain_text(&app, old));
        assert_eq!(
            app.session_runtime.session_id.as_ref().map(ToString::to_string).as_deref(),
            Some("test-session")
        );
        assert_eq!(app.session_runtime.conversation_id.as_deref(), Some(replacement));
        finish_input_cycle(&mut app);
        assert!(commands(&mut rx).is_empty());
    }
}

#[test]
fn clear_identity_replacement_rehydrates_commands_and_allows_reuse() {
    let (mut app, mut rx) = app_with_bridge_connection();
    advertise(
        &mut app,
        vec![
            model::AvailableCommand::new("clear", "Clear"),
            model::AvailableCommand::new("obsolete-audit", "Old only"),
        ],
    );
    let mut current = "test-session";
    for replacement in ["clear-session-1", "clear-session-2"] {
        draft(&mut app, "/clear");
        enter(&mut app);
        let sent = commands(&mut rx);
        assert_eq!(sent.len(), 1);
        assert_prompt(&sent[0], current, "/clear ");
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: current.into(),
                update: model::SessionUpdate::ConversationReset {
                    new_conversation_id: replacement.into(),
                    trigger: Some("clear".into()),
                    timestamp: None,
                    user_message_uuid: None,
                },
            },
        );
        let cwd = app.cwd_raw.clone();
        handle_client_event(
            &mut app,
            ClientEvent::SessionReplaced {
                session_id: model::SessionId::new(replacement),
                cwd,
                current_model: test_current_model("opus"),
                available_models: Vec::new(),
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates: Vec::new(),
                restored_input: None,
            },
        );
        assert_replacement_refreshes(&mut rx, replacement);
        draft(&mut app, "/obsolete");
        assert!(
            app.slash
                .visible()
                .is_none_or(|m| m.candidates.iter().all(|c| c.insert_value != "/obsolete-audit"))
        );
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: replacement.into(),
                update: model::SessionUpdate::AvailableCommandsUpdate(
                    model::AvailableCommandsUpdate::new(vec![
                        model::AvailableCommand::new("clear", "Clear"),
                        model::AvailableCommand::new("deploy-audit", "New only")
                            .aliases(vec!["ship-audit".into()]),
                    ]),
                ),
            },
        );
        draft(&mut app, "/");
        assert!(
            app.slash
                .visible()
                .unwrap()
                .candidates
                .iter()
                .all(|c| c.insert_value != "/obsolete-audit")
        );
        draft(&mut app, "/ship-aud");
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), "/ship-audit ");
        assert!(commands(&mut rx).is_empty());
        enter(&mut app);
        let sent = commands(&mut rx);
        assert_eq!(sent.len(), 1);
        assert_prompt(&sent[0], replacement, "/ship-audit ");
        handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: replacement.into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
        assert!(commands(&mut rx).is_empty());
        current = replacement;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn session_command_completion_dispatches_once_and_waits_for_matching_acknowledgement() {
    tokio::task::LocalSet::new().run_until(async {
        for kind in ["effort", "mode", "model"] {
            let (mut app, mut rx) = app_with_bridge_connection();
            seed_effort(&mut app);
            app.session_runtime.mode = Some(mode_state("code"));
            app.sdk_inventory.available_models = vec![model::AvailableModel::new("sonnet", "Sonnet"), model::AvailableModel::new("haiku", "Haiku")];
            draft(&mut app, &format!("/{kind}")); enter(&mut app);
            assert!(commands(&mut rx).is_empty()); assert!(app.slash.is_visible());
            let targets = match kind { "mode" => [("pl", "plan"), ("co", "code")], "model" => [("so", "sonnet"), ("ha", "haiku")], _ => [("hi", "high"), ("max", "max")] };
            for (prefix, target) in targets {
                let old_model=app.session_runtime.current_model.as_ref().unwrap().resolved_id.clone();
                let old_mode=app.session_runtime.mode.as_ref().unwrap().current_mode_id.clone();
                let old_effort=app.session_runtime.config_options["effortLevel"].clone();
                draft(&mut app, &format!("/{kind} {prefix}")); enter(&mut app);
                let sent=received(&mut rx).await;
                match kind {
                    "mode" => assert!(matches!(sent, BridgeCommand::SetMode { session_id, mode } if session_id=="test-session" && mode==target)),
                    "model" => assert!(matches!(sent, BridgeCommand::SetModel { session_id, model } if session_id=="test-session" && model==target)),
                    _ => assert!(matches!(sent, BridgeCommand::SetEffort { session_id, effort } if session_id=="test-session" && effort.as_deref()==Some(target))),
                }
                assert_eq!(app.session_runtime.current_model.as_ref().unwrap().resolved_id,old_model);
                assert_eq!(app.session_runtime.mode.as_ref().unwrap().current_mode_id,old_mode);
                assert_eq!(app.session_runtime.config_options["effortLevel"],old_effort);
                handle_client_event(&mut app,session_update(model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate { option_id:"unrelated-audit-option".into(),value:serde_json::json!(true) })));
                assert_eq!(app.status,AppStatus::CommandPending);
                let ack=match kind {
                    "mode" => model::SessionUpdate::ModeStateUpdate(mode_state(target)),
                    "model" => model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(test_current_model(target))),
                    _ => model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {option_id:"effortLevel".into(),value:serde_json::json!(target)}),
                };
                handle_client_event(&mut app,session_update(ack)); assert_eq!(app.status,AppStatus::Ready);
                match kind {
                    "mode" => assert_eq!(app.session_runtime.mode.as_ref().unwrap().current_mode_id,target),
                    "model" => assert_eq!(app.session_runtime.current_model.as_ref().unwrap().resolved_id,target),
                    _ => assert_eq!(app.session_runtime.config_options["effortLevel"],serde_json::json!(target)),
                }
                finish_input_cycle(&mut app); assert!(commands(&mut rx).is_empty());
            }
        }
        let (mut app,mut rx)=app_with_bridge_connection();seed_effort(&mut app);
        type_text(&mut app,"/effort");key(&mut app,KeyCode::Char(' '));assert!(app.slash.is_visible());
        type_text(&mut app," hi");key(&mut app,KeyCode::Tab);
        assert_eq!(app.input.text(),"/effort  high ");assert!(commands(&mut rx).is_empty());enter(&mut app);
        assert!(matches!(received(&mut rx).await,BridgeCommand::SetEffort {session_id,effort} if session_id=="test-session" && effort.as_deref()==Some("high")));
        handle_client_event(&mut app,session_update(model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {option_id:"effortLevel".into(),value:serde_json::json!("high")})));
        finish_input_cycle(&mut app);assert!(commands(&mut rx).is_empty());
    }).await;
}

#[test]
fn btw_name_waits_for_free_text_and_one_enter_sends_the_complete_question() {
    for typed in [false, true] {
        let (mut app, mut rx) = app_with_bridge_connection();
        app.push_message_tracked(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete("main question"))],
            None,
        ));
        handle_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("main received answer")),
            ))),
        );
        draft(&mut app, "/btw");
        enter(&mut app);
        assert_eq!(app.input.text(), "/btw ");
        assert!(commands(&mut rx).is_empty());
        let expected = if typed {
            type_text(&mut app, " Why  does /clear work?");
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            );
            type_text(&mut app, "/effort hi");
            "Why  does /clear work?\n/effort hi"
        } else {
            app.input.set_text("/btw   Why  now?\nKeep this line.  ");
            "Why  now?\nKeep this line."
        };
        assert!(app.slash.visible().is_none());
        assert!(commands(&mut rx).is_empty());
        enter(&mut app);
        let sent = commands(&mut rx);
        let [BridgeCommand::SideQuestion { session_id, btw_id, question }] = sent.as_slice() else {
            panic!("SideQuestion: {sent:?}")
        };
        assert_eq!(session_id, "test-session");
        assert_eq!(question, expected);
        assert!(app.pending_user_messages.is_empty());
        handle_client_event(
            &mut app,
            ClientEvent::BtwResult {
                session_id: session_id.clone(),
                btw_id: btw_id.clone(),
                question: question.clone(),
                answer: "audit side answer".into(),
                metadata: crate::agent::wire::SideQuestionMetadata {
                    synthetic: false,
                    refusal_fallback: None,
                },
            },
        );
        assert!(app.btw.get(btw_id).is_none());
        assert!(app.transcript.messages.iter().any(|m|m.blocks.iter().any(|b|matches!(b,MessageBlock::BtwExchange(e) if e.question==expected && e.answer=="audit side answer"))));
        assert!(canonical_messages_contain_text(&app, "main question"));
        assert!(canonical_messages_contain_text(&app, "main received answer"));
        assert_eq!(app.status, AppStatus::Running);
        assert!(commands(&mut rx).is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_arguments_reach_usage_feedback_without_dispatch_or_input_capture() {
    tokio::task::LocalSet::new().run_until(async {
        for (input, guidance, correction) in [
            ("/effort impossible", "/effort", "/effort high"), ("/effort high extra", "/effort", "/effort high"),
            ("/docs invalid-topic", "/docs", "/docs commands"), ("/btw ", "/btw", "/btw corrected question"),
            ("/new-session extra", "/new-session", "/new-session"), ("/resume one two", "/resume", "/resume"),
            ("/resume manually-supplied-id", "/resume", "/resume"), ("/effort ", "/effort", "/effort high"),
        ] {
            for dismissed in [false,true] {
                let (mut app,mut rx)=app_with_bridge_connection();seed_effort(&mut app);draft(&mut app,input);
                // Bare required names request assistance; explicit dismissal submits the missing argument.
                if dismissed || input=="/effort " {key(&mut app,KeyCode::Esc);}
                enter(&mut app);assert!(commands(&mut rx).is_empty(),"{input}");
                assert!(app.pending_submit.is_none());assert!(app.input.is_empty());
                assert!(app.transcript.messages.iter().any(|m|m.blocks.iter().any(|b|matches!(b,MessageBlock::Text(t) if t.text.contains(guidance) && t.text.contains("Usage")))) ,"{input}");
                let allowed = match guidance { "/effort" => "high", "/docs" => "commands", "/btw" => "question", other => other };
                assert!(feedback_contains(&app, allowed), "{input}: expected useful argument guidance");
                assert!(!app.slash.is_visible());
                assert!(hint_rows(&app).iter().all(|r| !r.contains("impossible")));
                draft(&mut app,correction);enter(&mut app);
                match guidance {
                    "/effort" => assert!(matches!(received(&mut rx).await,BridgeCommand::SetEffort {session_id,effort} if session_id=="test-session" && effort.as_deref()==Some("high"))),
                    "/new-session" => assert!(matches!(commands(&mut rx).as_slice(),[BridgeCommand::NewSession {..}])),
                    "/btw" => assert!(matches!(commands(&mut rx).as_slice(),[BridgeCommand::SideQuestion {session_id,question,..}] if session_id=="test-session" && question=="corrected question")),
                    "/resume" => {assert_eq!(app.surface_mode,SurfaceMode::Fullscreen(FullscreenView::SessionPicker));assert!(commands(&mut rx).is_empty());},
                    _ => {assert!(feedback_contains(&app,"/new-session"));assert!(commands(&mut rx).is_empty());},
                }
            }
        }
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn active_turn_rules_still_block_app_and_session_commands_after_completion() {
    tokio::task::LocalSet::new().run_until(async {
        for input in ["/new-session", "/effort high", "/clear"] {
            let (mut app,mut rx)=app_with_bridge_connection();seed_effort(&mut app);
            advertise(&mut app,vec![model::AvailableCommand::new("clear","Clear")]);
            handle_client_event(&mut app,session_update(model::SessionUpdate::AgentResponseStarted));
            handle_client_event(&mut app,session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new("received active turn"))))));
            draft(&mut app,input);enter(&mut app);
            assert!(commands(&mut rx).is_empty());assert_eq!(app.input.text(),format!("{input} "));
            assert_eq!(app.status,AppStatus::Running);assert!(!app.turn.cancel_requested);assert!(app.pending_user_messages.is_empty());
            handle_client_event(&mut app,super::turn_complete(None));
            assert!(canonical_messages_contain_text(&app,"received active turn"));
            assert!(commands(&mut rx).is_empty());assert_eq!(app.input.text(),format!("{input} "));
            enter(&mut app);
            let sent=received(&mut rx).await;
            match input {
                "/new-session"=>assert!(matches!(sent,BridgeCommand::NewSession {..})),
                "/effort high"=>assert!(matches!(sent,BridgeCommand::SetEffort {session_id,effort} if session_id=="test-session" && effort.as_deref()==Some("high"))),
                _=>assert_prompt(&sent,"test-session","/clear "),
            }
            finish_input_cycle(&mut app);assert!(commands(&mut rx).is_empty());
        }
    }).await;
}

#[test]
fn remapped_submit_action_and_unbound_enter_use_the_same_slash_workflow() {
    for bindings in [
        vec![KeyBinding::new(
            KeyContext::AutocompleteSlash,
            "ctrl-j".parse().expect("key"),
            KeyAction::App(AppAction::SubmitInput),
            KeyBindingSource::Config,
        )],
        Vec::new(),
    ] {
        let (mut app, mut rx) = app_with_bridge_connection();
        draft(&mut app, "/new-session");
        let is_remapped = !bindings.is_empty();
        app.keymap = ResolvedKeymap::from_bindings(bindings).expect("keymap");
        if is_remapped {
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            );
            crate::app::finalize_deferred_submit(&mut app);
        } else {
            enter(&mut app);
        }
        assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::NewSession { .. }]));
    }
}

#[test]
fn delayed_paste_still_cancels_the_deferred_slash_submit() {
    let (mut app, mut rx) = app_with_bridge_connection();
    draft(&mut app, "/new-session");
    key(&mut app, KeyCode::Enter);
    assert!(app.pending_submit.is_some());
    handle_terminal_event(&mut app, Event::Paste(" pasted text".into()));
    finish_input_cycle(&mut app);
    assert!(commands(&mut rx).is_empty());
    assert!(app.transcript.messages.is_empty());
    assert_eq!(app.input.text(), "/new-session  pasted text");
    assert_eq!(app.input.cursor(), (0, 25));
    draft(&mut app, "/new-session");
    enter(&mut app);
    assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::NewSession { .. }]));
}

fn finish_input_cycle(app: &mut App) {
    crate::app::finalize_pending_paste_event(app);
    crate::app::finalize_deferred_submit(app);
}

fn remap_completion_keys(app: &mut App) {
    let mut bindings = crate::app::keymap::default_bindings();
    for (spec, action) in [
        ("f2", KeyAction::App(AppAction::SubmitInput)),
        ("f3", KeyAction::Autocomplete(AutocompleteAction::Confirm)),
        ("f4", KeyAction::Input(InputAction::DeleteCharBefore)),
    ] {
        bindings.push(KeyBinding::new(
            KeyContext::AutocompleteSlash,
            spec.parse().expect("key"),
            action,
            KeyBindingSource::Config,
        ));
    }
    app.keymap = ResolvedKeymap::from_bindings(bindings).expect("keymap");
}

#[test]
fn queued_paste_blocks_autocomplete_edits_and_submission_until_the_drain_finishes() {
    for input in ["/help", "/clea", "/effort hi", "/clear "] {
        for code in [
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::Char('x'),
            KeyCode::F(2),
            KeyCode::F(3),
            KeyCode::F(4),
        ] {
            let (mut app, mut rx) = app_with_bridge_connection();
            app.session_runtime.current_model =
                Some(test_current_model("opus").supports_effort(true).supported_effort_levels(
                    vec![model::EffortLevel::High, model::EffortLevel::Max],
                ));
            advertise(
                &mut app,
                vec![model::AvailableCommand::new("clear", "Clear").input_hint("[name]")],
            );
            draft(&mut app, input);
            if input == "/clear " {
                key(&mut app, KeyCode::Tab);
            }
            assert!(app.slash.is_visible());
            remap_completion_keys(&mut app);

            // All events arrive before either finalizer, as in one terminal drain.
            handle_terminal_event(&mut app, Event::Paste(" pasted".into()));
            key(&mut app, code);
            assert!(app.pending_submit.is_none(), "{input}: {code:?}");
            assert_eq!(app.input.text(), input, "{input}: {code:?}");
            finish_input_cycle(&mut app);

            assert!(commands(&mut rx).is_empty(), "{input}: {code:?}");
            assert_eq!(app.input.text(), format!("{input} pasted"));
        }
    }
}

#[test]
fn editing_a_reopened_command_menu_cancels_the_previous_submission_in_the_same_drain() {
    for (code, expected, cursor) in [
        (KeyCode::Backspace, "/hep ", 3),
        (KeyCode::Delete, "/hel ", 4),
        (KeyCode::Char('x'), "/helxp ", 5),
        (KeyCode::Tab, "/help ", 6),
        (KeyCode::F(4), "/hep ", 3),
    ] {
        let (mut app, mut rx) = app_with_bridge_connection();
        draft(&mut app, "/help");
        key(&mut app, KeyCode::Enter);
        assert!(app.pending_submit.is_some());
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Left);
        remap_completion_keys(&mut app);
        key(&mut app, code);
        assert_eq!(app.input.text(), expected, "{code:?}");
        assert_eq!(app.input.cursor(), (0, cursor), "{code:?}");
        finish_input_cycle(&mut app);
        assert_eq!(app.input.text(), expected);
        assert_eq!(app.input.cursor(), (0, cursor));
        assert!(commands(&mut rx).is_empty());
        assert!(app.transcript.messages.is_empty());
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
    }
}

#[test]
fn autocomplete_paste_guard_keeps_the_clear_input_control_available() {
    let (mut app, mut rx) = app_with_bridge_connection();
    draft(&mut app, "/help");
    handle_terminal_event(&mut app, Event::Paste(" pasted".into()));
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    finish_input_cycle(&mut app);

    assert!(app.input.is_empty());
    assert!(!app.paste.has_pending_text());
    assert!(commands(&mut rx).is_empty());
    assert!(!app.shutdown_requested());
}

#[tokio::test(flavor = "current_thread")]
async fn pasted_argument_refreshes_suggestions_before_the_next_explicit_submit() {
    tokio::task::LocalSet::new().run_until(async {
        let (mut app,mut rx)=app_with_bridge_connection();seed_effort(&mut app);draft(&mut app,"/effort hi");
        assert!(app.slash.visible().unwrap().candidates.iter().any(|c|c.insert_value=="high"));
        handle_terminal_event(&mut app,Event::Paste("bogus".into()));key(&mut app,KeyCode::Enter);finish_input_cycle(&mut app);
        assert_eq!(app.input.text(),"/effort hibogus");assert!(commands(&mut rx).is_empty());assert!(app.slash.visible().unwrap().candidates.is_empty());
        enter(&mut app);assert!(commands(&mut rx).is_empty());assert!(app.input.is_empty());
        assert!(feedback_contains(&app,"/effort"));assert!(feedback_contains(&app,"high"));
        draft(&mut app,"/effort max");enter(&mut app);
        assert!(matches!(received(&mut rx).await,BridgeCommand::SetEffort {session_id,effort} if session_id=="test-session" && effort.as_deref()==Some("max")));
    }).await;
}

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        // Model ordinary keystrokes independently of machine-speed paste
        // detection; paste suppression is exercised separately above.
        app.paste.burst = crate::app::paste_burst::PasteBurstDetector::new();
        key(app, KeyCode::Char(ch));
    }
}

#[test]
fn sdk_completion_preserves_exact_payloads_and_app_command_ownership() {
    for (prefix, command) in [("/clea", "/clear"), ("/rese", "/reset"), ("/new", "/new")] {
        for name in ["", "Investigation \u{1f980}  follow-up"] {
            let (mut app, mut rx) = app_with_bridge_connection();
            advertise(
                &mut app,
                vec![
                    model::AvailableCommand::new("clear", "Clear")
                        .input_hint("[audit-name]")
                        .aliases(vec!["reset".into(), "new".into()]),
                ],
            );
            type_text(&mut app, prefix);
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.input.text(), format!("{command} "));
            assert_eq!(app.focus_owner(), FocusOwner::Input);
            assert!(!app.slash.is_visible());
            assert!(hint_rows(&app).iter().all(|r| !r.contains("[audit-name]")));
            assert!(commands(&mut rx).is_empty());
            if !name.is_empty() {
                type_text(&mut app, &format!(" {name}"));
                assert!(!app.slash.is_visible());
            }
            enter(&mut app);
            let sent = commands(&mut rx);
            assert_eq!(sent.len(), 1);
            let expected =
                if name.is_empty() { format!("{command} ") } else { format!("{command}  {name}") };
            assert_prompt(&sent[0], "test-session", &expected);
        }
    }
    for command in ["help", "clear", "new", "reset"] {
        let (mut app, mut rx) = app_with_bridge_connection();
        advertise(
            &mut app,
            vec![
                model::AvailableCommand::new("clear", "Clear")
                    .aliases(vec!["new".into(), "reset".into()]),
                model::AvailableCommand::new(format!("a-{command}"), "Competing plugin"),
            ],
        );
        draft(&mut app, &format!("/{command}"));
        enter(&mut app);
        let sent = commands(&mut rx);
        if command == "help" {
            assert!(sent.is_empty());
            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
            assert_eq!(app.config.active_tab, crate::app::ConfigTab::Help);
        } else {
            assert_eq!(sent.len(), 1);
            assert_prompt(&sent[0], "test-session", &format!("/{command} "));
        }
    }
}

#[test]
fn resume_command_completion_never_inserts_a_session_id() {
    for input in ["/resume", "/resume ", "/resu"] {
        let (mut app, mut rx) = app_with_bridge_connection();
        app.recent_sessions = vec![recent("suggested-session", "Suggested")];
        draft(&mut app, input);
        if input == "/resu" {
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.input.text(), "/resume ");
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.input.text(), "/resume ");
        }
        assert!(commands(&mut rx).is_empty());
        enter(&mut app);
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
        assert!(commands(&mut rx).is_empty());
    }
}

#[test]
fn escape_prevents_rewind_fetch_until_tab_and_async_targets_do_not_reopen_the_menu() {
    for cached in [false, true] {
        let (mut app, mut rx) = app_with_bridge_connection();
        let targets = vec![
            model::RewindTarget {
                uuid: "user-1".into(),
                first_text: "Original question".into(),
                input_text: "Original question".into(),
                index: 0,
                previous_assistant_uuid: None,
                resume_anchor_uuid: None,
            },
            model::RewindTarget {
                uuid: "other-user".into(),
                first_text: "Different question".into(),
                input_text: "Different question".into(),
                index: 1,
                previous_assistant_uuid: None,
                resume_anchor_uuid: None,
            },
        ];
        if cached {
            app.sdk_inventory.rewind_targets_session_id = app.session_runtime.session_id.clone();
            app.sdk_inventory.rewind_targets = targets;
            draft(&mut app, "/rewind us");
            enter(&mut app);
        } else {
            draft(&mut app, "/rewind");
            key(&mut app, KeyCode::Esc);
            type_text(&mut app, " ");
            assert!(!app.slash.is_visible());
            assert!(commands(&mut rx).is_empty());
            key(&mut app, KeyCode::Tab);
            assert!(
                matches!(commands(&mut rx).as_slice(),[BridgeCommand::GetRewindTargets {session_id}] if session_id=="test-session")
            );
            key(&mut app, KeyCode::Tab);
            assert!(commands(&mut rx).is_empty());
            key(&mut app, KeyCode::Esc);
            handle_client_event(
                &mut app,
                ClientEvent::RewindTargetsReceived {
                    session_id: "test-session".into(),
                    error: None,
                    targets,
                },
            );
            assert!(!app.slash.is_visible());
            // Narrow the still-dismissed query before explicit completion, independently of target order.
            type_text(&mut app, "us");
            assert!(!app.slash.is_visible());
            assert!(commands(&mut rx).is_empty());
            key(&mut app, KeyCode::Tab);
        }
        assert_eq!(app.input.text(), "/rewind user-1 ");
        assert!(app.slash.is_visible());
        assert!(commands(&mut rx).is_empty());
        if cached {
            select_candidate(&mut app, "conversation");
        }
        enter(&mut app);
        let sent = commands(&mut rx);
        let [BridgeCommand::Rewind { session_id, target_user_message_id, restore_mode, .. }] =
            sent.as_slice()
        else {
            panic!("Rewind: {sent:?}")
        };
        assert_eq!(session_id, "test-session");
        assert_eq!(target_user_message_id, "user-1");
        assert_eq!(
            *restore_mode,
            if cached {
                crate::agent::types::RewindRestoreMode::Conversation
            } else {
                crate::agent::types::RewindRestoreMode::Both
            }
        );
    }
}

#[test]
fn explicit_sdk_hint_dismissal_survives_refresh_and_new_drafts_reset_it() {
    let (mut app, mut rx) = app_with_bridge_connection();
    let inventory = vec![model::AvailableCommand::new("clear", "Clear").input_hint("[audit-name]")];
    advertise(&mut app, inventory.clone());
    draft(&mut app, "/clear ");
    key(&mut app, KeyCode::Tab);
    assert!(hint_rows(&app).iter().any(|r| r.contains("[audit-name]")));
    key(&mut app, KeyCode::Esc);
    advertise(&mut app, inventory);
    type_text(&mut app, " investigation");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Right);
    assert!(!app.slash.is_visible());
    assert!(hint_rows(&app).iter().all(|r| !r.contains("[audit-name]")));
    assert!(commands(&mut rx).is_empty());
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    assert!(app.input.is_empty());
    type_text(&mut app, "/clea");
    assert!(app.slash.is_visible());
    enter(&mut app);
    let sent = commands(&mut rx);
    assert_eq!(sent.len(), 1);
    assert_prompt(&sent[0], "test-session", "/clear ");
}

#[test]
fn custom_space_and_tab_bindings_take_precedence_over_default_completion() {
    for (code, chord) in [(KeyCode::Char(' '), "space"), (KeyCode::Tab, "tab")] {
        let (mut app, mut rx) = app_with_bridge_connection();
        draft(&mut app, "/effo");
        app.keymap = ResolvedKeymap::from_bindings(vec![KeyBinding::new(
            KeyContext::AutocompleteSlash,
            chord.parse().unwrap(),
            KeyAction::Input(InputAction::DeleteCharBefore),
            KeyBindingSource::Config,
        )])
        .unwrap();
        key(&mut app, code);
        finish_input_cycle(&mut app);
        assert_eq!(app.input.text(), "/eff");
        assert_eq!(app.input.cursor(), (0, 4));
        assert!(commands(&mut rx).is_empty());
    }
}

#[test]
fn resume_window_searches_names_ids_and_branches_and_dispatches_only_the_selected_session() {
    for query in ["FEATURE", "MATCH-", "LEASE/"] {
        let (mut app, mut rx) = app_with_bridge_connection();
        app.recent_sessions = [
            ("hidden", "Other conversation", None),
            ("empty-branch", "Another conversation", Some("")),
            ("match-a", "Feature alpha", Some("release/alpha")),
            ("match-b", "Feature beta", Some("Release/beta")),
        ]
        .into_iter()
        .map(|(id, title, branch)| RecentSessionInfo {
            session_id: id.into(),
            summary: title.into(),
            last_modified_ms: 0,
            file_size_bytes: 0,
            cwd: None,
            git_branch: branch.map(str::to_owned),
            custom_title: Some(title.into()),
            first_prompt: None,
        })
        .collect();
        draft(&mut app, "/resume");
        enter(&mut app);
        type_text(&mut app, query);
        key(&mut app, KeyCode::Down);
        assert!(commands(&mut rx).is_empty(), "{query}");
        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
        enter(&mut app);
        assert!(
            matches!(commands(&mut rx).as_slice(),
            [BridgeCommand::ResumeSession { session_id, .. }] if session_id == "match-b"),
            "{query}"
        );
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
    }
}

#[test]
fn cancelling_resume_search_preserves_the_current_session_and_draft() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.recent_sessions = vec![recent("match-a", "Audit matching"), recent("unrelated", "Other")];
    app.input.set_text("unfinished \u{754c}\nsecond line");
    app.input.set_cursor(1, 3);
    crate::app::session_picker::open(&mut app);
    assert_eq!(crate::app::session_picker::picker_session_count(&app), 2);
    handle_terminal_event(&mut app, Event::Paste("Audit matching".into()));
    assert_eq!(app.session_picker.query, "Audit matching");
    assert_eq!(
        crate::app::session_picker::picker_sessions(&app)
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>(),
        ["match-a"]
    );
    key(&mut app, KeyCode::Esc);
    finish_input_cycle(&mut app);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    assert_eq!(app.session_runtime.session_id.as_ref().unwrap().to_string(), "test-session");
    assert_eq!(app.input.text(), "unfinished \u{754c}\nsecond line");
    assert_eq!(app.input.cursor(), (1, 3));
    assert!(commands(&mut rx).is_empty());
    enter(&mut app);
    let sent = commands(&mut rx);
    assert_eq!(sent.len(), 1);
    assert_prompt(&sent[0], "test-session", "unfinished \u{754c}\nsecond line");
}

fn assert_prompt(command: &BridgeCommand, session: &str, text: &str) {
    let BridgeCommand::Prompt { session_id, chunks, .. } = command else {
        panic!("expected Prompt: {command:?}")
    };
    assert_eq!(session_id, session);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].kind, "text");
    assert_eq!(chunks[0].value, serde_json::json!(text));
}

async fn received(rx: &mut crate::agent::client::CommandReceiver) -> BridgeCommand {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let command = rx.recv_envelope().await.expect("command channel").command;
            if let BridgeCommand::GetContextUsage { session_id } = &command {
                assert_eq!(session_id, "test-session");
            } else {
                return command;
            }
        }
    })
    .await
    .expect("command delivered")
}

fn select_candidate(app: &mut App, value: &str) {
    let limit = app.slash.visible().expect("menu").candidates.len();
    for _ in 0..limit {
        let menu = app.slash.visible().expect("menu");
        if menu.candidates[menu.dialog.selected].insert_value == value {
            return;
        }
        key(app, KeyCode::Down);
    }
    panic!("candidate {value} not selectable");
}

fn hint_rows(app: &App) -> Vec<String> {
    crate::ui::input_rows::build_composer_hint_rows(app)
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn mode_state(current: &str) -> crate::app::ModeState {
    crate::app::ModeState {
        current_mode_id: current.into(),
        current_mode_name: current.into(),
        available_modes: ["code", "plan"]
            .into_iter()
            .map(|id| crate::app::ModeInfo { id: id.into(), name: id.into() })
            .collect(),
    }
}

fn seed_effort(app: &mut App) {
    app.session_runtime.current_model = Some(
        test_current_model("opus")
            .supports_effort(true)
            .supported_effort_levels(vec![model::EffortLevel::High, model::EffortLevel::Max]),
    );
    app.session_runtime.config_options.insert("effortLevel".into(), serde_json::json!("low"));
}

fn recent(id: &str, title: &str) -> RecentSessionInfo {
    RecentSessionInfo {
        session_id: id.into(),
        summary: title.into(),
        last_modified_ms: 1,
        file_size_bytes: 1,
        cwd: None,
        git_branch: None,
        custom_title: None,
        first_prompt: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_open_effort_menu_follows_current_session_capabilities_without_input_edits() {
    tokio::task::LocalSet::new().run_until(async {
        for supported in [true,false] {
            let (mut app,mut rx)=app_with_bridge_connection();seed_effort(&mut app);draft(&mut app,"/effort ");select_candidate(&mut app,"max");
            let old_rows=hint_rows(&app);assert!(old_rows.iter().any(|r|r.contains("> max")));
            let model=test_current_model("haiku").supports_effort(supported).supported_effort_levels(if supported {vec![model::EffortLevel::Low,model::EffortLevel::Medium]}else{vec![]});
            handle_client_event(&mut app,ClientEvent::SessionUpdate {session_id:"other-session".into(),update:model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(model.clone()))});
            assert_eq!(hint_rows(&app),old_rows);assert!(commands(&mut rx).is_empty());
            handle_client_event(&mut app,session_update(model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(model))));
            assert_eq!(app.input.text(),"/effort ");assert_eq!(app.input.cursor(),(0,8));assert!(commands(&mut rx).is_empty());
            let rows=hint_rows(&app);
            assert!(rows.iter().all(|r|!r.contains("high") && !r.contains("max")),"stale menu: {rows:?}");
            assert!(rows.iter().any(|r|r.contains("reset")));
            if supported { assert!(rows.iter().any(|r|r.contains("low")));assert!(rows.iter().any(|r|r.contains("medium"))); }
            else {assert!(rows.iter().all(|r|!r.contains("low") && !r.contains("medium")));}
            let selected=if supported {"low"}else{"reset"};select_candidate(&mut app,selected);
            assert!(hint_rows(&app).iter().any(|r|r.contains(&format!("> {selected}"))));
            enter(&mut app);
            assert!(matches!(received(&mut rx).await,BridgeCommand::SetEffort {session_id,effort} if session_id=="test-session" && effort.as_deref()==if supported{Some("low")}else{None}));
        }
    }).await;
}

fn feedback_contains(app: &App, expected: &str) -> bool {
    app.transcript.messages.iter().filter(|m| matches!(m.role, MessageRole::System(_))).any(|m| {
        m.blocks.iter().any(|b| match b {
            MessageBlock::Text(t) => t.text.contains(expected),
            MessageBlock::Notice(n) => n.text.text.contains(expected),
            _ => false,
        })
    })
}

fn assert_replacement_refreshes(rx: &mut crate::agent::client::CommandReceiver, replacement: &str) {
    while let Ok(envelope) = rx.try_recv() {
        match envelope.command {
            BridgeCommand::GetContextUsage { session_id }
            | BridgeCommand::GetStatusSnapshot { session_id }
            | BridgeCommand::GetMcpSnapshot { session_id }
            | BridgeCommand::InspectSettings { session_id, .. } => {
                assert_eq!(session_id, replacement);
            }
            other => panic!("unexpected replacement action: {other:?}"),
        }
    }
}
