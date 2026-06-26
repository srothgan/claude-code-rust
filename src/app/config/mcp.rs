// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

mod actions;
mod auth;
mod elicitation;
mod errors;
mod overlays;
mod ownership;
mod removal;
mod snapshot;
mod types;

pub(crate) use actions::{
    authenticate_mcp_server, available_mcp_actions, clear_mcp_server_auth, handle_mcp_key,
    is_mcp_action_available, reconnect_mcp_server, set_mcp_server_enabled,
};
pub(crate) use auth::{
    copy_text_to_clipboard, present_mcp_auth_redirect, submit_mcp_oauth_callback_url,
};
pub(crate) use elicitation::{
    handle_mcp_elicitation_completed, present_mcp_elicitation_request,
    send_mcp_elicitation_response,
};
pub(crate) use errors::handle_mcp_operation_error;
pub(crate) use overlays::{
    McpAuthRedirectOverlayState, McpCallbackUrlOverlayState, McpDetailsOverlayState,
    McpElicitationOverlayState, open_mcp_server_details,
};
pub(crate) use ownership::{mcp_config_removal_scope, mcp_server_owner_summary};
pub(crate) use removal::{
    apply_mcp_config_remove_failure, apply_mcp_config_remove_success,
    apply_pending_dynamic_mcp_removal_confirmation, handle_mcp_set_servers_result,
    pending_dynamic_mcp_removal_confirmation_from_snapshot, remove_mcp_server_from_config,
};
#[allow(unused_imports)]
pub(crate) use snapshot::{
    apply_removed_config_mcp_server_confirmation_failures, filter_removed_config_mcp_servers,
    filter_stale_plugin_mcp_servers, reconcile_removed_config_mcp_server_guards,
    reconcile_stale_plugin_mcp_servers, refresh_mcp_snapshot, refresh_mcp_snapshot_if_needed,
    request_mcp_snapshot,
};
#[allow(unused_imports)]
pub(crate) use types::{
    McpConfigRemoveConfirmationFailure, McpConfigScope, McpServerActionKind, McpServerOwnership,
    PendingDynamicMcpRemovalConfirmation,
};

#[allow(unused_imports)]
mod prelude {
    pub(super) use super::super::{ConfigOverlayState, ConfigState, ConfigTab};
    pub(super) use super::actions::{available_mcp_actions, is_mcp_action_available};
    pub(super) use super::auth::open_url_in_browser;
    pub(super) use super::overlays::{
        McpAuthRedirectOverlayState, McpCallbackUrlOverlayState, McpDetailsOverlayState,
        McpElicitationOverlayState, open_selected_mcp_server_details,
    };
    pub(super) use super::ownership::{
        is_mcp_config_removal_available, mcp_config_removal_scope, mcp_server_ownership,
    };
    pub(super) use super::removal::{
        apply_mcp_config_remove_failure, apply_mcp_config_remove_success_state,
        is_removed_config_mcp_server_suppressed, mcp_server_matches_removed_key,
        mcp_server_name_eq, mcp_snapshot_source_label, normalize_mcp_config_scope_key,
        normalized_removed_config_key,
    };
    pub(super) use super::snapshot::refresh_mcp_snapshot;
    pub(super) use super::types::*;
    pub(super) use crate::agent::{events::ClientEvent, model, types};
    pub(super) use crate::app::App;
    pub(super) use crate::app::plugins::InstalledPluginEntry;
    pub(super) use crate::app::state::types::{RemovedMcpServerGuard, RemovedMcpServerKey};
    pub(super) use crate::app::view::{self, FullscreenView};
    pub(super) use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    pub(super) use std::collections::BTreeMap;
}
