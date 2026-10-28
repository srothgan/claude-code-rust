// SPDX-License-Identifier: Apache-2.0
//! Public terminal-event integration tests. These check completion, focus, and
//! the exact draft handed to the event loop, without executing auth commands.

use claude_code_rust::agent::{events::ClientEvent, model};
use claude_code_rust::app::{
    App, AutocompleteKind, FocusOwner, ModeInfo, ModeState, handle_client_event,
    handle_terminal_event,
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

const NO_ARGUMENT_COMMANDS: &[&str] = &[
    "/cancel",
    "/compact",
    "/config",
    "/fast",
    "/help",
    "/mcp",
    "/plugins",
    "/status",
    "/usage",
    "/login",
    "/logout",
    "/new-session",
    "/resume",
];

const REQUIRED_ARGUMENT_COMMANDS: &[(&str, &str)] = &[
    ("/btw", "Why does this work?"),
    ("/docs", "commands"),
    ("/agent", "reset"),
    ("/effort", "high"),
    ("/thinking", "off"),
    ("/ultracode", "status"),
    ("/mode", "plan"),
    ("/model", "opus"),
    ("/rewind", "user-1 conversation"),
];

fn app() -> App {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(model::SessionId::new("test-session"));
    app.session_runtime.current_model = Some(
        model::CurrentModel::new("opus", "Opus", "Opus")
            .supports_effort(true)
            .supported_effort_levels(model::EffortLevel::ALL.to_vec()),
    );
    app.session_runtime.mode = Some(ModeState {
        current_mode_id: "plan".into(),
        current_mode_name: "Plan".into(),
        available_modes: vec![ModeInfo { id: "plan".into(), name: "Plan".into() }],
    });
    app.sdk_inventory.available_models = vec![model::AvailableModel::new("opus", "Opus")];
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "test-session".into(),
            update: model::SessionUpdate::AvailableCommandsUpdate(
                model::AvailableCommandsUpdate::new(vec![
                    model::AvailableCommand::new("clear", "Clear conversation")
                        .input_hint("[name]")
                        .aliases(vec!["reset".into(), "new".into()])
                        .builtin(true),
                    model::AvailableCommand::new("ping", "No arguments"),
                    model::AvailableCommand::new("deploy", "Deploy")
                        .input_hint("<target>")
                        .aliases(vec!["ship".into()]),
                ]),
            ),
        },
    );
    app
}

fn key(app: &mut App, code: KeyCode) {
    handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn draft(app: &mut App, text: &str) {
    app.input.set_text(text);
    app.input.set_cursor(0, 0);
    // Moving the cursor through the public terminal boundary activates the real
    // autocomplete detection, rather than fabricating a SlashState.
    key(app, KeyCode::End);
}

#[allow(clippy::expect_used)]
fn deferred_draft(app: &App) -> String {
    app.pending_submit.as_ref().expect("one Enter must arm submission").lines.join("\n")
}

#[test]
fn default_exact_matches_and_deliberate_selection_preserve_handoff() {
    for command in NO_ARGUMENT_COMMANDS
        .iter()
        .copied()
        .chain(REQUIRED_ARGUMENT_COMMANDS.iter().map(|(name, _)| *name))
        .chain(["/clear", "/new", "/reset", "/ping", "/deploy", "/ship"])
    {
        for code in [KeyCode::Tab, KeyCode::Enter] {
            let mut app = app();
            let name = command.trim_start_matches('/');
            app.sdk_inventory.available_commands.extend([
                model::AvailableCommand::new(format!("a-{name}"), "Competing substring"),
                model::AvailableCommand::new(format!("{name}-extra"), "Competing prefix"),
            ]);
            draft(&mut app, command);
            assert_selected(&app, command);
            key(&mut app, code);
            assert_eq!(app.input.text(), format!("{command} "));
            let required = REQUIRED_ARGUMENT_COMMANDS.iter().any(|(name, _)| *name == command);
            if code == KeyCode::Enter && !required {
                assert_eq!(deferred_draft(&app), format!("{command} "));
            } else {
                assert!(app.pending_submit.is_none());
            }
        }
    }
    for code in [KeyCode::Tab, KeyCode::Enter] {
        let mut app = app();
        app.sdk_inventory
            .available_commands
            .push(model::AvailableCommand::new("a-help", "Competing plugin"));
        draft(&mut app, "/help");
        select(&mut app, "/a-help");
        assert_selected(&app, "/a-help");
        key(&mut app, code);
        assert_eq!(app.input.text(), "/a-help ");
        if code == KeyCode::Enter {
            assert_eq!(deferred_draft(&app), "/a-help ");
        } else {
            assert!(app.pending_submit.is_none());
        }
    }
    let mut argument = app();
    draft(&mut argument, "/effort high");
    select(&mut argument, "xhigh");
    assert_selected(&argument, "xhigh");
    key(&mut argument, KeyCode::Enter);
    assert_eq!(deferred_draft(&argument), "/effort xhigh ");
    let mut bare = app();
    draft(&mut bare, "/");
    select(&mut bare, "/docs");
    assert_selected(&bare, "/docs");
    key(&mut bare, KeyCode::Enter);
    assert_eq!(bare.input.text(), "/docs ");
    assert!(bare.pending_submit.is_none());
    assert!(
        bare.slash
            .visible()
            .expect("topic assistance")
            .candidates
            .iter()
            .any(|c| c.insert_value == "commands")
    );
}

#[test]
fn enter_hands_off_only_complete_commands_with_exact_source() {
    for command in NO_ARGUMENT_COMMANDS {
        let mut app = app();
        draft(&mut app, command);
        key(&mut app, KeyCode::Enter);
        assert_eq!(deferred_draft(&app), format!("{command} "));
        assert_eq!(app.focus_owner(), FocusOwner::Input);
        assert_eq!(app.active_autocomplete_kind(), None);
    }
    for (command, _) in REQUIRED_ARGUMENT_COMMANDS {
        let mut app = app();
        draft(&mut app, command);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.input.text(), format!("{command} "));
        assert!(app.pending_submit.is_none());
        if *command == "/btw" {
            assert_eq!(app.focus_owner(), FocusOwner::Input);
        } else {
            assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash));
        }
    }
    for (input, expected) in [
        ("/btw Why  now?", "/btw Why  now?"),
        ("/docs commands", "/docs commands "),
        ("/agent reset", "/agent reset "),
        ("/effort high", "/effort high "),
        ("/thinking off", "/thinking off "),
        ("/ultracode status", "/ultracode status "),
        ("/mode plan", "/mode plan "),
        ("/model opus", "/model opus "),
        ("/rewind user-1 conversation", "/rewind user-1 conversation "),
        ("/clear", "/clear "),
        ("/clear ", "/clear "),
        ("/clea", "/clear "),
        ("/new", "/new "),
        ("/rese", "/reset "),
        ("/resu", "/resume "),
        ("/ping", "/ping "),
        ("/deploy", "/deploy "),
        ("/ship", "/ship "),
        ("/clear  investigation \u{1f980}", "/clear  investigation \u{1f980}"),
        ("/deploy staging", "/deploy staging"),
        ("/ship staging", "/ship staging"),
    ] {
        let mut app = app();
        draft(&mut app, input);
        key(&mut app, KeyCode::Enter);
        assert_eq!(deferred_draft(&app), expected, "{input}");
        assert_eq!(app.active_autocomplete_kind(), None);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }
}

#[test]
fn tab_completes_usable_drafts_without_handing_off_commands() {
    for command in NO_ARGUMENT_COMMANDS.iter().copied().chain(["/ping"]) {
        let mut app = app();
        draft(&mut app, command);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), format!("{command} "));
        assert!(app.pending_submit.is_none());
        assert_eq!(app.active_autocomplete_kind(), None);
        assert_eq!(app.focus_owner(), FocusOwner::Input);
        key(&mut app, KeyCode::End);
        assert_eq!(app.active_autocomplete_kind(), None);
    }
    for (input, expected, menu) in [
        ("/effort", "/effort ", true),
        ("/btw", "/btw ", false),
        ("/resu", "/resume ", false),
        ("/clea", "/clear ", false),
        ("/rese", "/reset ", false),
        ("/new", "/new ", false),
        ("/deploy", "/deploy ", false),
        ("/ship", "/ship ", false),
        ("/effort hi", "/effort high ", false),
        ("/deploy staging", "/deploy staging", true),
    ] {
        let mut app = app();
        draft(&mut app, input);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), expected, "{input}");
        assert!(app.pending_submit.is_none());
        assert_eq!(app.active_autocomplete_kind(), menu.then_some(AutocompleteKind::Slash));
        assert_eq!(app.focus_owner(), if menu { FocusOwner::Mention } else { FocusOwner::Input });
        if input == "/effort" {
            select(&mut app, "high");
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.input.text(), "/effort high ");
            assert!(app.pending_submit.is_none());
        } else if !menu {
            key(&mut app, KeyCode::End);
            assert_eq!(app.active_autocomplete_kind(), None);
        }
    }
}

#[test]
fn large_command_inventories_remain_browsable_without_a_hidden_cutoff() {
    for target in ["/aaa-74", "/new-session"] {
        let mut app = app();
        let mut inventory: Vec<_> = (0..75)
            .map(|i| model::AvailableCommand::new(format!("aaa-{i:02}"), "Plugin command"))
            .collect();
        inventory.push(model::AvailableCommand::new("clear", "Clear conversation"));
        advertise(&mut app, inventory);
        draft(&mut app, "/");
        for name in ["/aaa-00", "/aaa-37", "/aaa-74", "/clear", "/new-session", "/rewind", "/usage"]
        {
            assert!(
                app.slash.visible().unwrap().candidates.iter().any(|c| c.insert_value == name),
                "{name}"
            );
        }
        select(&mut app, target);
        assert_selected(&app, target);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), format!("{target} "));
        assert!(app.pending_submit.is_none());
        draft(&mut app, "/aaa-");
        let names = app
            .slash
            .visible()
            .unwrap()
            .candidates
            .iter()
            .map(|c| c.insert_value.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let expected =
            (0..75).map(|i| format!("/aaa-{i:02}")).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names, expected.iter().map(String::as_str).collect());
        assert!(!names.contains("/clear"));
        assert!(!names.contains("/new-session"));
    }
}

#[test]
fn no_match_completion_hands_off_literal_values_and_invalid_suffixes() {
    for (matching, literal) in [
        ("/effort hi", "/effort impossible"),
        ("/mode pl", "/mode manually-supplied-mode"),
        ("/model op", "/model manually-supplied-model"),
        ("/docs co", "/docs invalid-topic"),
        ("/help", "/unsupported-command"),
        ("/depl", "/deploy staging --dry-run"),
    ] {
        let mut app = app();
        draft(&mut app, matching);
        assert!(!app.slash.visible().expect("initial matches").candidates.is_empty());
        draft(&mut app, literal);
        key(&mut app, KeyCode::Enter);
        assert_eq!(deferred_draft(&app), literal);
        assert_eq!(app.active_autocomplete_kind(), None);
    }
    let mut app = app();
    draft(&mut app, "/effort high extra");
    app.input.set_cursor(0, 13);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Enter);
    assert_eq!(deferred_draft(&app), "/effort high extra");
}

#[test]
fn modified_enter_in_a_slash_menu_inserts_a_newline_without_submitting() {
    for modifiers in [KeyModifiers::SHIFT, KeyModifiers::CONTROL] {
        let mut app = app();
        draft(&mut app, "/effort");

        handle_terminal_event(&mut app, Event::Key(KeyEvent::new(KeyCode::Enter, modifiers)));

        assert_eq!(app.input.text(), "/effort\n");
        assert!(app.pending_submit.is_none());
    }
}

#[test]
fn completing_a_required_command_before_existing_whitespace_enters_argument_mode() {
    let mut app = app();
    draft(&mut app, "/effort ");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.input.text(), "/effort ");
    assert_eq!(app.input.cursor(), (0, 8));
    assert!(app.pending_submit.is_none());
    assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash));
    select(&mut app, "max");
    assert_selected(&app, "max");
    key(&mut app, KeyCode::Enter);
    assert_eq!(deferred_draft(&app), "/effort max ");
}

#[test]
fn optional_arguments_and_passive_inventory_refresh_do_not_open_a_popup() {
    let mut app = app();
    draft(&mut app, "/clea");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.input.text(), "/clear  ");
    assert_eq!(app.active_autocomplete_kind(), None);
    // A prepared draft models argument editing without coupling this test to
    // the timing-based paste detector, which has separate regression coverage.
    draft(&mut app, "/clear  investigation 🦀");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.active_autocomplete_kind(), None);
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "test-session".into(),
            update: model::SessionUpdate::AvailableCommandsUpdate(
                model::AvailableCommandsUpdate::new(vec![
                    model::AvailableCommand::new("clear", "Updated metadata").input_hint("[name]"),
                ]),
            ),
        },
    );
    assert_eq!(app.active_autocomplete_kind(), None);
    key(&mut app, KeyCode::Enter);
    assert_eq!(deferred_draft(&app), "/clear  investigation 🦀");
}

#[test]
fn tab_explicitly_opens_opaque_sdk_argument_help_without_mutating_the_draft() {
    for (input, hint) in [
        ("/clear ", "[conversation label]"),
        ("/reset ", "[conversation label]"),
        ("/new ", "[conversation label]"),
        ("/deploy ", "<deployment target>"),
        ("/ship staging", "<deployment target>"),
    ] {
        let mut app = app();
        advertise(
            &mut app,
            vec![
                model::AvailableCommand::new("clear", "Clear")
                    .input_hint("[conversation label]")
                    .aliases(vec!["reset".into(), "new".into()]),
                model::AvailableCommand::new("deploy", "Deploy")
                    .input_hint("<deployment target>")
                    .aliases(vec!["ship".into()]),
            ],
        );
        draft(&mut app, input);
        assert_eq!(app.active_autocomplete_kind(), None);
        for _ in 0..2 {
            key(&mut app, KeyCode::Tab);
            let menu = app.slash.visible().expect("explicit SDK hint");
            assert!(menu.candidates.is_empty());
            assert!(menu.placeholder.as_ref().is_some_and(|p| p.contains(hint)));
            assert_eq!(app.input.text(), input);
            assert!(app.pending_submit.is_none());
        }
        key(&mut app, KeyCode::Enter);
        assert_eq!(deferred_draft(&app), input);
        assert_eq!(app.active_autocomplete_kind(), None);
    }
}

#[test]
fn space_never_accepts_the_selected_command_and_only_required_arguments_open_automatically() {
    for (text, menu_after_space) in [
        ("/clea", false),
        ("/clear", false),
        ("/", false),
        ("/unknown", false),
        ("/effort", true),
        ("/btw", false),
    ] {
        let mut app = app();
        draft(&mut app, text);
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Char(' '));
        assert_eq!(app.input.text(), format!("{text} "));
        assert!(app.pending_submit.is_none());
        assert_eq!(
            app.active_autocomplete_kind(),
            menu_after_space.then_some(AutocompleteKind::Slash),
            "{text}"
        );
    }
}

#[test]
fn escape_dismissal_survives_argument_edits_and_tab_explicitly_completes_again() {
    let mut app = app();
    draft(&mut app, "/effort ");
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.focus_owner(), FocusOwner::Input);
    draft(&mut app, "/effort hi");
    assert_eq!(app.active_autocomplete_kind(), None);
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.active_autocomplete_kind(), None);
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.input.text(), "/effort high ");
    assert_eq!(app.active_autocomplete_kind(), None);
    assert!(app.pending_submit.is_none());
    key(&mut app, KeyCode::End);
    assert_eq!(app.active_autocomplete_kind(), None);
    key(&mut app, KeyCode::Enter);
    assert_eq!(deferred_draft(&app).trim(), "/effort high");
}

#[test]
fn editing_the_command_token_reopens_completion_after_dismissal() {
    let mut unchanged = app();
    draft(&mut unchanged, "/effort hi");
    key(&mut unchanged, KeyCode::Esc);
    key(&mut unchanged, KeyCode::Left);
    key(&mut unchanged, KeyCode::Right);
    assert_eq!(unchanged.input.text(), "/effort hi");
    assert_eq!(unchanged.active_autocomplete_kind(), None);
    assert!(unchanged.pending_submit.is_none());

    for interior in [false, true] {
        let mut app = app();
        draft(&mut app, "/effort ");
        key(&mut app, KeyCode::Esc);
        if interior {
            key(&mut app, KeyCode::Backspace);
            key(&mut app, KeyCode::Esc);
            key(&mut app, KeyCode::Left);
            assert_eq!(app.active_autocomplete_kind(), None);
        }
        key(&mut app, KeyCode::Backspace);
        assert_eq!(app.input.text(), if interior { "/effot" } else { "/effort" });
        let menu = app.slash.visible().expect("edited command menu");
        assert!(menu.candidates.iter().any(|c| c.insert_value == "/effort"));
        assert!(menu.candidates.iter().all(|c| c.insert_value != "/help"));
        select(&mut app, "/effort");
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.input.text(), "/effort ");
        assert!(app.pending_submit.is_none());
        select(&mut app, "high");
        key(&mut app, KeyCode::Enter);
        assert_eq!(deferred_draft(&app), "/effort high ");
    }
}

#[test]
fn completion_preserves_unicode_whitespace_and_existing_suffixes() {
    for (text, col, expected, menu) in [
        ("\u{2003}/clea 🦀", 6, "\u{2003}/clear 🦀", false),
        ("/effo high", 5, "/effort high", true),
        ("/new-sess ", 9, "/new-session ", false),
    ] {
        let mut app = app();
        app.input.set_text(text);
        app.input.set_cursor(0, col - 1);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), expected);
        assert_eq!(app.active_autocomplete_kind(), menu.then_some(AutocompleteKind::Slash));
        assert!(app.pending_submit.is_none());
    }
}

#[test]
fn app_owned_completion_and_handoff_ignore_conflicting_sdk_hints() {
    for (command, hint, expected, ready, argument, complete) in [
        ("effort", "[level]", "/effort ", false, "/effort high", "/effort high "),
        ("compact", "[instructions]", "/compact ", true, "/compact", "/compact "),
        ("btw", "[question]", "/btw ", false, "/btw Why  now?", "/btw Why  now?"),
    ] {
        let fixture = || {
            let mut app = app();
            advertise(
                &mut app,
                vec![model::AvailableCommand::new(command, "SDK collision").input_hint(hint)],
            );
            app
        };
        for code in [KeyCode::Tab, KeyCode::Enter] {
            let mut app = fixture();
            draft(&mut app, &format!("/{command}"));
            key(&mut app, code);
            assert_eq!(app.input.text(), expected);
            if code == KeyCode::Enter && ready {
                assert_eq!(deferred_draft(&app), expected);
            } else {
                assert!(app.pending_submit.is_none());
            }
            assert_eq!(
                app.active_autocomplete_kind(),
                (command == "effort").then_some(AutocompleteKind::Slash)
            );
            if code == KeyCode::Tab && command == "compact" {
                key(&mut app, KeyCode::Tab);
                assert_eq!(app.active_autocomplete_kind(), None);
                assert!(app.pending_submit.is_none());
            }
            let mut app = fixture();
            draft(&mut app, argument);
            key(&mut app, KeyCode::Enter);
            assert_eq!(deferred_draft(&app), complete);
        }
    }
}

#[test]
fn only_first_nonblank_line_commands_complete_before_handoff() {
    for (source, row, col, expected, completion) in [
        ("Explain this:\n/clea", 1, 5, "Explain this:\n/clea", false),
        ("/btw Why?\n/effort hi", 1, 10, "/btw Why?\n/effort hi", false),
        ("/clear investigation\n/new", 1, 4, "/clear investigation\n/new", false),
        ("\n \n\u{2003}/clea", 2, 6, "\n \n\u{2003}/clear ", true),
    ] {
        let mut app = app();
        app.input.set_text(source);
        app.input.set_cursor(row, col - 1);
        key(&mut app, KeyCode::Right);
        assert_eq!(app.active_autocomplete_kind(), completion.then_some(AutocompleteKind::Slash));
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), expected);
        assert_eq!(app.active_autocomplete_kind(), None);
        assert!(app.pending_submit.is_none());
        key(&mut app, KeyCode::Enter);
        assert_eq!(deferred_draft(&app), expected);
    }
}

#[test]
fn effort_completion_follows_reported_model_choices_and_keeps_reset_available() {
    for (supports, levels, expected, targets) in [
        (
            true,
            vec![model::EffortLevel::Low, model::EffortLevel::High],
            vec!["low", "high", "reset"],
            vec!["high", "reset"],
        ),
        (
            true,
            vec![model::EffortLevel::Medium, model::EffortLevel::Max],
            vec!["medium", "max", "reset"],
            vec!["max", "reset"],
        ),
        (false, vec![], vec!["reset"], vec!["reset"]),
    ] {
        for target in targets {
            let mut app = app();
            handle_client_event(
                &mut app,
                ClientEvent::SessionUpdate {
                    session_id: "test-session".into(),
                    update: model::SessionUpdate::CurrentModelUpdate(
                        model::CurrentModelUpdate::new(
                            model::CurrentModel::new("next-model", "Next", "Next")
                                .supports_effort(supports)
                                .supported_effort_levels(levels.clone()),
                        ),
                    ),
                },
            );
            draft(&mut app, "/effort");
            key(&mut app, KeyCode::Enter);
            let actual = app
                .slash
                .visible()
                .unwrap()
                .candidates
                .iter()
                .map(|c| c.insert_value.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(actual, expected.iter().copied().collect());
            select(&mut app, target);
            key(&mut app, KeyCode::Enter);
            assert_eq!(deferred_draft(&app), format!("/effort {target} "));
        }
    }
}

#[allow(clippy::expect_used, clippy::panic)]
fn select(app: &mut App, target: &str) {
    let limit = app.slash.visible().expect("menu").candidates.len();
    for _ in 0..limit {
        let menu = app.slash.visible().expect("menu");
        if menu.candidates[menu.dialog.selected].insert_value == target {
            return;
        }
        key(app, KeyCode::Down);
    }
    panic!("expected selectable target {target}");
}

#[allow(clippy::expect_used)]
fn assert_selected(app: &App, target: &str) {
    let menu = app.slash.visible().expect("menu");
    assert_eq!(menu.candidates[menu.dialog.selected].insert_value, target);
    assert_eq!(app.focus_owner(), FocusOwner::Mention);
}

fn advertise(app: &mut App, commands: Vec<model::AvailableCommand>) {
    handle_client_event(
        app,
        ClientEvent::SessionUpdate {
            session_id: "test-session".into(),
            update: model::SessionUpdate::AvailableCommandsUpdate(
                model::AvailableCommandsUpdate::new(commands),
            ),
        },
    );
}
