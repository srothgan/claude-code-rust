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
    for ch in "/new".chars() {
        key(&mut app, KeyCode::Char(ch));
    }

    enter(&mut app);

    assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::NewSession { .. }]));
    assert!(app.input.is_empty());
    assert_eq!(app.status, AppStatus::CommandPending);
    assert_eq!(app.turn.pending_command_label.as_deref(), Some("Starting new session..."));
}

#[test]
fn clear_with_an_empty_argument_menu_dispatches_and_resets_the_conversation() {
    let (mut app, mut rx) = app_with_bridge_connection();
    advertise(
        &mut app,
        vec![
            model::AvailableCommand::new("clear", "Clear conversation")
                .input_hint("[name]")
                .builtin(true),
        ],
    );
    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete("old conversation"))],
        None,
    ));
    draft(&mut app, "/clear ");
    assert!(app.slash.visible().is_none());
    key(&mut app, KeyCode::Tab);
    assert!(app.slash.visible().is_some_and(|slash| slash.candidates.is_empty()));
    let hint = crate::ui::input_rows::build_composer_hint_rows(&app)
        .into_iter()
        .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
        .collect::<String>();
    assert!(hint.contains("Arguments: [name]"));
    assert!(!hint.contains("Type a command name"));

    for conversation in ["new-conversation-1", "new-conversation-2"] {
        enter(&mut app);
        let dispatched = commands(&mut rx);
        let [BridgeCommand::Prompt { session_id, message_uuid, chunks, .. }] =
            dispatched.as_slice()
        else {
            panic!("expected exactly one /clear prompt: {dispatched:?}");
        };
        assert_eq!(session_id, "test-session");
        assert_eq!(chunks[0].kind, "text");
        assert_eq!(chunks[0].value.as_str().map(str::trim), Some("/clear"));
        handle_client_event(
            &mut app,
            session_update(model::SessionUpdate::ConversationReset {
                new_conversation_id: conversation.into(),
                trigger: Some("clear".into()),
                timestamp: None,
                user_message_uuid: Some(message_uuid.clone()),
            }),
        );
        assert_eq!(app.status, AppStatus::Ready);
        assert_eq!(app.session_runtime.conversation_id.as_deref(), Some(conversation));
        assert!(!canonical_messages_contain_text(&app, "old conversation"));
        assert_eq!(app.transcript.messages.len(), 1);
        assert_eq!(app.transcript.messages[0].role, MessageRole::Welcome);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
        assert!(app.pending_submit.is_none());
        draft(&mut app, "/clear");
    }
}

#[test]
fn clear_identity_replacement_rehydrates_commands_and_allows_reuse() {
    let (mut app, mut rx) = app_with_bridge_connection();
    let inventory = vec![
        model::AvailableCommand::new("clear", "Clear conversation")
            .input_hint("[name]")
            .aliases(vec!["reset".into(), "new".into()])
            .builtin(true),
        model::AvailableCommand::new("deploy", "Deploy").input_hint("<target>"),
    ];
    advertise(&mut app, inventory.clone());
    let mut session_id = "test-session".to_owned();
    for replacement in ["clear-session-1", "clear-session-2"] {
        draft(&mut app, "/clear");
        key(&mut app, KeyCode::Tab);
        assert!(app.slash.visible().is_none());
        assert_eq!(app.focus_owner(), FocusOwner::Input);
        enter(&mut app);
        let dispatched = commands(&mut rx);
        assert!(
            matches!(dispatched.as_slice(), [BridgeCommand::Prompt { session_id: sent_id, chunks, .. }]
            if sent_id == &session_id && chunks[0].value.as_str().map(str::trim) == Some("/clear"))
        );

        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: session_id.clone(),
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
        assert!(app.sdk_inventory.available_commands.is_empty());
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: replacement.into(),
                update: model::SessionUpdate::AvailableCommandsUpdate(
                    model::AvailableCommandsUpdate::new(inventory.clone())
                        .source("commands_changed")
                        .generation(3),
                ),
            },
        );
        assert_eq!(app.sdk_inventory.available_commands, inventory);
        assert_eq!(app.status, AppStatus::Ready);
        draft(&mut app, "/");
        for command in ["/clear", "/new", "/reset", "/new-session", "/deploy"] {
            assert!(
                app.slash
                    .visible()
                    .expect("command menu")
                    .candidates
                    .iter()
                    .any(|candidate| candidate.insert_value == command),
                "{command}"
            );
        }
        draft(&mut app, "/new");
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), "/new ");
        assert!(app.slash.visible().is_none());
        assert!(app.pending_submit.is_none());
        commands(&mut rx);
        session_id = replacement.into();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn effort_completion_dispatches_once_and_acknowledgement_allows_reuse() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut rx) = app_with_bridge_connection();
            app.session_runtime.current_model =
                Some(test_current_model("opus").supports_effort(true).supported_effort_levels(
                    vec![model::EffortLevel::High, model::EffortLevel::Max],
                ));
            draft(&mut app, "/effort");
            enter(&mut app);
            assert!(commands(&mut rx).is_empty());
            assert_eq!(app.status, AppStatus::Ready);
            assert!(app.slash.is_visible());

            for (typed, expected) in [("/effort hi", "high"), ("/effort max", "max")] {
                draft(&mut app, typed);
                enter(&mut app);
                tokio::task::yield_now().await;
                let dispatched = commands(&mut rx);
                assert!(
                    matches!(dispatched.as_slice(),
                        [BridgeCommand::SetEffort { session_id, effort }]
                            if session_id == "test-session" && effort.as_deref() == Some(expected)
                    ),
                    "{dispatched:?}"
                );
                assert_eq!(app.status, AppStatus::CommandPending);
                handle_client_event(
                    &mut app,
                    session_update(model::SessionUpdate::ConfigOptionUpdate(
                        model::ConfigOptionUpdate {
                            option_id: "effortLevel".into(),
                            value: serde_json::json!(expected),
                        },
                    )),
                );
                assert_eq!(app.status, AppStatus::Ready);
                assert!(app.turn.pending_command_label.is_none());
            }
        })
        .await;
}

#[test]
fn btw_name_waits_for_free_text_and_one_enter_sends_the_complete_question() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.status = AppStatus::Running;
    draft(&mut app, "/btw");
    enter(&mut app);
    assert!(commands(&mut rx).is_empty());
    assert_eq!(app.input.text(), "/btw ");

    let question = "Why  does /clear need Enter?\nKeep these spaces and this line.";
    app.input.set_text(&format!("/btw   {question}  "));
    enter(&mut app);

    let dispatched = commands(&mut rx);
    let [BridgeCommand::SideQuestion { btw_id, question: sent, .. }] = dispatched.as_slice() else {
        panic!("expected exactly one side question: {dispatched:?}");
    };
    assert_eq!(sent, question);
    assert_eq!(app.status, AppStatus::Running);
    assert!(app.pending_user_messages.is_empty());
    assert!(app.transcript.messages.is_empty());
    handle_client_event(
        &mut app,
        ClientEvent::BtwResult {
            session_id: "test-session".into(),
            btw_id: btw_id.clone(),
            question: sent.clone(),
            answer: "The autocomplete handler consumed it.".into(),
            metadata: crate::agent::wire::SideQuestionMetadata {
                synthetic: false,
                refusal_fallback: None,
            },
        },
    );
    assert!(app.btw.get(btw_id).is_none());
    assert!(matches!(app.transcript.messages[0].blocks.as_slice(),
        [MessageBlock::BtwExchange(exchange)] if exchange.question == question
    ));
}

#[test]
fn invalid_arguments_reach_usage_feedback_without_dispatch_or_input_capture() {
    for input in [
        "/effort impossible",
        "/docs invalid-topic",
        "/btw ",
        "/new-session extra",
        "/effort high extra",
        "/resume one two",
    ] {
        let (mut app, mut rx) = app_with_bridge_connection();
        draft(&mut app, input);
        enter(&mut app);

        assert!(commands(&mut rx).is_empty(), "{input}");
        assert!(app.input.is_empty(), "{input}");
        assert!(app.pending_submit.is_none(), "{input}");
        let command = input.split_whitespace().next().expect("command");
        let usage = crate::app::slash::command_spec(command).expect("app command").usage;
        assert!(canonical_messages_contain_text(&app, usage), "{input}");
    }
}

#[test]
fn optional_resume_opens_the_picker_without_selecting_a_suggested_session() {
    for input in ["/resume", "/resume "] {
        let (mut app, mut rx) = app_with_bridge_connection();
        app.recent_sessions = vec![RecentSessionInfo {
            session_id: "suggested-session".into(),
            summary: "Suggested session".into(),
            last_modified_ms: 1,
            file_size_bytes: 1,
            cwd: None,
            git_branch: None,
            custom_title: None,
            first_prompt: None,
        }];
        draft(&mut app, input);
        enter(&mut app);

        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
        assert!(commands(&mut rx).is_empty());
    }

    let (mut app, mut rx) = app_with_bridge_connection();
    draft(&mut app, "/resume manually-supplied-id");
    enter(&mut app);
    assert!(matches!(commands(&mut rx).as_slice(),
        [BridgeCommand::ResumeSession { session_id, .. }] if session_id == "manually-supplied-id"
    ));
}

#[test]
fn rewind_waits_for_both_arguments_before_dispatching_the_selected_restore_mode() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.sdk_inventory.rewind_targets_session_id = app.session_runtime.session_id.clone();
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".into(),
        first_text: "Original question".into(),
        input_text: "Original question".into(),
        index: 0,
        previous_assistant_uuid: None,
        resume_anchor_uuid: None,
    }];
    draft(&mut app, "/rewind us");
    enter(&mut app);
    assert_eq!(app.input.text(), "/rewind user-1 ");
    assert!(commands(&mut rx).is_empty());
    assert!(app.slash.is_visible());

    key(&mut app, KeyCode::Down);
    enter(&mut app);
    assert!(matches!(commands(&mut rx).as_slice(),
        [BridgeCommand::Rewind { target_user_message_id, restore_mode, .. }]
            if target_user_message_id == "user-1"
                && *restore_mode == crate::agent::types::RewindRestoreMode::Conversation
    ));
}

#[test]
fn active_turn_rules_still_block_app_and_session_commands_after_completion() {
    for input in ["/new-session", "/effort high", "/clear"] {
        let (mut app, mut rx) = app_with_bridge_connection();
        advertise(&mut app, vec![model::AvailableCommand::new("clear", "Clear")]);
        app.status = AppStatus::Running;
        draft(&mut app, input);
        enter(&mut app);

        assert!(commands(&mut rx).is_empty(), "{input}");
        assert_eq!(app.input.text().trim(), input);
        assert_eq!(app.status, AppStatus::Running);
        assert!(!app.turn.cancel_requested);
    }
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

    handle_terminal_event(&mut app, Event::Paste("pasted text".into()));
    crate::app::finalize_deferred_submit(&mut app);

    assert!(commands(&mut rx).is_empty());
    assert!(app.pending_submit.is_none());
    assert_eq!(app.paste.pending_text, "pasted text");
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
    for code in
        [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('x'), KeyCode::Tab, KeyCode::F(4)]
    {
        let (mut app, mut rx) = app_with_bridge_connection();
        draft(&mut app, "/help");
        key(&mut app, KeyCode::Enter);
        assert!(app.pending_submit.is_some());
        key(&mut app, KeyCode::Left);
        key(&mut app, KeyCode::Left);
        assert!(app.slash.is_visible());
        remap_completion_keys(&mut app);

        key(&mut app, code);
        let edited = app.input.text();
        assert!(app.pending_submit.is_none(), "{code:?}");
        if code == KeyCode::F(4) {
            assert_eq!(app.slash.visible().expect("updated menu").query, "he");
        }
        finish_input_cycle(&mut app);

        assert!(commands(&mut rx).is_empty(), "{code:?}");
        assert_eq!(app.input.text(), edited, "{code:?}");
        assert!(app.transcript.messages.is_empty(), "old /help must not execute");
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

#[test]
fn pasted_argument_refreshes_suggestions_before_the_next_explicit_submit() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.session_runtime.current_model = Some(
        test_current_model("opus")
            .supports_effort(true)
            .supported_effort_levels(vec![model::EffortLevel::High, model::EffortLevel::Max]),
    );
    draft(&mut app, "/effort hi");
    assert!(!app.slash.visible().expect("menu").candidates.is_empty());
    handle_terminal_event(&mut app, Event::Paste("bogus".into()));
    key(&mut app, KeyCode::Enter);
    finish_input_cycle(&mut app);

    assert_eq!(app.input.text(), "/effort hibogus");
    assert!(commands(&mut rx).is_empty());
    let menu = app.slash.visible().expect("updated argument menu");
    assert_eq!(menu.query, "hibogus");
    assert!(menu.candidates.is_empty());

    enter(&mut app);
    assert!(commands(&mut rx).is_empty(), "stale high suggestion must not execute");
    assert!(app.input.is_empty());
    assert!(canonical_messages_contain_text(
        &app,
        crate::app::slash::command_spec("/effort").expect("command").usage
    ));
}

#[test]
fn exact_app_and_sdk_command_names_dispatch_despite_competing_substring_matches() {
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

        let dispatched = commands(&mut rx);
        if command == "help" {
            assert!(dispatched.is_empty(), "app help must not send a plugin prompt");
            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
            assert_eq!(app.config.active_tab, crate::app::ConfigTab::Help);
        } else {
            assert!(
                matches!(dispatched.as_slice(), [BridgeCommand::Prompt { chunks, .. }]
                if chunks[0].value.as_str().map(str::trim) == Some(format!("/{command}").as_str())),
                "{dispatched:?}"
            );
        }
    }
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
fn optional_sdk_command_completion_and_named_execution_preserve_exact_input() {
    for (prefix, command) in [("/clea", "/clear"), ("/rese", "/reset"), ("/new", "/new")] {
        for name in ["", "Investigation 🦀  follow-up"] {
            let (mut app, mut rx) = app_with_bridge_connection();
            advertise(
                &mut app,
                vec![
                    model::AvailableCommand::new("clear", "Clear")
                        .input_hint("[name]")
                        .aliases(vec!["reset".into(), "new".into()]),
                ],
            );
            type_text(&mut app, prefix);
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.input.text(), format!("{command} "));
            assert_eq!(app.focus_owner(), FocusOwner::Input);
            assert!(app.slash.visible().is_none());
            let hints = crate::ui::input_rows::build_composer_hint_rows(&app)
                .into_iter()
                .flat_map(|line| line.spans.into_iter().map(|span| span.content.into_owned()))
                .collect::<String>();
            assert!(!hints.contains("Arguments:"));
            assert!(commands(&mut rx).is_empty());
            if !name.is_empty() {
                type_text(&mut app, &format!(" {name}"));
                assert!(app.slash.visible().is_none());
            }
            enter(&mut app);
            let dispatched = commands(&mut rx);
            let [BridgeCommand::Prompt { chunks, .. }] = dispatched.as_slice() else {
                panic!("expected one SDK command: {dispatched:?}");
            };
            let expected =
                if name.is_empty() { format!("{command} ") } else { format!("{command}  {name}") };
            assert_eq!(chunks[0].value.as_str(), Some(expected.as_str()));
        }
    }
}

#[test]
fn optional_resume_completion_opens_picker_and_explicit_tab_can_resume_a_suggestion() {
    for explicit_id in [false, true] {
        let (mut app, mut rx) = app_with_bridge_connection();
        app.recent_sessions = vec![RecentSessionInfo {
            session_id: "recent-session".into(),
            summary: "Recent".into(),
            last_modified_ms: 1,
            file_size_bytes: 1,
            cwd: None,
            git_branch: None,
            custom_title: None,
            first_prompt: None,
        }];
        draft(&mut app, "/resu");
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), "/resume ");
        assert!(app.slash.visible().is_none());
        assert!(commands(&mut rx).is_empty());
        if explicit_id {
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.input.text(), "/resume recent-session ");
            assert!(app.pending_submit.is_none());
        }
        enter(&mut app);
        if explicit_id {
            assert!(
                matches!(commands(&mut rx).as_slice(), [BridgeCommand::ResumeSession { session_id, .. }] if session_id == "recent-session")
            );
        } else {
            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
            assert!(commands(&mut rx).is_empty());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn space_keeps_required_effort_assistance_and_dispatches_only_after_enter() {
    tokio::task::LocalSet::new().run_until(async {
        let (mut app, mut rx) = app_with_bridge_connection();
        app.session_runtime.current_model = Some(test_current_model("opus").supports_effort(true).supported_effort_levels(vec![model::EffortLevel::High, model::EffortLevel::Max]));
        type_text(&mut app, "/effort");
        key(&mut app, KeyCode::Char(' '));
        assert!(app.slash.is_visible());
        type_text(&mut app, " hi");
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), "/effort  high ");
        assert!(app.slash.visible().is_none());
        assert!(commands(&mut rx).is_empty());
        enter(&mut app);
        tokio::task::yield_now().await;
        assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::SetEffort { effort, .. }] if effort.as_deref() == Some("high")));
    }).await;
}

#[test]
fn missing_or_invalid_arguments_after_dismissal_still_reach_authoritative_validation() {
    for text in ["/effort ", "/effort impossible", "/btw ", "/resume first second"] {
        let (mut app, mut rx) = app_with_bridge_connection();
        draft(&mut app, text);
        if app.slash.is_visible() {
            key(&mut app, KeyCode::Esc);
        }
        enter(&mut app);
        assert!(commands(&mut rx).is_empty(), "{text}");
        let command = text.split_whitespace().next().expect("command");
        let usage = crate::app::slash::command_spec(command).expect("app command").usage;
        assert!(canonical_messages_contain_text(&app, usage), "{text}");
    }
}

#[test]
fn btw_keystrokes_preserve_spaces_and_line_breaks_without_argument_popup() {
    let (mut app, mut rx) = app_with_bridge_connection();
    app.status = AppStatus::Running;
    type_text(&mut app, "/btw");
    key(&mut app, KeyCode::Tab);
    type_text(&mut app, " Why  does /clear work?");
    handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));
    type_text(&mut app, "/effort hi");
    assert!(app.slash.visible().is_none());
    assert!(commands(&mut rx).is_empty());
    enter(&mut app);
    assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::SideQuestion { question, .. }]
        if question == "Why  does /clear work?\n/effort hi"));
    assert_eq!(app.status, AppStatus::Running);
}

#[test]
fn escape_prevents_rewind_fetch_until_tab_and_async_targets_do_not_reopen_the_menu() {
    let (mut app, mut rx) = app_with_bridge_connection();
    draft(&mut app, "/rewind");
    key(&mut app, KeyCode::Esc);
    type_text(&mut app, " ");
    assert!(app.slash.visible().is_none());
    assert!(commands(&mut rx).is_empty());
    key(&mut app, KeyCode::Tab);
    assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::GetRewindTargets { .. }]));
    assert!(app.slash.visible().expect("loading menu").candidates.is_empty());
    key(&mut app, KeyCode::Tab);
    assert!(commands(&mut rx).is_empty());
    key(&mut app, KeyCode::Esc);
    handle_client_event(
        &mut app,
        ClientEvent::RewindTargetsReceived {
            session_id: "test-session".into(),
            error: None,
            targets: vec![model::RewindTarget {
                uuid: "user-1".into(),
                first_text: "Original question".into(),
                input_text: "Original question".into(),
                index: 0,
                previous_assistant_uuid: None,
                resume_anchor_uuid: None,
            }],
        },
    );
    assert!(app.slash.visible().is_none());
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.input.text(), "/rewind user-1 ");
    assert!(app.slash.is_visible());
    enter(&mut app);
    assert!(
        matches!(commands(&mut rx).as_slice(), [BridgeCommand::Rewind { target_user_message_id, restore_mode, .. }]
        if target_user_message_id == "user-1" && *restore_mode == crate::agent::types::RewindRestoreMode::Both)
    );
}

#[test]
fn explicit_sdk_hint_dismissal_survives_refresh_and_new_drafts_reset_it() {
    let (mut app, mut rx) = app_with_bridge_connection();
    let inventory = vec![model::AvailableCommand::new("clear", "Clear").input_hint("[name]")];
    advertise(&mut app, inventory.clone());
    draft(&mut app, "/clear ");
    key(&mut app, KeyCode::Tab);
    assert!(app.slash.is_visible());
    key(&mut app, KeyCode::Esc);
    advertise(&mut app, inventory);
    type_text(&mut app, " investigation");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Right);
    assert!(app.slash.visible().is_none());
    assert!(commands(&mut rx).is_empty());
    handle_terminal_event(
        &mut app,
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    );
    assert!(app.input.is_empty());
    type_text(&mut app, "/clea");
    assert!(app.slash.is_visible());
    enter(&mut app);
    assert!(matches!(commands(&mut rx).as_slice(), [BridgeCommand::Prompt { chunks, .. }]
        if chunks[0].value.as_str().map(str::trim) == Some("/clear")));
}

#[test]
fn custom_space_and_tab_bindings_take_precedence_over_default_completion() {
    for (code, chord) in [(KeyCode::Char(' '), "space"), (KeyCode::Tab, "tab")] {
        let (mut app, mut rx) = app_with_bridge_connection();
        draft(&mut app, "/effo");
        app.keymap = ResolvedKeymap::from_bindings(vec![KeyBinding::new(
            KeyContext::AutocompleteSlash,
            chord.parse().expect("key chord"),
            KeyAction::App(AppAction::Redraw),
            KeyBindingSource::Config,
        )])
        .expect("keymap");
        key(&mut app, code);
        assert_eq!(app.input.text(), "/effo");
        assert!(app.slash.is_visible());
        assert!(app.pending_submit.is_none());
        assert!(commands(&mut rx).is_empty());
    }
}
