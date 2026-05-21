// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct KeySpec {
    code: KeyCodeSpec,
    modifiers: KeyModifiers,
}

impl KeySpec {
    pub fn new(code: KeyCodeSpec, modifiers: KeyModifiers) -> Self {
        Self { code: code.normalized_for_modifiers(modifiers), modifiers }
    }

    pub fn char(ch: char, modifiers: KeyModifiers) -> Self {
        Self::new(KeyCodeSpec::Char(ch), modifiers)
    }

    pub fn from_event(key: KeyEvent) -> Option<Self> {
        let mut modifiers = key.modifiers;
        let code = match key.code {
            KeyCode::Backspace => KeyCodeSpec::Backspace,
            KeyCode::Enter => KeyCodeSpec::Enter,
            KeyCode::Left => KeyCodeSpec::Left,
            KeyCode::Right => KeyCodeSpec::Right,
            KeyCode::Up => KeyCodeSpec::Up,
            KeyCode::Down => KeyCodeSpec::Down,
            KeyCode::Home => KeyCodeSpec::Home,
            KeyCode::End => KeyCodeSpec::End,
            KeyCode::PageUp => KeyCodeSpec::PageUp,
            KeyCode::PageDown => KeyCodeSpec::PageDown,
            KeyCode::Tab => KeyCodeSpec::Tab,
            KeyCode::BackTab => {
                modifiers.insert(KeyModifiers::SHIFT);
                KeyCodeSpec::Tab
            }
            KeyCode::Delete => KeyCodeSpec::Delete,
            KeyCode::Insert => KeyCodeSpec::Insert,
            KeyCode::F(index) => KeyCodeSpec::F(index),
            KeyCode::Char(ch) => normalized_char_code(ch, &mut modifiers),
            KeyCode::Esc => KeyCodeSpec::Esc,
            KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => return None,
        };
        Some(Self::new(code, modifiers))
    }

    pub fn code(&self) -> KeyCodeSpec {
        self.code
    }

    pub fn modifiers(&self) -> KeyModifiers {
        self.modifiers
    }

    pub fn matches_event(&self, key: KeyEvent) -> bool {
        Self::from_event(key).is_some_and(|candidate| candidate == *self)
    }
}

impl fmt::Display for KeySpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".to_owned());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt".to_owned());
        }
        if self.modifiers.contains(KeyModifiers::SUPER) {
            parts.push("cmd".to_owned());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift".to_owned());
        }
        parts.push(self.code.to_string());
        formatter.write_str(&parts.join("-"))
    }
}

impl FromStr for KeySpec {
    type Err = ParseKeySpecError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let normalized = input.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(ParseKeySpecError::Empty);
        }

        let mut modifiers = KeyModifiers::NONE;
        let tokens: Vec<&str> = normalized.split('-').filter(|token| !token.is_empty()).collect();
        if tokens.is_empty() {
            return Err(ParseKeySpecError::Empty);
        }

        let mut key_start = None;
        for (index, token) in tokens.iter().enumerate() {
            if let Some(modifier) = parse_modifier(token) {
                if modifiers.contains(modifier) {
                    return Err(ParseKeySpecError::DuplicateModifier((*token).to_owned()));
                }
                modifiers.insert(modifier);
            } else {
                key_start = Some(index);
                break;
            }
        }

        let Some(key_start) = key_start else {
            return Err(ParseKeySpecError::MissingKey);
        };
        let key_name = tokens[key_start..].join("-");
        let code = parse_key_code(&key_name)
            .ok_or_else(|| ParseKeySpecError::UnsupportedKey(key_name.clone()))?;
        Ok(Self::new(code, modifiers))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyCodeSpec {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Insert,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

impl KeyCodeSpec {
    fn normalized_for_modifiers(self, modifiers: KeyModifiers) -> Self {
        match self {
            Self::Char(ch) if should_canonicalize_char(ch, modifiers) => {
                Self::Char(ch.to_ascii_lowercase())
            }
            _ => self,
        }
    }
}

impl fmt::Display for KeyCodeSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(' ') => formatter.write_str("space"),
            Self::Char(ch) if ch.is_ascii_control() => {
                write!(formatter, "u+{:04x}", u32::from(*ch))
            }
            Self::Char(ch) => write!(formatter, "{ch}"),
            Self::Enter => formatter.write_str("enter"),
            Self::Esc => formatter.write_str("esc"),
            Self::Backspace => formatter.write_str("backspace"),
            Self::Delete => formatter.write_str("delete"),
            Self::Insert => formatter.write_str("insert"),
            Self::Tab => formatter.write_str("tab"),
            Self::Left => formatter.write_str("left"),
            Self::Right => formatter.write_str("right"),
            Self::Up => formatter.write_str("up"),
            Self::Down => formatter.write_str("down"),
            Self::Home => formatter.write_str("home"),
            Self::End => formatter.write_str("end"),
            Self::PageUp => formatter.write_str("page-up"),
            Self::PageDown => formatter.write_str("page-down"),
            Self::F(index) => write!(formatter, "f{index}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseKeySpecError {
    Empty,
    MissingKey,
    DuplicateModifier(String),
    UnsupportedKey(String),
}

impl fmt::Display for ParseKeySpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("key spec is empty"),
            Self::MissingKey => formatter.write_str("key spec is missing a key"),
            Self::DuplicateModifier(modifier) => {
                write!(formatter, "key spec repeats modifier '{modifier}'")
            }
            Self::UnsupportedKey(key) => write!(formatter, "unsupported key '{key}'"),
        }
    }
}

impl Error for ParseKeySpecError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyContext {
    Global,
    ChatBlocked,
    ChatInput,
    AutocompleteMention,
    AutocompleteSlash,
    AutocompleteSubagent,
    InlinePermission,
    InlineQuestion,
}

impl KeyContext {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::ChatBlocked => "chat_blocked",
            Self::ChatInput => "chat_input",
            Self::AutocompleteMention => "autocomplete_mention",
            Self::AutocompleteSlash => "autocomplete_slash",
            Self::AutocompleteSubagent => "autocomplete_subagent",
            Self::InlinePermission => "inline_permission",
            Self::InlineQuestion => "inline_question",
        }
    }

    pub fn resolution_chain(self) -> &'static [KeyContext] {
        match self {
            Self::Global => &[Self::Global],
            Self::ChatBlocked => &[Self::ChatBlocked, Self::Global],
            Self::ChatInput => &[Self::ChatInput, Self::Global],
            Self::AutocompleteMention => &[Self::AutocompleteMention, Self::Global],
            Self::AutocompleteSlash => &[Self::AutocompleteSlash, Self::Global],
            Self::AutocompleteSubagent => &[Self::AutocompleteSubagent, Self::Global],
            Self::InlinePermission => &[Self::InlinePermission, Self::Global],
            Self::InlineQuestion => &[Self::InlineQuestion, Self::Global],
        }
    }
}

impl fmt::Display for KeyContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyAction {
    App(AppAction),
    Input(InputAction),
    Autocomplete(AutocompleteAction),
    Interaction(InteractionAction),
    Terminal(TerminalAction),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AppAction {
    Quit,
    ClearInputOrQuit,
    Redraw,
    CancelTurn,
    SubmitInput,
    FocusPromptOrAcceptSuggestion,
    CycleMode,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InputAction {
    MoveCharLeft,
    MoveCharRight,
    MoveWordLeft,
    MoveWordRight,
    MoveLineStart,
    MoveLineEnd,
    MoveUp,
    MoveDown,
    DeleteCharBefore,
    DeleteCharAfter,
    DeleteWordBefore,
    DeleteWordAfter,
    KillLineStart,
    KillLineEnd,
    Yank,
    Undo,
    Redo,
    InsertNewline,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AutocompleteAction {
    MovePrevious,
    MoveNext,
    Confirm,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionAction {
    MovePrevious,
    MoveNext,
    MoveStart,
    MoveEnd,
    Confirm,
    Cancel,
    FocusNext,
    ToggleSelection,
    ToggleNotes,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalAction {
    Suspend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyActionDescriptor {
    pub action: KeyAction,
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub default_contexts: &'static [KeyContext],
}

impl KeyAction {
    pub fn id(self) -> &'static str {
        self.descriptor().id
    }

    pub fn label(self) -> &'static str {
        self.descriptor().label
    }

    pub fn description(self) -> &'static str {
        self.descriptor().description
    }

    pub fn descriptor(self) -> &'static KeyActionDescriptor {
        action_descriptor(self).unwrap_or_else(|| {
            unreachable!("key action {self:?} is missing from the action catalog")
        })
    }

    pub fn from_id(id: &str) -> Option<Self> {
        action_catalog()
            .iter()
            .find(|descriptor| descriptor.id == id)
            .map(|descriptor| descriptor.action)
    }
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyBindingSource {
    Default,
    Config,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    pub context: KeyContext,
    pub spec: KeySpec,
    pub action: KeyAction,
    pub source: KeyBindingSource,
}

impl KeyBinding {
    pub fn new(
        context: KeyContext,
        spec: KeySpec,
        action: KeyAction,
        source: KeyBindingSource,
    ) -> Self {
        Self { context, spec, action, source }
    }

    fn default(context: KeyContext, spec: KeySpec, action: KeyAction) -> Self {
        Self::new(context, spec, action, KeyBindingSource::Default)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedKeyAction {
    pub action: KeyAction,
    pub requested_context: KeyContext,
    pub matched_context: KeyContext,
    pub source: KeyBindingSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedHelpBinding {
    pub spec: KeySpec,
    pub action: KeyAction,
    pub requested_context: KeyContext,
    pub matched_context: KeyContext,
    pub source: KeyBindingSource,
}

impl ResolvedHelpBinding {
    pub fn descriptor(&self) -> &'static KeyActionDescriptor {
        self.action.descriptor()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeymapBuildError {
    DuplicateBinding {
        context: KeyContext,
        spec: KeySpec,
        existing_action: KeyAction,
        duplicate_action: KeyAction,
    },
    UncataloguedAction {
        action: KeyAction,
    },
    ShadowedGlobalBinding {
        context: KeyContext,
        spec: KeySpec,
        context_action: KeyAction,
        global_action: KeyAction,
    },
    ProtectedGlobalActionConflict {
        context: KeyContext,
        spec: KeySpec,
        action: KeyAction,
    },
    PlatformInvalidBinding {
        context: KeyContext,
        spec: KeySpec,
        reason: &'static str,
    },
    UnsupportedDefaultBinding {
        context: KeyContext,
        spec: KeySpec,
        action: KeyAction,
        reason: &'static str,
    },
}

impl fmt::Display for KeymapBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateBinding { context, spec, existing_action, duplicate_action } => write!(
                formatter,
                "duplicate key binding for {spec} in {context}: {existing_action:?} and {duplicate_action:?}"
            ),
            Self::UncataloguedAction { action } => {
                write!(formatter, "key binding references uncatalogued action {action:?}")
            }
            Self::ShadowedGlobalBinding { context, spec, context_action, global_action } => write!(
                formatter,
                "key binding for {spec} in {context} shadows global binding: {context_action:?} and {global_action:?}"
            ),
            Self::ProtectedGlobalActionConflict { context, spec, action } => write!(
                formatter,
                "key binding for {spec} in {context} conflicts with protected global action {}",
                action.id()
            ),
            Self::PlatformInvalidBinding { context, spec, reason } => {
                write!(
                    formatter,
                    "key binding for {spec} in {context} is invalid on this platform: {reason}"
                )
            }
            Self::UnsupportedDefaultBinding { context, spec, action, reason } => write!(
                formatter,
                "default key binding for {spec} in {context} uses unsupported action {}: {reason}",
                action.id()
            ),
        }
    }
}

impl Error for KeymapBuildError {}

#[derive(Clone, Debug)]
pub struct ResolvedKeymap {
    bindings: Vec<KeyBinding>,
    actions: HashMap<KeyBindingLookup, ResolvedBinding>,
}

impl ResolvedKeymap {
    pub fn from_bindings(
        bindings: impl IntoIterator<Item = KeyBinding>,
    ) -> Result<Self, KeymapBuildError> {
        let mut ordered_bindings = Vec::new();
        let mut actions: HashMap<KeyBindingLookup, ResolvedBinding> = HashMap::new();
        for binding in bindings {
            if action_descriptor(binding.action).is_none() {
                return Err(KeymapBuildError::UncataloguedAction { action: binding.action });
            }
            if let Some(reason) = platform_invalid_binding_reason(&binding) {
                return Err(KeymapBuildError::PlatformInvalidBinding {
                    context: binding.context,
                    spec: binding.spec,
                    reason,
                });
            }
            let lookup = KeyBindingLookup { context: binding.context, spec: binding.spec.clone() };
            if let Some(existing_binding) = actions.get(&lookup).copied() {
                return Err(KeymapBuildError::DuplicateBinding {
                    context: binding.context,
                    spec: binding.spec,
                    existing_action: existing_binding.action,
                    duplicate_action: binding.action,
                });
            }
            actions
                .insert(lookup, ResolvedBinding { action: binding.action, source: binding.source });
            ordered_bindings.push(binding);
        }
        validate_resolution_conflicts(&actions)?;
        Ok(Self { bindings: ordered_bindings, actions })
    }

    pub fn defaults() -> Self {
        match Self::validate_defaults() {
            Ok(keymap) => keymap,
            Err(error) => unreachable!("default keymap should validate: {error}"),
        }
    }

    pub fn validate_defaults() -> Result<Self, KeymapBuildError> {
        Self::from_bindings(default_bindings())
    }

    pub fn action_for(&self, context: KeyContext, spec: &KeySpec) -> Option<KeyAction> {
        let lookup = KeyBindingLookup { context, spec: spec.clone() };
        self.actions.get(&lookup).map(|binding| binding.action)
    }

    pub fn action_for_event(&self, context: KeyContext, key: KeyEvent) -> Option<KeyAction> {
        let spec = KeySpec::from_event(key)?;
        self.action_for(context, &spec)
    }

    pub fn resolve(&self, context: KeyContext, spec: &KeySpec) -> Option<ResolvedKeyAction> {
        for matched_context in context.resolution_chain() {
            let lookup = KeyBindingLookup { context: *matched_context, spec: spec.clone() };
            if let Some(binding) = self.actions.get(&lookup).copied() {
                return Some(ResolvedKeyAction {
                    action: binding.action,
                    requested_context: context,
                    matched_context: *matched_context,
                    source: binding.source,
                });
            }
        }
        None
    }

    pub fn resolve_event(&self, context: KeyContext, key: KeyEvent) -> Option<ResolvedKeyAction> {
        let spec = KeySpec::from_event(key)?;
        self.resolve(context, &spec)
    }

    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    pub fn help_bindings_for_context(&self, context: KeyContext) -> Vec<ResolvedHelpBinding> {
        let mut seen = HashSet::new();
        self.bindings
            .iter()
            .filter_map(|binding| {
                let resolved = self.resolve(context, &binding.spec)?;
                if resolved.action != binding.action || resolved.matched_context != binding.context
                {
                    return None;
                }
                let key = (binding.spec.clone(), binding.action, binding.context);
                if !seen.insert(key) {
                    return None;
                }
                Some(ResolvedHelpBinding {
                    spec: binding.spec.clone(),
                    action: binding.action,
                    requested_context: context,
                    matched_context: binding.context,
                    source: binding.source,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResolvedBinding {
    action: KeyAction,
    source: KeyBindingSource,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct KeyBindingLookup {
    context: KeyContext,
    spec: KeySpec,
}

fn validate_resolution_conflicts(
    actions: &HashMap<KeyBindingLookup, ResolvedBinding>,
) -> Result<(), KeymapBuildError> {
    for (lookup, binding) in actions {
        if lookup.context == KeyContext::Global
            || !lookup.context.resolution_chain().contains(&KeyContext::Global)
        {
            continue;
        }

        let global_lookup =
            KeyBindingLookup { context: KeyContext::Global, spec: lookup.spec.clone() };
        let Some(global_binding) = actions.get(&global_lookup).copied() else {
            continue;
        };

        if is_protected_global_action(global_binding.action) {
            return Err(KeymapBuildError::ProtectedGlobalActionConflict {
                context: lookup.context,
                spec: lookup.spec.clone(),
                action: global_binding.action,
            });
        }

        return Err(KeymapBuildError::ShadowedGlobalBinding {
            context: lookup.context,
            spec: lookup.spec.clone(),
            context_action: binding.action,
            global_action: global_binding.action,
        });
    }

    Ok(())
}

fn is_protected_global_action(action: KeyAction) -> bool {
    matches!(
        action,
        KeyAction::App(AppAction::Quit | AppAction::Redraw)
            | KeyAction::Terminal(TerminalAction::Suspend)
    )
}

fn platform_invalid_binding_reason(binding: &KeyBinding) -> Option<&'static str> {
    if binding.spec.modifiers().contains(KeyModifiers::SUPER) && !cfg!(target_os = "macos") {
        return Some("cmd/super bindings are only supported on macOS");
    }
    if cfg!(target_os = "windows")
        && binding.action == KeyAction::Terminal(TerminalAction::Suspend)
        && is_ctrl_z(&binding.spec)
    {
        return Some("ctrl-z suspend is not supported on Windows");
    }
    None
}

fn is_ctrl_z(spec: &KeySpec) -> bool {
    spec.code() == KeyCodeSpec::Char('z') && spec.modifiers() == KeyModifiers::CONTROL
}

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

fn normalized_char_code(ch: char, modifiers: &mut KeyModifiers) -> KeyCodeSpec {
    if let Some(alpha) = control_char_to_alpha(ch)
        && !modifiers.contains(KeyModifiers::ALT)
    {
        modifiers.insert(KeyModifiers::CONTROL);
        return KeyCodeSpec::Char(alpha);
    }
    KeyCodeSpec::Char(ch)
}

fn control_char_to_alpha(ch: char) -> Option<char> {
    let value = u32::from(ch);
    if (1..=26).contains(&value) { char::from_u32(value + u32::from(b'a') - 1) } else { None }
}

fn should_canonicalize_char(ch: char, modifiers: KeyModifiers) -> bool {
    ch.is_ascii_alphabetic()
        && modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::SUPER,
        )
}

fn parse_modifier(token: &str) -> Option<KeyModifiers> {
    match token {
        "ctrl" | "control" => Some(KeyModifiers::CONTROL),
        "alt" | "option" => Some(KeyModifiers::ALT),
        "shift" => Some(KeyModifiers::SHIFT),
        "cmd" | "command" | "super" => Some(KeyModifiers::SUPER),
        _ => None,
    }
}

fn parse_key_code(key_name: &str) -> Option<KeyCodeSpec> {
    match key_name {
        "enter" | "return" => Some(KeyCodeSpec::Enter),
        "esc" | "escape" => Some(KeyCodeSpec::Esc),
        "backspace" | "bs" => Some(KeyCodeSpec::Backspace),
        "delete" | "del" => Some(KeyCodeSpec::Delete),
        "insert" | "ins" => Some(KeyCodeSpec::Insert),
        "tab" => Some(KeyCodeSpec::Tab),
        "left" => Some(KeyCodeSpec::Left),
        "right" => Some(KeyCodeSpec::Right),
        "up" => Some(KeyCodeSpec::Up),
        "down" => Some(KeyCodeSpec::Down),
        "home" => Some(KeyCodeSpec::Home),
        "end" => Some(KeyCodeSpec::End),
        "pageup" | "page-up" => Some(KeyCodeSpec::PageUp),
        "pagedown" | "page-down" => Some(KeyCodeSpec::PageDown),
        "space" => Some(KeyCodeSpec::Char(' ')),
        _ => parse_function_key(key_name).or_else(|| parse_single_char_key(key_name)),
    }
}

fn parse_function_key(key_name: &str) -> Option<KeyCodeSpec> {
    let digits = key_name.strip_prefix('f')?;
    let index = digits.parse::<u8>().ok()?;
    (index > 0).then_some(KeyCodeSpec::F(index))
}

fn parse_single_char_key(key_name: &str) -> Option<KeyCodeSpec> {
    let mut chars = key_name.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(KeyCodeSpec::Char(ch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn key_spec_from_event_accepts_standard_ctrl_v_encoding() {
        let key = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);

        assert_eq!(KeySpec::from_event(key), Some("ctrl-v".parse().expect("parse key")));
    }

    #[test]
    fn key_spec_from_event_accepts_raw_control_character_encoding() {
        let key = KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::NONE);

        assert_eq!(KeySpec::from_event(key), Some("ctrl-v".parse().expect("parse key")));
    }

    #[test]
    fn key_spec_from_event_rejects_raw_control_character_with_alt_as_plain_ctrl() {
        let key = KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::ALT);

        assert_ne!(KeySpec::from_event(key), Some("ctrl-v".parse().expect("parse key")));
    }

    #[test]
    fn key_spec_matching_uses_exact_modifiers() {
        let spec: KeySpec = "ctrl-v".parse().expect("parse key");
        let ctrl_alt_v =
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL | KeyModifiers::ALT);

        assert!(!spec.matches_event(ctrl_alt_v));
    }

    #[test]
    fn key_spec_parse_and_display_are_canonical() {
        for (raw, canonical) in [
            ("ctrl-a", "ctrl-a"),
            ("alt-b", "alt-b"),
            ("shift-enter", "shift-enter"),
            ("cmd-z", "cmd-z"),
            ("esc", "esc"),
            ("backspace", "backspace"),
            ("delete", "delete"),
            ("ctrl-left", "ctrl-left"),
            ("option-f", "alt-f"),
            ("command-shift-z", "cmd-shift-z"),
        ] {
            let spec: KeySpec = raw.parse().expect("parse key");

            assert_eq!(spec.to_string(), canonical);
        }
    }

    #[test]
    fn resolved_keymap_resolves_binding_in_the_right_context() {
        let keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-a".parse().expect("parse key"),
            KeyAction::Input(InputAction::MoveLineStart),
            KeyBindingSource::Default,
        )])
        .expect("build keymap");

        assert_eq!(
            keymap.action_for(KeyContext::ChatInput, &"ctrl-a".parse().expect("parse key")),
            Some(KeyAction::Input(InputAction::MoveLineStart))
        );
    }

    #[test]
    fn resolved_keymap_resolves_global_fallback_with_metadata() {
        let keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::Global,
            "ctrl-q".parse().expect("parse key"),
            KeyAction::App(AppAction::Quit),
            KeyBindingSource::Default,
        )])
        .expect("build keymap");

        assert_eq!(
            keymap.resolve(KeyContext::ChatInput, &"ctrl-q".parse().expect("parse key")),
            Some(ResolvedKeyAction {
                action: KeyAction::App(AppAction::Quit),
                requested_context: KeyContext::ChatInput,
                matched_context: KeyContext::Global,
                source: KeyBindingSource::Default,
            })
        );
    }

    #[test]
    fn resolved_keymap_help_bindings_follow_resolution_chain() {
        let keymap = ResolvedKeymap::from_bindings([
            KeyBinding::new(
                KeyContext::Global,
                "ctrl-q".parse().expect("parse key"),
                KeyAction::App(AppAction::Quit),
                KeyBindingSource::Default,
            ),
            KeyBinding::new(
                KeyContext::ChatInput,
                "ctrl-x".parse().expect("parse key"),
                KeyAction::App(AppAction::SubmitInput),
                KeyBindingSource::Default,
            ),
        ])
        .expect("build keymap");

        let bindings = keymap.help_bindings_for_context(KeyContext::ChatInput);

        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].spec, "ctrl-q".parse().expect("parse key"));
        assert_eq!(bindings[0].matched_context, KeyContext::Global);
        assert_eq!(bindings[0].descriptor().label, "Quit");
        assert_eq!(bindings[1].spec, "ctrl-x".parse().expect("parse key"));
        assert_eq!(bindings[1].matched_context, KeyContext::ChatInput);
        assert_eq!(bindings[1].descriptor().label, "Send message");
    }

    #[test]
    fn resolved_keymap_does_not_resolve_binding_outside_resolution_chain() {
        let keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::ChatInput,
            "ctrl-a".parse().expect("parse key"),
            KeyAction::Input(InputAction::MoveLineStart),
            KeyBindingSource::Default,
        )])
        .expect("build keymap");

        assert_eq!(
            keymap.resolve(KeyContext::InlinePermission, &"ctrl-a".parse().expect("parse key")),
            None
        );
    }

    #[test]
    fn resolved_keymap_rejects_duplicate_binding_in_same_context() {
        let result = ResolvedKeymap::from_bindings([
            KeyBinding::new(
                KeyContext::ChatInput,
                "ctrl-a".parse().expect("parse key"),
                KeyAction::Input(InputAction::MoveLineStart),
                KeyBindingSource::Default,
            ),
            KeyBinding::new(
                KeyContext::ChatInput,
                "ctrl-a".parse().expect("parse key"),
                KeyAction::App(AppAction::Redraw),
                KeyBindingSource::Default,
            ),
        ]);

        assert!(matches!(result, Err(KeymapBuildError::DuplicateBinding { .. })));
    }

    #[test]
    fn resolved_keymap_rejects_shadowed_global_binding() {
        let result = ResolvedKeymap::from_bindings([
            KeyBinding::new(
                KeyContext::Global,
                "ctrl-g".parse().expect("parse key"),
                KeyAction::App(AppAction::CancelTurn),
                KeyBindingSource::Config,
            ),
            KeyBinding::new(
                KeyContext::ChatInput,
                "ctrl-g".parse().expect("parse key"),
                KeyAction::Input(InputAction::MoveLineStart),
                KeyBindingSource::Config,
            ),
        ]);

        assert!(matches!(result, Err(KeymapBuildError::ShadowedGlobalBinding { .. })));
    }

    #[test]
    fn resolved_keymap_rejects_protected_global_action_conflict() {
        let result = ResolvedKeymap::from_bindings([
            KeyBinding::new(
                KeyContext::Global,
                "ctrl-q".parse().expect("parse key"),
                KeyAction::App(AppAction::Quit),
                KeyBindingSource::Default,
            ),
            KeyBinding::new(
                KeyContext::ChatInput,
                "ctrl-q".parse().expect("parse key"),
                KeyAction::Input(InputAction::MoveLineStart),
                KeyBindingSource::Config,
            ),
        ]);

        assert!(matches!(result, Err(KeymapBuildError::ProtectedGlobalActionConflict { .. })));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn resolved_keymap_rejects_cmd_bindings_off_macos() {
        let result = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::ChatInput,
            "cmd-z".parse().expect("parse key"),
            KeyAction::Input(InputAction::Undo),
            KeyBindingSource::Config,
        )]);

        assert!(matches!(result, Err(KeymapBuildError::PlatformInvalidBinding { .. })));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolved_keymap_rejects_ctrl_z_suspend_on_windows() {
        let result = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::Global,
            "ctrl-z".parse().expect("parse key"),
            KeyAction::Terminal(TerminalAction::Suspend),
            KeyBindingSource::Config,
        )]);

        assert!(matches!(result, Err(KeymapBuildError::PlatformInvalidBinding { .. })));
    }

    #[test]
    fn action_catalog_has_unique_stable_ids() {
        let mut ids = HashSet::new();

        for descriptor in action_catalog() {
            assert!(!descriptor.id.is_empty(), "{descriptor:?}");
            assert!(!descriptor.label.is_empty(), "{descriptor:?}");
            assert!(!descriptor.description.is_empty(), "{descriptor:?}");
            assert!(!descriptor.default_contexts.is_empty(), "{descriptor:?}");
            assert!(ids.insert(descriptor.id), "duplicate action id {}", descriptor.id);
            assert_eq!(KeyAction::from_id(descriptor.id), Some(descriptor.action));
            assert_eq!(descriptor.action.id(), descriptor.id);
            assert_eq!(descriptor.action.label(), descriptor.label);
            assert_eq!(descriptor.action.description(), descriptor.description);
        }

        assert_eq!(KeyAction::from_id("input.missing"), None);
    }

    #[test]
    fn default_bindings_reference_catalogued_actions() {
        for binding in default_bindings() {
            let descriptor = action_descriptor(binding.action)
                .unwrap_or_else(|| panic!("missing descriptor for {:?}", binding.action));

            assert_eq!(descriptor.action, binding.action);
        }
    }

    #[test]
    fn default_keymap_contains_chat_input_readline_bindings() {
        let keymap = ResolvedKeymap::defaults();

        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            ),
            Some(KeyAction::Input(InputAction::Yank))
        );
        assert_eq!(
            keymap.action_for_event(
                KeyContext::ChatInput,
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            ),
            Some(KeyAction::Input(InputAction::MoveLineStart))
        );
    }

    #[test]
    fn default_keymap_does_not_bind_permission_ctrl_shortcuts() {
        let keymap = ResolvedKeymap::defaults();

        for key in [
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(keymap.action_for_event(KeyContext::InlinePermission, key), None);
        }
    }

    #[test]
    fn default_keymap_resolves_enter_by_context() {
        let keymap = ResolvedKeymap::defaults();
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            keymap.action_for_event(KeyContext::ChatInput, enter),
            Some(KeyAction::App(AppAction::SubmitInput))
        );
        assert_eq!(
            keymap.action_for_event(KeyContext::InlinePermission, enter),
            Some(KeyAction::Interaction(InteractionAction::Confirm))
        );
        assert_eq!(
            keymap.action_for_event(
                KeyContext::InlineQuestion,
                KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            ),
            Some(KeyAction::Interaction(InteractionAction::ToggleSelection))
        );
        assert_eq!(
            keymap.action_for_event(
                KeyContext::InlineQuestion,
                KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            ),
            Some(KeyAction::Interaction(InteractionAction::MoveStart))
        );
        assert_eq!(
            keymap.action_for_event(
                KeyContext::InlineQuestion,
                KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            ),
            Some(KeyAction::Interaction(InteractionAction::ToggleNotes))
        );
    }

    #[test]
    fn default_keymap_uses_platform_history_bindings() {
        let keymap = ResolvedKeymap::defaults();

        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                keymap.action_for_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER),
                ),
                Some(KeyAction::Input(InputAction::Undo))
            );
            assert_eq!(
                keymap.action_for_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER | KeyModifiers::SHIFT),
                ),
                Some(KeyAction::Input(InputAction::Redo))
            );
        }

        #[cfg(target_os = "windows")]
        {
            assert_eq!(
                keymap.action_for_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
                ),
                Some(KeyAction::Input(InputAction::Undo))
            );
            assert_eq!(
                keymap.action_for_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL | KeyModifiers::SHIFT,),
                ),
                Some(KeyAction::Input(InputAction::Redo))
            );
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            assert_eq!(
                keymap.action_for_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('_'), KeyModifiers::CONTROL),
                ),
                Some(KeyAction::Input(InputAction::Undo))
            );
            assert_eq!(
                keymap.action_for_event(
                    KeyContext::ChatInput,
                    KeyEvent::new(KeyCode::Char('/'), KeyModifiers::CONTROL),
                ),
                Some(KeyAction::Input(InputAction::Undo))
            );
        }

        #[cfg(unix)]
        {
            assert_eq!(
                keymap
                    .resolve_event(
                        KeyContext::ChatInput,
                        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
                    )
                    .map(|resolved| resolved.action),
                Some(KeyAction::Terminal(TerminalAction::Suspend))
            );
        }

        #[cfg(target_os = "windows")]
        {
            assert_ne!(
                keymap
                    .resolve_event(
                        KeyContext::ChatInput,
                        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
                    )
                    .map(|resolved| resolved.action),
                Some(KeyAction::Terminal(TerminalAction::Suspend))
            );
        }
    }

    #[test]
    fn default_keymap_bindings_are_conflict_free() {
        ResolvedKeymap::validate_defaults().expect("default keymap should be conflict-free");
    }
}
