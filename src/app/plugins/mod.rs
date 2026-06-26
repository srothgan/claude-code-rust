// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod actions;
mod cli;
mod inventory;
mod keyboard;
mod overlays;
mod runtime_mcp;
mod selection;
mod text;
mod types;

pub(crate) use actions::{
    apply_cli_action_failure, apply_cli_action_success, apply_runtime_reload_failure,
    apply_runtime_reload_success, execute_confirmed_installed_plugin_action,
    execute_confirmed_marketplace_action,
};
pub(crate) use inventory::{
    apply_inventory_refresh_failure, apply_inventory_refresh_success,
    request_inventory_refresh_if_needed, reset_for_session_change,
};
pub(crate) use keyboard::{handle_key, handle_paste};
pub(crate) use overlays::{
    handle_add_marketplace_overlay_key, handle_installed_overlay_key,
    handle_marketplace_overlay_key, handle_plugin_install_overlay_key,
};
pub(crate) use runtime_mcp::{
    installed_mcp_plugin_for_runtime_server, is_plugin_mcp_runtime_server_name,
    is_stale_plugin_mcp_runtime_server, open_installed_actions_overlay_for_mcp_server,
};
pub(crate) use selection::{
    clamp_selection, filtered_marketplace_plugins, ordered_installed, search_enabled,
    visible_marketplaces,
};
pub(crate) use text::display_label;
pub use types::{
    InstalledPluginEntry, MarketplaceEntry, MarketplaceSourceEntry, PluginsCliActionSuccess,
    PluginsInventorySnapshot, PluginsState, PluginsViewTab,
};

#[allow(unused_imports)]
mod prelude {
    pub(super) use super::cli;
    pub(super) use super::types::{
        InstalledPluginEntry, MarketplaceEntry, MarketplaceSourceEntry, PluginsCliActionSuccess,
        PluginsInventorySnapshot, PluginsState, PluginsViewTab,
    };
    pub(super) use crate::agent::events::ClientEvent;
    pub(super) use crate::app::App;
    pub(super) use crate::app::config::{
        AddMarketplaceOverlayState, ConfigOverlayState, ConfirmationAction,
        ConfirmationOverlayState, InstalledPluginActionKind, InstalledPluginActionOverlayState,
        MarketplaceActionKind, MarketplaceActionsOverlayState, PluginInstallActionKind,
        PluginInstallOverlayState,
    };
    pub(super) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    pub(super) use serde_json::Value;
    pub(super) use std::path::PathBuf;
    pub(super) use std::time::{Duration, Instant};
}

#[cfg(test)]
mod tests;
