// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::action::{
    AppAction, AutocompleteAction, InputAction, InteractionAction, KeyAction, TerminalAction,
};
use super::binding::KeyContext;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyActionDescriptor {
    pub action: KeyAction,
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub default_contexts: &'static [KeyContext],
}

pub fn action_descriptor(action: KeyAction) -> Option<&'static KeyActionDescriptor> {
    action_catalog().iter().find(|descriptor| descriptor.action == action)
}

pub fn action_catalog() -> &'static [KeyActionDescriptor] {
    ACTION_CATALOG
}

const ACTION_CATALOG: &[KeyActionDescriptor] = &[
    KeyActionDescriptor {
        action: KeyAction::App(AppAction::Quit),
        id: "app.quit",
        label: "Quit",
        description: "Quit the application.",
        default_contexts: &[KeyContext::Global, KeyContext::ChatBlocked],
    },
    KeyActionDescriptor {
        action: KeyAction::App(AppAction::ClearInputOrQuit),
        id: "app.clear_input_or_quit",
        label: "Clear draft / quit",
        description: "Clear local input state, or quit when input is already empty.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::App(AppAction::Redraw),
        id: "app.redraw",
        label: "Redraw screen",
        description: "Request a visible chat redraw.",
        default_contexts: &[KeyContext::Global],
    },
    KeyActionDescriptor {
        action: KeyAction::App(AppAction::CancelTurn),
        id: "app.cancel_turn",
        label: "Cancel turn",
        description: "Cancel the active turn from chat input.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::App(AppAction::SubmitInput),
        id: "app.submit_input",
        label: "Send message",
        description: "Submit the current chat input.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::App(AppAction::FocusPromptOrAcceptSuggestion),
        id: "app.focus_prompt_or_accept_suggestion",
        label: "Focus prompt / accept suggestion",
        description: "Focus a pending prompt, or accept the current prompt suggestion.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::App(AppAction::CycleMode),
        id: "app.cycle_mode",
        label: "Cycle mode",
        description: "Cycle to the next available model mode.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveCharLeft),
        id: "input.move_char_left",
        label: "Move left",
        description: "Move the input cursor one character left.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveCharRight),
        id: "input.move_char_right",
        label: "Move right",
        description: "Move the input cursor one character right.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveWordLeft),
        id: "input.move_word_left",
        label: "Move word left",
        description: "Move the input cursor one word left.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveWordRight),
        id: "input.move_word_right",
        label: "Move word right",
        description: "Move the input cursor one word right.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveLineStart),
        id: "input.move_line_start",
        label: "Move line start",
        description: "Move the input cursor to the start of the line.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveLineEnd),
        id: "input.move_line_end",
        label: "Move line end",
        description: "Move the input cursor to the end of the line.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveLineStartOrUp),
        id: "input.move_line_start_or_up",
        label: "Move line start, then up",
        description: "Move to the current line start, or the previous line start when already there.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveLineEndOrDown),
        id: "input.move_line_end_or_down",
        label: "Move line end, then down",
        description: "Move to the current line end, or the next line end when already there.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveUp),
        id: "input.move_up",
        label: "Move up",
        description: "Move the input cursor up, or browse chat history.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::MoveDown),
        id: "input.move_down",
        label: "Move down",
        description: "Move the input cursor down, or browse chat history.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::DeleteCharBefore),
        id: "input.delete_char_before",
        label: "Delete before cursor",
        description: "Delete the character before the input cursor.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::DeleteCharAfter),
        id: "input.delete_char_after",
        label: "Delete after cursor",
        description: "Delete the character after the input cursor.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::DeleteWordBefore),
        id: "input.delete_word_before",
        label: "Delete word before cursor",
        description: "Delete the word before the input cursor.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::DeleteWordAfter),
        id: "input.delete_word_after",
        label: "Delete word after cursor",
        description: "Delete the word after the input cursor.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::KillLineStart),
        id: "input.kill_line_start",
        label: "Kill line start",
        description: "Delete input text from the cursor to the start of the line.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::KillLineEnd),
        id: "input.kill_line_end",
        label: "Kill line end",
        description: "Delete input text from the cursor to the end of the line.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::Yank),
        id: "input.yank",
        label: "Yank",
        description: "Paste the most recently killed input text.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::Undo),
        id: "input.undo",
        label: "Undo",
        description: "Undo the previous input edit.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::Redo),
        id: "input.redo",
        label: "Redo",
        description: "Redo the previously undone input edit.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Input(InputAction::InsertNewline),
        id: "input.insert_newline",
        label: "Insert newline",
        description: "Insert a newline into the current input draft.",
        default_contexts: &[KeyContext::ChatInput],
    },
    KeyActionDescriptor {
        action: KeyAction::Autocomplete(AutocompleteAction::MovePrevious),
        id: "autocomplete.move_previous",
        label: "Previous suggestion",
        description: "Move to the previous autocomplete suggestion.",
        default_contexts: &[
            KeyContext::AutocompleteMention,
            KeyContext::AutocompleteSlash,
            KeyContext::AutocompleteSubagent,
        ],
    },
    KeyActionDescriptor {
        action: KeyAction::Autocomplete(AutocompleteAction::MoveNext),
        id: "autocomplete.move_next",
        label: "Next suggestion",
        description: "Move to the next autocomplete suggestion.",
        default_contexts: &[
            KeyContext::AutocompleteMention,
            KeyContext::AutocompleteSlash,
            KeyContext::AutocompleteSubagent,
        ],
    },
    KeyActionDescriptor {
        action: KeyAction::Autocomplete(AutocompleteAction::Confirm),
        id: "autocomplete.confirm",
        label: "Confirm suggestion",
        description: "Confirm the selected autocomplete suggestion.",
        default_contexts: &[
            KeyContext::AutocompleteMention,
            KeyContext::AutocompleteSlash,
            KeyContext::AutocompleteSubagent,
        ],
    },
    KeyActionDescriptor {
        action: KeyAction::Autocomplete(AutocompleteAction::Cancel),
        id: "autocomplete.cancel",
        label: "Cancel autocomplete",
        description: "Close the active autocomplete menu.",
        default_contexts: &[
            KeyContext::AutocompleteMention,
            KeyContext::AutocompleteSlash,
            KeyContext::AutocompleteSubagent,
        ],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::MovePrevious),
        id: "interaction.move_previous",
        label: "Previous option",
        description: "Move to the previous inline prompt option.",
        default_contexts: &[KeyContext::InlinePermission, KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::MoveNext),
        id: "interaction.move_next",
        label: "Next option",
        description: "Move to the next inline prompt option.",
        default_contexts: &[KeyContext::InlinePermission, KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::MoveStart),
        id: "interaction.move_start",
        label: "First option",
        description: "Move to the first inline prompt option.",
        default_contexts: &[KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::MoveEnd),
        id: "interaction.move_end",
        label: "Last option",
        description: "Move to the last inline prompt option.",
        default_contexts: &[KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::Confirm),
        id: "interaction.confirm",
        label: "Confirm option",
        description: "Confirm the selected inline prompt option.",
        default_contexts: &[KeyContext::InlinePermission, KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::Cancel),
        id: "interaction.cancel",
        label: "Cancel prompt",
        description: "Cancel or reject the active inline prompt.",
        default_contexts: &[KeyContext::InlinePermission, KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::FocusNext),
        id: "interaction.focus_next",
        label: "Return to draft / next prompt",
        description: "Return focus to the draft, or move to the next inline permission prompt.",
        default_contexts: &[KeyContext::InlinePermission],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::ToggleSelection),
        id: "interaction.toggle_selection",
        label: "Toggle selection",
        description: "Toggle the selected inline question option.",
        default_contexts: &[KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Interaction(InteractionAction::ToggleNotes),
        id: "interaction.toggle_notes",
        label: "Toggle notes",
        description: "Toggle notes editing for the active inline question.",
        default_contexts: &[KeyContext::InlineQuestion],
    },
    KeyActionDescriptor {
        action: KeyAction::Terminal(TerminalAction::Suspend),
        id: "terminal.suspend",
        label: "Suspend process",
        description: "Suspend the TUI process after restoring terminal state.",
        default_contexts: &[KeyContext::Global],
    },
];
