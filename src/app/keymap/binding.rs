// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use std::error::Error;
use std::fmt;

use super::action::KeyAction;
use super::catalog::KeyActionDescriptor;
use super::spec::KeySpec;
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

    pub(super) fn default(context: KeyContext, spec: KeySpec, action: KeyAction) -> Self {
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
