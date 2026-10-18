// SPDX-License-Identifier: Apache-2.0

use super::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginsViewTab {
    #[default]
    Installed,
    Plugins,
    Marketplace,
    Session,
}

impl PluginsViewTab {
    pub const ALL: [Self; 4] = [Self::Installed, Self::Plugins, Self::Marketplace, Self::Session];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Plugins => "Plugins",
            Self::Marketplace => "Marketplace",
            Self::Session => "Session",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Installed => Self::Plugins,
            Self::Plugins => Self::Marketplace,
            Self::Marketplace => Self::Session,
            Self::Session => Self::Installed,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Installed => Self::Session,
            Self::Session => Self::Marketplace,
            Self::Plugins => Self::Installed,
            Self::Marketplace => Self::Plugins,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPluginEntry {
    pub id: String,
    pub version: Option<String>,
    pub scope: String,
    pub enabled: bool,
    pub installed_at: Option<String>,
    pub last_updated: Option<String>,
    pub project_path: Option<String>,
    pub mcp_server_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceEntry {
    pub plugin_id: String,
    pub name: String,
    pub description: Option<String>,
    pub marketplace_name: Option<String>,
    pub version: Option<String>,
    pub install_count: Option<u64>,
    pub source: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceSourceEntry {
    pub name: String,
    pub source: Option<String>,
    pub repo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsInventorySnapshot {
    pub installed: Vec<InstalledPluginEntry>,
    pub marketplace: Vec<MarketplaceEntry>,
    pub marketplaces: Vec<MarketplaceSourceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginsState {
    /// Confirmed session attachments from the SDK, separate from installed disk records.
    pub session_plugins: Vec<crate::agent::types::SessionPlugin>,
    pub session_selected_index: usize,
    pub pending_reload: Option<PendingPluginReload>,
    pub active_tab: PluginsViewTab,
    pub search_focused: bool,
    pub installed_search_query: String,
    pub plugins_search_query: String,
    pub installed_selected_index: usize,
    pub plugins_selected_index: usize,
    pub marketplace_selected_index: usize,
    pub installed: Vec<InstalledPluginEntry>,
    pub marketplace: Vec<MarketplaceEntry>,
    pub marketplaces: Vec<MarketplaceSourceEntry>,
    pub loading: bool,
    pub status_message: Option<String>,
    pub last_error: Option<String>,
    pub last_inventory_refresh_at: Option<Instant>,
    pub claude_path: Option<PathBuf>,
    pub runtime_reload_after_refresh: bool,
    pub pending_runtime_reload_success_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginsCliActionSuccess {
    pub snapshot: PluginsInventorySnapshot,
    pub message: String,
    pub claude_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPluginReload {
    pub session_id: String,
    pub request_id: String,
    pub folders: Vec<String>,
    pub held: bool,
}

impl PluginsState {
    #[must_use]
    pub fn selected_index_for(&self, tab: PluginsViewTab) -> usize {
        match tab {
            PluginsViewTab::Installed => self.installed_selected_index,
            PluginsViewTab::Plugins => self.plugins_selected_index,
            PluginsViewTab::Marketplace => self.marketplace_selected_index,
            PluginsViewTab::Session => self.session_selected_index,
        }
    }

    pub fn set_selected_index_for(&mut self, tab: PluginsViewTab, index: usize) {
        match tab {
            PluginsViewTab::Installed => self.installed_selected_index = index,
            PluginsViewTab::Plugins => self.plugins_selected_index = index,
            PluginsViewTab::Marketplace => self.marketplace_selected_index = index,
            PluginsViewTab::Session => self.session_selected_index = index,
        }
    }

    pub fn clear_feedback(&mut self) {
        self.status_message = None;
        self.last_error = None;
    }

    #[must_use]
    pub fn search_query_for(&self, tab: PluginsViewTab) -> &str {
        match tab {
            PluginsViewTab::Installed => &self.installed_search_query,
            PluginsViewTab::Plugins => &self.plugins_search_query,
            PluginsViewTab::Marketplace | PluginsViewTab::Session => "",
        }
    }

    pub fn active_search_query_mut(&mut self) -> Option<&mut String> {
        match self.active_tab {
            PluginsViewTab::Installed => Some(&mut self.installed_search_query),
            PluginsViewTab::Plugins => Some(&mut self.plugins_search_query),
            PluginsViewTab::Marketplace | PluginsViewTab::Session => None,
        }
    }
}
