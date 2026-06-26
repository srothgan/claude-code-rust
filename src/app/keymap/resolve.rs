// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crossterm::event::{KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};

use super::action::{AppAction, KeyAction, TerminalAction};
use super::binding::{
    KeyBinding, KeyBindingSource, KeyContext, KeymapBuildError, ResolvedHelpBinding,
    ResolvedKeyAction,
};
use super::catalog::action_descriptor;
use super::defaults::default_bindings;
use super::spec::{KeyCodeSpec, KeySpec};
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
