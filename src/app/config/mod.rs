// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

mod controller;
mod edit;
mod help;
mod mcp;
mod mcp_edit;
mod overlays;
mod service;
mod state;
mod status;
mod tabs;

pub use controller::{handle_key, handle_paste, open};
pub(crate) use controller::{open_tab, refresh_runtime_tabs_for_session_change};
pub(crate) use mcp::{
    McpAuthRedirectOverlayState, McpDetailsOverlayState, McpElicitationOverlayState,
    apply_mcp_config_remove_failure, apply_mcp_config_remove_success,
    apply_pending_dynamic_mcp_removal_confirmation,
    apply_removed_config_mcp_server_confirmation_failures, available_mcp_actions,
    filter_removed_config_mcp_servers, filter_stale_plugin_mcp_servers,
    handle_mcp_elicitation_completed, handle_mcp_elicitation_response_queued,
    handle_mcp_operation_error, handle_mcp_set_servers_result, is_mcp_action_available,
    mcp_server_owner_summary, open_url_in_browser,
    pending_dynamic_mcp_removal_confirmation_from_snapshot, present_mcp_auth_redirect,
    present_mcp_elicitation_request, reconcile_removed_config_mcp_server_guards,
    reconcile_stale_plugin_mcp_servers, refresh_mcp_snapshot,
};
// Used by the binary UI target, but not by the library target in isolation.
#[allow(unused_imports)]
pub(crate) use mcp::McpCallbackUrlOverlayState;
pub use overlays::*;
pub(crate) use service::{apply_settings_result, request_settings};
pub use state::{ConfigState, PendingSessionTitleChangeKind, PendingSessionTitleChangeState};
pub use tabs::{ConfigHelpSection, ConfigTab};

mod prelude {
    pub(super) use super::overlays::*;
    pub(super) use super::status::request_status_snapshot_if_needed;
    pub(super) use super::tabs::{ConfigHelpSection, ConfigTab};
    pub(super) use super::{edit, help, mcp};
    pub(super) use crate::app::App;
    pub(super) use crate::app::dialog::DialogState;
    pub(super) use crate::app::view::{self, FullscreenView, SurfaceMode};
    pub(super) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    pub(super) use serde_json::Value;
}

#[cfg(test)]
mod tests;
