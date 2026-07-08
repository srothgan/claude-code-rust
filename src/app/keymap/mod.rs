// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod action;
mod binding;
mod catalog;
mod defaults;
mod parse;
mod resolve;
mod spec;

pub use action::{
    AppAction, AutocompleteAction, InputAction, InteractionAction, KeyAction, TerminalAction,
};
pub use binding::{
    KeyBinding, KeyBindingSource, KeyContext, KeymapBuildError, ResolvedHelpBinding,
    ResolvedKeyAction,
};
pub use catalog::{KeyActionDescriptor, action_catalog, action_descriptor};
pub use defaults::default_bindings;
pub use parse::ParseKeySpecError;
pub use resolve::ResolvedKeymap;
pub use spec::{KeyCodeSpec, KeySpec};

#[cfg(test)]
mod tests;
