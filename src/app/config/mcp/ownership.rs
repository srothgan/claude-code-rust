// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

pub(crate) fn mcp_server_ownership<'a>(
    app: &'a App,
    server: &'a crate::agent::model::McpServerStatus,
) -> McpServerOwnership<'a> {
    if crate::app::plugins::is_plugin_mcp_runtime_server_name(&server.name) {
        return crate::app::plugins::installed_mcp_plugin_for_runtime_server(app, &server.name)
            .map_or(McpServerOwnership::PluginOwnedUnknown, McpServerOwnership::PluginOwned);
    }

    match server.scope.as_deref().and_then(McpConfigScope::from_status_scope) {
        Some(McpConfigScope::User) => McpServerOwnership::Persisted(McpConfigScope::User),
        Some(McpConfigScope::Local) => McpServerOwnership::Persisted(McpConfigScope::Local),
        Some(McpConfigScope::Project) => McpServerOwnership::Persisted(McpConfigScope::Project),
        Some(McpConfigScope::Dynamic) => McpServerOwnership::SdkDynamic,
        None => McpServerOwnership::RuntimeOnly,
    }
}

#[must_use]
pub(crate) fn mcp_config_removal_scope(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
) -> Option<McpConfigScope> {
    match mcp_server_ownership(app, server) {
        McpServerOwnership::Persisted(scope) => Some(scope),
        McpServerOwnership::SdkDynamic => Some(McpConfigScope::Dynamic),
        McpServerOwnership::PluginOwned(_)
        | McpServerOwnership::PluginOwnedUnknown
        | McpServerOwnership::RuntimeOnly => None,
    }
}

pub(crate) fn mcp_server_owner_summary(
    app: &App,
    server: &crate::agent::model::McpServerStatus,
) -> Option<String> {
    match mcp_server_ownership(app, server) {
        McpServerOwnership::PluginOwned(entry) => {
            Some(format!("Managed by plugin: {}", crate::app::plugins::display_label(&entry.id)))
        }
        McpServerOwnership::PluginOwnedUnknown => Some(
            "Managed by a plugin. Refresh plugin inventory from the Plugins tab to manage it here."
                .to_owned(),
        ),
        McpServerOwnership::Persisted(_)
        | McpServerOwnership::SdkDynamic
        | McpServerOwnership::RuntimeOnly => None,
    }
}

pub(super) fn is_mcp_config_removal_available(
    app: &App,
    server_name: &str,
    scope: McpConfigScope,
) -> bool {
    app.mcp
        .servers
        .iter()
        .find(|server| mcp_server_name_eq(&server.name, server_name))
        .is_some_and(|server| mcp_config_removal_scope(app, server) == Some(scope))
}
