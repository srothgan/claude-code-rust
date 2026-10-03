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
fn submitted_text(app: &App) -> String {
    app.pending_submit.as_ref().expect("one Enter must arm submission").lines.join("\n")
}

#[test]
fn exact_names_win_over_competing_plugins_for_every_app_command_and_sdk_alias() {
    for command in NO_ARGUMENT_COMMANDS
        .iter()
        .copied()
        .chain(REQUIRED_ARGUMENT_COMMANDS.iter().map(|(command, _)| *command))
        .chain(["/resume", "/clear", "/new", "/reset", "/ping", "/deploy", "/ship"])
    {
        for code in [KeyCode::Tab, KeyCode::Enter] {
            let mut app = app();
            let name = command.trim_start_matches('/');
            app.sdk_inventory
                .available_commands
                .push(model::AvailableCommand::new(format!("a-{name}"), "Competing substring"));
            app.sdk_inventory
                .available_commands
                .push(model::AvailableCommand::new(format!("{name}-extra"), "Competing prefix"));
            draft(&mut app, command);
            assert_eq!(app.slash.visible().expect("menu").candidates[0].insert_value, command);

            key(&mut app, code);

            assert_eq!(app.input.text().trim(), command, "{command}: {code:?}");
            let required = REQUIRED_ARGUMENT_COMMANDS.iter().any(|(name, _)| *name == command);
            assert_eq!(
                app.pending_submit.is_some(),
                code == KeyCode::Enter && !required,
                "{command}: {code:?}"
            );
        }
    }
}

#[test]
fn explicit_arrow_selection_can_override_an_exact_command_name() {
    for code in [KeyCode::Tab, KeyCode::Enter] {
        let mut app = app();
        app.sdk_inventory
            .available_commands
            .push(model::AvailableCommand::new("a-help", "Competing plugin"));
        draft(&mut app, "/help");
        key(&mut app, KeyCode::Down);
        key(&mut app, code);

        assert_eq!(app.input.text().trim(), "/a-help");
        assert_eq!(app.pending_submit.is_some(), code == KeyCode::Enter);
    }
}

#[test]
fn commands_without_arguments_submit_on_the_first_enter() {
    for command in NO_ARGUMENT_COMMANDS {
        let mut app = app();
        draft(&mut app, command);
        assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash), "{command}");

        key(&mut app, KeyCode::Enter);

        assert_eq!(submitted_text(&app).trim(), *command);
        assert_eq!(app.focus_owner(), FocusOwner::Input, "{command}");
        assert_eq!(app.active_autocomplete_kind(), None, "{command}");
    }
}

#[test]
fn commands_requiring_arguments_complete_the_name_without_submitting() {
    for (command, _) in REQUIRED_ARGUMENT_COMMANDS {
        let mut app = app();
        draft(&mut app, command);

        key(&mut app, KeyCode::Enter);

        assert!(app.pending_submit.is_none(), "{command} must wait for arguments");
        assert_eq!(app.input.text(), format!("{command} "));
    }
}

#[test]
fn complete_argument_commands_submit_on_the_first_enter() {
    for (command, arguments) in REQUIRED_ARGUMENT_COMMANDS {
        let mut app = app();
        let text = format!("{command} {arguments}");
        draft(&mut app, &text);

        key(&mut app, KeyCode::Enter);

        assert_eq!(submitted_text(&app).trim(), text);
        assert_eq!(app.active_autocomplete_kind(), None, "{text}");
    }
}

#[test]
fn optional_resume_argument_can_be_omitted_or_supplied() {
    for text in ["/resume", "/resume ", "/resume manually-supplied-id"] {
        let mut app = app();
        draft(&mut app, text);

        key(&mut app, KeyCode::Enter);

        assert_eq!(submitted_text(&app).trim(), text.trim());
    }
}

#[test]
fn session_commands_and_aliases_submit_with_or_without_argument_hints() {
    for text in [
        "/clear",
        "/clear ",
        "/new",
        "/reset",
        "/clear investigation",
        "/deploy staging",
        "/ship staging",
    ] {
        let mut app = app();
        draft(&mut app, text);

        key(&mut app, KeyCode::Enter);

        assert_eq!(submitted_text(&app).trim(), text.trim());
        assert_eq!(app.focus_owner(), FocusOwner::Input);
    }
}

#[test]
fn tab_never_submits_a_command() {
    for (text, completed) in [
        ("/new", "/new"),
        ("/clear", "/clear"),
        ("/effort hi", "/effort high"),
        ("/deploy staging", "/deploy staging"),
    ] {
        let mut app = app();
        draft(&mut app, text);

        key(&mut app, KeyCode::Tab);

        assert!(app.pending_submit.is_none(), "{text}");
        assert_eq!(app.input.text().trim(), completed);
    }
}

#[test]
fn no_argument_completion_closes_the_menu_and_returns_input_focus() {
    for command in NO_ARGUMENT_COMMANDS.iter().copied().chain(["/ping"]) {
        let mut app = app();
        draft(&mut app, command);
        key(&mut app, KeyCode::Tab);

        assert_eq!(app.input.text(), format!("{command} "));
        assert!(app.pending_submit.is_none(), "{command}");
        assert_eq!(app.active_autocomplete_kind(), None, "{command}");
        assert_eq!(app.focus_owner(), FocusOwner::Input, "{command}");

        // Cursor synchronization must not reopen an argument menu.
        key(&mut app, KeyCode::End);
        assert_eq!(app.active_autocomplete_kind(), None, "{command}");
    }
}

#[test]
fn argument_commands_remain_drafts_after_name_completion() {
    for (command, keeps_menu) in [
        ("/effort", true),
        ("/btw", false),
        ("/resume", false),
        ("/clear", false),
        ("/deploy", false),
        ("/ship", false),
    ] {
        let mut app = app();
        draft(&mut app, command);
        key(&mut app, KeyCode::Tab);

        assert_eq!(app.input.text(), format!("{command} "));
        assert!(app.pending_submit.is_none());
        assert_eq!(
            app.active_autocomplete_kind(),
            keeps_menu.then_some(AutocompleteKind::Slash),
            "{command}"
        );
        assert_eq!(
            app.focus_owner(),
            if keeps_menu { FocusOwner::Mention } else { FocusOwner::Input },
            "{command}"
        );
    }
}

#[test]
fn large_command_inventories_remain_browsable_without_a_hidden_cutoff() {
    let mut app = app();
    let mut commands: Vec<_> = (0..75)
        .map(|index| model::AvailableCommand::new(format!("aaa-{index:02}"), "Plugin command"))
        .collect();
    commands.push(model::AvailableCommand::new("clear", "Clear conversation").builtin(true));
    handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "test-session".into(),
            update: model::SessionUpdate::AvailableCommandsUpdate(
                model::AvailableCommandsUpdate::new(commands),
            ),
        },
    );
    draft(&mut app, "/");
    let candidates = &app.slash.visible().expect("command menu").candidates;
    assert!(candidates.len() > 75);
    for name in ["/clear", "/new-session", "/rewind", "/usage"] {
        assert!(candidates.iter().any(|candidate| candidate.insert_value == name), "{name}");
    }
    let new_index = candidates
        .iter()
        .position(|candidate| candidate.insert_value == "/new-session")
        .expect("new session command");
    for _ in 0..new_index {
        key(&mut app, KeyCode::Down);
    }
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.input.text(), "/new-session ");
    assert_eq!(app.active_autocomplete_kind(), None);
    assert!(app.pending_submit.is_none());

    draft(&mut app, "/aaa-");
    assert_eq!(app.slash.visible().expect("filtered command menu").candidates.len(), 75);
}

#[test]
fn empty_menus_allow_validation_and_preserve_manually_typed_values() {
    for text in [
        "/effort impossible",
        "/mode manually-supplied-mode",
        "/model manually-supplied-model",
        "/docs invalid-topic",
        "/unsupported-command",
        "/deploy staging --dry-run",
    ] {
        let mut app = app();
        draft(&mut app, text);

        key(&mut app, KeyCode::Enter);

        assert_eq!(submitted_text(&app), text);
        assert_eq!(app.active_autocomplete_kind(), None, "{text}");
    }
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
fn completion_in_the_middle_of_an_invalid_command_still_reaches_validation() {
    let mut app = app();
    draft(&mut app, "/effort high extra");
    app.input.set_cursor(0, "/effort high".len() + 1);
    key(&mut app, KeyCode::Left);

    key(&mut app, KeyCode::Enter);

    assert_eq!(submitted_text(&app), "/effort high extra");
}

#[test]
fn completing_a_required_command_before_existing_whitespace_enters_argument_mode() {
    let mut app = app();
    draft(&mut app, "/effort ");
    key(&mut app, KeyCode::Left);

    key(&mut app, KeyCode::Enter);

    assert!(app.pending_submit.is_none());
    assert_eq!(app.input.cursor_col(), "/effort ".len());
    assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash));
    key(&mut app, KeyCode::Enter);
    assert_eq!(submitted_text(&app).trim(), "/effort low");
}

#[test]
fn explicit_selection_can_override_an_exact_typed_argument() {
    let mut app = app();
    draft(&mut app, "/effort high");

    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);

    assert_eq!(submitted_text(&app).trim(), "/effort xhigh");
}

#[test]
fn optional_name_completion_closes_without_losing_argument_support() {
    for (prefix, completed) in
        [("/clea", "/clear"), ("/rese", "/reset"), ("/new", "/new"), ("/resu", "/resume")]
    {
        for code in [KeyCode::Tab, KeyCode::Enter] {
            let mut app = app();
            draft(&mut app, prefix);
            key(&mut app, code);
            assert_eq!(app.input.text(), format!("{completed} "));
            assert_eq!(app.active_autocomplete_kind(), None);
            assert_eq!(app.focus_owner(), FocusOwner::Input);
            assert_eq!(app.pending_submit.is_some(), code == KeyCode::Enter);
        }
    }
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
    assert_eq!(submitted_text(&app), "/clear  investigation 🦀");
}

#[test]
fn tab_explicitly_opens_opaque_sdk_argument_help_without_mutating_the_draft() {
    for (text, hint) in [
        ("/clear ", "[name]"),
        ("/reset ", "[name]"),
        ("/new ", "[name]"),
        ("/deploy ", "<target>"),
        ("/ship staging", "<target>"),
    ] {
        let mut app = app();
        draft(&mut app, text);
        assert_eq!(app.active_autocomplete_kind(), None);
        for _ in 0..2 {
            key(&mut app, KeyCode::Tab);
            let menu = app.slash.visible().expect("explicit argument help");
            assert_eq!(menu.placeholder.as_deref(), Some(format!("Arguments: {hint}").as_str()));
            assert!(menu.candidates.is_empty());
            assert_eq!(app.input.text(), text);
            assert!(app.pending_submit.is_none());
        }
        key(&mut app, KeyCode::Enter);
        assert_eq!(submitted_text(&app), text);
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
fn argument_assistance_survives_spaces_without_submitting_or_changing_selection_into_input() {
    let mut app = app();
    draft(&mut app, "/effort");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Char(' '));
    key(&mut app, KeyCode::Char(' '));
    assert_eq!(app.input.text(), "/effort   ");
    assert!(app.pending_submit.is_none());
    assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash));
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
    assert_eq!(submitted_text(&app).trim(), "/effort high");
}

#[test]
fn editing_the_command_token_reopens_completion_after_dismissal() {
    let mut app = app();
    draft(&mut app, "/effort ");
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.input.text(), "/effort");
    assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Left);
    assert_eq!(app.active_autocomplete_kind(), None);
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash));
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
fn app_contracts_override_sdk_hints_for_both_completion_and_submission() {
    for (command, hint, has_menu) in [
        ("effort", "[level]", true),
        ("compact", "[instructions]", false),
        ("btw", "[question]", false),
    ] {
        let mut app = app();
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: "test-session".into(),
                update: model::SessionUpdate::AvailableCommandsUpdate(
                    model::AvailableCommandsUpdate::new(vec![
                        model::AvailableCommand::new(command, "SDK collision").input_hint(hint),
                    ]),
                ),
            },
        );
        draft(&mut app, &format!("/{command}"));
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.active_autocomplete_kind(), has_menu.then_some(AutocompleteKind::Slash));
        if command == "compact" {
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.active_autocomplete_kind(), None);
            key(&mut app, KeyCode::Enter);
            assert!(app.pending_submit.is_some());
        }
    }
}

#[test]
fn slashes_on_later_lines_never_complete_or_mutate_prompt_and_argument_text() {
    for text in ["Explain this:\n/clea", "/btw Why?\n/effort hi", "/clear investigation\n/new"] {
        let mut app = app();
        app.input.set_text(text);
        let row = app.input.lines().len() - 1;
        let col = app.input.lines()[row].chars().count();
        app.input.set_cursor(row, col - 1);
        key(&mut app, KeyCode::Right);
        assert_eq!(app.active_autocomplete_kind(), None);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.input.text(), text);
        assert_eq!(app.active_autocomplete_kind(), None);
        assert!(app.pending_submit.is_none());
        key(&mut app, KeyCode::Enter);
        assert_eq!(submitted_text(&app), text);
    }
}

#[test]
fn leading_blank_lines_still_allow_command_completion_and_submission() {
    let mut app = app();
    app.input.set_text("\n \n\u{2003}/clea");
    app.input.set_cursor(2, 5);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.active_autocomplete_kind(), Some(AutocompleteKind::Slash));
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.input.text(), "\n \n\u{2003}/clear ");
    assert_eq!(app.active_autocomplete_kind(), None);
    assert!(app.pending_submit.is_none());
    key(&mut app, KeyCode::Enter);
    assert_eq!(submitted_text(&app), "\n \n\u{2003}/clear ");
}

#[test]
fn effort_completion_follows_reported_model_choices_and_keeps_reset_available() {
    let mut app = app();
    for (supports, levels, expected) in [
        (
            true,
            vec![model::EffortLevel::Low, model::EffortLevel::High],
            vec!["low", "high", "reset"],
        ),
        (false, vec![], vec!["reset"]),
    ] {
        handle_client_event(
            &mut app,
            ClientEvent::SessionUpdate {
                session_id: "test-session".into(),
                update: model::SessionUpdate::CurrentModelUpdate(model::CurrentModelUpdate::new(
                    model::CurrentModel::new("next-model", "Next", "Next")
                        .supports_effort(supports)
                        .supported_effort_levels(levels),
                )),
            },
        );
        draft(&mut app, "");
        draft(&mut app, "/effort");
        key(&mut app, KeyCode::Enter);
        let choices: Vec<_> = app
            .slash
            .visible()
            .expect("effort menu")
            .candidates
            .iter()
            .map(|choice| choice.insert_value.as_str())
            .collect();
        assert_eq!(choices, expected);
        key(&mut app, KeyCode::Esc);
    }
}
