// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDetailsOverlayState {
    pub server_name: String,
    pub selected_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCallbackUrlOverlayState {
    pub server_name: String,
    pub draft: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpElicitationOverlayState {
    pub request: crate::agent::types::ElicitationRequest,
    pub selected_index: usize,
    pub browser_opened: bool,
    pub browser_open_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthRedirectOverlayState {
    pub redirect: crate::agent::types::McpAuthRedirect,
    pub selected_index: usize,
    pub browser_opened: bool,
    pub browser_open_error: Option<String>,
}

impl ConfigState {
    #[must_use]
    pub fn mcp_details_overlay(&self) -> Option<&McpDetailsOverlayState> {
        if let Some(ConfigOverlayState::McpDetails(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_details_overlay_mut(&mut self) -> Option<&mut McpDetailsOverlayState> {
        if let Some(ConfigOverlayState::McpDetails(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn mcp_callback_url_overlay(&self) -> Option<&McpCallbackUrlOverlayState> {
        if let Some(ConfigOverlayState::McpCallbackUrl(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_callback_url_overlay_mut(&mut self) -> Option<&mut McpCallbackUrlOverlayState> {
        if let Some(ConfigOverlayState::McpCallbackUrl(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn mcp_elicitation_overlay(&self) -> Option<&McpElicitationOverlayState> {
        if let Some(ConfigOverlayState::McpElicitation(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_elicitation_overlay_mut(&mut self) -> Option<&mut McpElicitationOverlayState> {
        if let Some(ConfigOverlayState::McpElicitation(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    #[must_use]
    pub fn mcp_auth_redirect_overlay(&self) -> Option<&McpAuthRedirectOverlayState> {
        if let Some(ConfigOverlayState::McpAuthRedirect(overlay)) = &self.overlay {
            Some(overlay)
        } else {
            None
        }
    }

    pub fn mcp_auth_redirect_overlay_mut(&mut self) -> Option<&mut McpAuthRedirectOverlayState> {
        if let Some(ConfigOverlayState::McpAuthRedirect(overlay)) = &mut self.overlay {
            Some(overlay)
        } else {
            None
        }
    }
}

pub(super) fn open_selected_mcp_server_details(app: &mut App) {
    let Some(server_name) =
        app.mcp.servers.get(app.config.mcp_selected_server_index).map(|server| server.name.clone())
    else {
        return;
    };
    open_mcp_server_details(app, server_name, None);
}

pub(crate) fn open_mcp_server_details(
    app: &mut App,
    server_name: String,
    preferred_action: Option<McpServerActionKind>,
) {
    let selected_index =
        app.mcp.servers.iter().find(|server| server.name == server_name).map_or(0, |server| {
            preferred_action
                .and_then(|action| {
                    available_mcp_actions(app, server)
                        .iter()
                        .position(|candidate| *candidate == action)
                })
                .unwrap_or(0)
        });
    app.config.replace_overlay(ConfigOverlayState::McpDetails(McpDetailsOverlayState {
        server_name,
        selected_index,
    }));
}
