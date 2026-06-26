// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crossterm::event::KeyModifiers;

use super::action::{AppAction, AutocompleteAction, InputAction, InteractionAction, KeyAction};
use super::binding::{KeyBinding, KeyContext};
use super::spec::{KeyCodeSpec, KeySpec};
pub fn default_bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    bindings.extend(global_default_bindings());
    bindings.extend(chat_blocked_default_bindings());
    bindings.extend(chat_control_default_bindings());
    bindings.extend(chat_navigation_default_bindings());
    bindings.extend(chat_readline_default_bindings());
    bindings.extend(chat_history_default_bindings());
    bindings.extend(autocomplete_default_bindings());
    bindings.extend(interaction_default_bindings());
    bindings
}

fn global_default_bindings() -> Vec<KeyBinding> {
    let bindings = vec![
        KeyBinding::default(
            KeyContext::Global,
            KeySpec::char('q', KeyModifiers::CONTROL),
            KeyAction::App(AppAction::Quit),
        ),
        KeyBinding::default(
            KeyContext::Global,
            KeySpec::char('l', KeyModifiers::CONTROL),
            KeyAction::App(AppAction::Redraw),
        ),
    ];
    #[cfg(unix)]
    {
        let mut unix_bindings = bindings;
        unix_bindings.push(KeyBinding::default(
            KeyContext::Global,
            KeySpec::char('z', KeyModifiers::CONTROL),
            KeyAction::Terminal(TerminalAction::Suspend),
        ));
        unix_bindings
    }
    #[cfg(not(unix))]
    {
        bindings
    }
}

fn chat_blocked_default_bindings() -> [KeyBinding; 1] {
    [KeyBinding::default(
        KeyContext::ChatBlocked,
        KeySpec::char('c', KeyModifiers::CONTROL),
        KeyAction::App(AppAction::Quit),
    )]
}

fn chat_control_default_bindings() -> [KeyBinding; 7] {
    [
        KeyBinding::default(
            KeyContext::ChatInput,
            KeySpec::char('c', KeyModifiers::CONTROL),
            KeyAction::App(AppAction::ClearInputOrQuit),
        ),
        KeyBinding::default(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Esc, KeyModifiers::NONE),
            KeyAction::App(AppAction::CancelTurn),
        ),
        KeyBinding::default(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Enter, KeyModifiers::NONE),
            KeyAction::App(AppAction::SubmitInput),
        ),
        KeyBinding::default(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Enter, KeyModifiers::SHIFT),
            KeyAction::Input(InputAction::InsertNewline),
        ),
        KeyBinding::default(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Enter, KeyModifiers::CONTROL),
            KeyAction::Input(InputAction::InsertNewline),
        ),
        KeyBinding::default(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Tab, KeyModifiers::NONE),
            KeyAction::App(AppAction::FocusPromptOrAcceptSuggestion),
        ),
        KeyBinding::default(
            KeyContext::ChatInput,
            KeySpec::new(KeyCodeSpec::Tab, KeyModifiers::SHIFT),
            KeyAction::App(AppAction::CycleMode),
        ),
    ]
}

fn chat_navigation_default_bindings() -> [KeyBinding; 16] {
    [
        chat_key(KeyCodeSpec::Left, KeyModifiers::NONE, InputAction::MoveCharLeft),
        chat_key(KeyCodeSpec::Right, KeyModifiers::NONE, InputAction::MoveCharRight),
        chat_key(KeyCodeSpec::Up, KeyModifiers::NONE, InputAction::MoveUp),
        chat_key(KeyCodeSpec::Down, KeyModifiers::NONE, InputAction::MoveDown),
        chat_key(KeyCodeSpec::Home, KeyModifiers::NONE, InputAction::MoveLineStart),
        chat_key(KeyCodeSpec::End, KeyModifiers::NONE, InputAction::MoveLineEnd),
        chat_key(KeyCodeSpec::Backspace, KeyModifiers::NONE, InputAction::DeleteCharBefore),
        chat_key(KeyCodeSpec::Delete, KeyModifiers::NONE, InputAction::DeleteCharAfter),
        chat_key(KeyCodeSpec::Left, KeyModifiers::CONTROL, InputAction::MoveWordLeft),
        chat_key(KeyCodeSpec::Right, KeyModifiers::CONTROL, InputAction::MoveWordRight),
        chat_key(KeyCodeSpec::Left, KeyModifiers::ALT, InputAction::MoveWordLeft),
        chat_key(KeyCodeSpec::Right, KeyModifiers::ALT, InputAction::MoveWordRight),
        chat_key(KeyCodeSpec::Backspace, KeyModifiers::CONTROL, InputAction::DeleteWordBefore),
        chat_key(KeyCodeSpec::Delete, KeyModifiers::CONTROL, InputAction::DeleteWordAfter),
        chat_key(KeyCodeSpec::Backspace, KeyModifiers::ALT, InputAction::DeleteWordBefore),
        chat_key(KeyCodeSpec::Delete, KeyModifiers::ALT, InputAction::DeleteWordAfter),
    ]
}

fn chat_readline_default_bindings() -> [KeyBinding; 13] {
    [
        chat_input('a', KeyModifiers::CONTROL, InputAction::MoveLineStart),
        chat_input('e', KeyModifiers::CONTROL, InputAction::MoveLineEnd),
        chat_input('b', KeyModifiers::CONTROL, InputAction::MoveCharLeft),
        chat_input('f', KeyModifiers::CONTROL, InputAction::MoveCharRight),
        chat_input('d', KeyModifiers::CONTROL, InputAction::DeleteCharAfter),
        chat_input('k', KeyModifiers::CONTROL, InputAction::KillLineEnd),
        chat_input('u', KeyModifiers::CONTROL, InputAction::KillLineStart),
        chat_input('y', KeyModifiers::CONTROL, InputAction::Yank),
        chat_input('b', KeyModifiers::ALT, InputAction::MoveWordLeft),
        chat_input('f', KeyModifiers::ALT, InputAction::MoveWordRight),
        chat_input('d', KeyModifiers::ALT, InputAction::DeleteWordAfter),
        chat_input('w', KeyModifiers::CONTROL, InputAction::DeleteWordBefore),
        chat_input('h', KeyModifiers::CONTROL, InputAction::DeleteCharBefore),
    ]
}

fn chat_history_default_bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();

    #[cfg(target_os = "macos")]
    {
        bindings.push(chat_input('z', KeyModifiers::SUPER, InputAction::Undo));
        bindings.push(chat_input(
            'z',
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
            InputAction::Redo,
        ));
        bindings.push(chat_input('y', KeyModifiers::SUPER, InputAction::Redo));
    }

    #[cfg(target_os = "windows")]
    {
        bindings.push(chat_input('z', KeyModifiers::CONTROL, InputAction::Undo));
        bindings.push(chat_input(
            'z',
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            InputAction::Redo,
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        bindings.push(chat_input('_', KeyModifiers::CONTROL, InputAction::Undo));
        bindings.push(chat_input('/', KeyModifiers::CONTROL, InputAction::Undo));
        bindings.push(chat_input(
            'z',
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            InputAction::Redo,
        ));
    }

    bindings
}

fn autocomplete_default_bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for context in [
        KeyContext::AutocompleteMention,
        KeyContext::AutocompleteSlash,
        KeyContext::AutocompleteSubagent,
    ] {
        bindings.extend([
            autocomplete(context, KeyCodeSpec::Up, AutocompleteAction::MovePrevious),
            autocomplete(context, KeyCodeSpec::Down, AutocompleteAction::MoveNext),
            autocomplete(context, KeyCodeSpec::Enter, AutocompleteAction::Confirm),
            autocomplete(context, KeyCodeSpec::Tab, AutocompleteAction::Confirm),
            autocomplete(context, KeyCodeSpec::Esc, AutocompleteAction::Cancel),
        ]);
    }
    bindings
}

fn interaction_default_bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    bindings.extend([
        interaction(
            KeyContext::InlinePermission,
            KeyCodeSpec::Left,
            InteractionAction::MovePrevious,
        ),
        interaction(KeyContext::InlinePermission, KeyCodeSpec::Up, InteractionAction::MovePrevious),
        interaction(KeyContext::InlinePermission, KeyCodeSpec::Right, InteractionAction::MoveNext),
        interaction(KeyContext::InlinePermission, KeyCodeSpec::Down, InteractionAction::MoveNext),
        interaction(KeyContext::InlinePermission, KeyCodeSpec::Enter, InteractionAction::Confirm),
        interaction(KeyContext::InlinePermission, KeyCodeSpec::Esc, InteractionAction::Cancel),
        interaction(KeyContext::InlinePermission, KeyCodeSpec::Tab, InteractionAction::FocusNext),
    ]);
    bindings.extend([
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Left, InteractionAction::MovePrevious),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Up, InteractionAction::MovePrevious),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Right, InteractionAction::MoveNext),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Down, InteractionAction::MoveNext),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Home, InteractionAction::MoveStart),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::End, InteractionAction::MoveEnd),
        interaction(
            KeyContext::InlineQuestion,
            KeyCodeSpec::Char(' '),
            InteractionAction::ToggleSelection,
        ),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Enter, InteractionAction::Confirm),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Esc, InteractionAction::Cancel),
        interaction(KeyContext::InlineQuestion, KeyCodeSpec::Tab, InteractionAction::ToggleNotes),
        interaction_with_modifiers(
            KeyContext::InlineQuestion,
            KeyCodeSpec::Tab,
            KeyModifiers::SHIFT,
            InteractionAction::ToggleNotes,
        ),
    ]);
    bindings
}

fn chat_input(ch: char, modifiers: KeyModifiers, action: InputAction) -> KeyBinding {
    KeyBinding::default(
        KeyContext::ChatInput,
        KeySpec::char(ch, modifiers),
        KeyAction::Input(action),
    )
}

fn chat_key(code: KeyCodeSpec, modifiers: KeyModifiers, action: InputAction) -> KeyBinding {
    KeyBinding::default(
        KeyContext::ChatInput,
        KeySpec::new(code, modifiers),
        KeyAction::Input(action),
    )
}

fn autocomplete(context: KeyContext, code: KeyCodeSpec, action: AutocompleteAction) -> KeyBinding {
    KeyBinding::default(
        context,
        KeySpec::new(code, KeyModifiers::NONE),
        KeyAction::Autocomplete(action),
    )
}

fn interaction(context: KeyContext, code: KeyCodeSpec, action: InteractionAction) -> KeyBinding {
    interaction_with_modifiers(context, code, KeyModifiers::NONE, action)
}

fn interaction_with_modifiers(
    context: KeyContext,
    code: KeyCodeSpec,
    modifiers: KeyModifiers,
    action: InteractionAction,
) -> KeyBinding {
    KeyBinding::default(context, KeySpec::new(code, modifiers), KeyAction::Interaction(action))
}
