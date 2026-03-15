mod cli;

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use std::time::{Duration, Instant};

const INVENTORY_REFRESH_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillsViewTab {
    #[default]
    Installed,
    Skills,
    Marketplace,
}

impl SkillsViewTab {
    pub const ALL: [Self; 3] = [Self::Installed, Self::Skills, Self::Marketplace];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Skills => "Skills",
            Self::Marketplace => "Marketplace",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Installed => Self::Skills,
            Self::Skills => Self::Marketplace,
            Self::Marketplace => Self::Installed,
        }
    }

    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Installed => Self::Marketplace,
            Self::Skills => Self::Installed,
            Self::Marketplace => Self::Skills,
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
pub struct SkillsInventorySnapshot {
    pub installed: Vec<InstalledPluginEntry>,
    pub marketplace: Vec<MarketplaceEntry>,
    pub marketplaces: Vec<MarketplaceSourceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillsState {
    pub active_tab: SkillsViewTab,
    pub search_focused: bool,
    pub installed_search_query: String,
    pub skills_search_query: String,
    pub installed_selected_index: usize,
    pub skills_selected_index: usize,
    pub marketplace_selected_index: usize,
    pub installed: Vec<InstalledPluginEntry>,
    pub marketplace: Vec<MarketplaceEntry>,
    pub marketplaces: Vec<MarketplaceSourceEntry>,
    pub loading: bool,
    pub status_message: Option<String>,
    pub last_error: Option<String>,
    pub last_inventory_refresh_at: Option<Instant>,
}

impl SkillsState {
    #[must_use]
    pub fn selected_index_for(&self, tab: SkillsViewTab) -> usize {
        match tab {
            SkillsViewTab::Installed => self.installed_selected_index,
            SkillsViewTab::Skills => self.skills_selected_index,
            SkillsViewTab::Marketplace => self.marketplace_selected_index,
        }
    }

    pub fn set_selected_index_for(&mut self, tab: SkillsViewTab, index: usize) {
        match tab {
            SkillsViewTab::Installed => self.installed_selected_index = index,
            SkillsViewTab::Skills => self.skills_selected_index = index,
            SkillsViewTab::Marketplace => self.marketplace_selected_index = index,
        }
    }

    pub fn clear_feedback(&mut self) {
        self.status_message = None;
        self.last_error = None;
    }

    #[must_use]
    pub fn search_query_for(&self, tab: SkillsViewTab) -> &str {
        match tab {
            SkillsViewTab::Installed => &self.installed_search_query,
            SkillsViewTab::Skills => &self.skills_search_query,
            SkillsViewTab::Marketplace => "",
        }
    }

    pub fn active_search_query_mut(&mut self) -> Option<&mut String> {
        match self.active_tab {
            SkillsViewTab::Installed => Some(&mut self.installed_search_query),
            SkillsViewTab::Skills => Some(&mut self.skills_search_query),
            SkillsViewTab::Marketplace => None,
        }
    }
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Left, KeyModifiers::NONE) => {
            app.skills.active_tab = app.skills.active_tab.prev();
            app.skills.search_focused = false;
            clamp_selection(app);
            request_inventory_refresh_if_needed(app);
            true
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            app.skills.active_tab = app.skills.active_tab.next();
            app.skills.search_focused = false;
            clamp_selection(app);
            request_inventory_refresh_if_needed(app);
            true
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            if search_enabled(app.skills.active_tab)
                && !app.skills.search_focused
                && app.skills.selected_index_for(app.skills.active_tab) == 0
            {
                app.skills.search_focused = true;
            } else if !app.skills.search_focused {
                move_selection(app, -1);
            }
            true
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            if app.skills.search_focused {
                app.skills.search_focused = false;
            } else {
                move_selection(app, 1);
            }
            true
        }
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            if search_enabled(app.skills.active_tab)
                && app.skills.search_focused
                && let Some(query) = app.skills.active_search_query_mut()
                && query.pop().is_some()
            {
                reset_selection_for_active_tab(app);
            }
            true
        }
        (KeyCode::Delete, KeyModifiers::NONE) => {
            if search_enabled(app.skills.active_tab)
                && app.skills.search_focused
                && let Some(query) = app.skills.active_search_query_mut()
                && !query.is_empty()
            {
                query.clear();
                reset_selection_for_active_tab(app);
            }
            true
        }
        (KeyCode::Char(ch), modifiers)
            if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
        {
            if search_enabled(app.skills.active_tab)
                && app.skills.search_focused
                && let Some(query) = app.skills.active_search_query_mut()
            {
                query.push(ch);
                reset_selection_for_active_tab(app);
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn request_inventory_refresh_if_needed(app: &mut App) {
    if app.skills.loading {
        return;
    }
    if app
        .skills
        .last_inventory_refresh_at
        .is_some_and(|refreshed_at| refreshed_at.elapsed() < INVENTORY_REFRESH_TTL)
    {
        clamp_selection(app);
        return;
    }
    request_inventory_refresh(app);
}

pub(crate) fn request_inventory_refresh(app: &mut App) {
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    app.skills.loading = true;
    app.skills.clear_feedback();
    app.skills.status_message = Some("Refreshing skills inventory...".to_owned());
    app.needs_redraw = true;
    let event_tx = app.event_tx.clone();
    let cwd_raw = app.cwd_raw.clone();
    tokio::task::spawn_local(async move {
        match cli::refresh_inventory(&cwd_raw).await {
            Ok(snapshot) => {
                let _ = event_tx
                    .send(crate::agent::events::ClientEvent::SkillsInventoryUpdated { snapshot });
            }
            Err(message) => {
                let _ = event_tx
                    .send(crate::agent::events::ClientEvent::SkillsInventoryRefreshFailed(message));
            }
        }
    });
}

pub(crate) fn apply_inventory_refresh_success(app: &mut App, snapshot: SkillsInventorySnapshot) {
    app.skills.installed = snapshot.installed;
    app.skills.marketplace = snapshot.marketplace;
    app.skills.marketplaces = snapshot.marketplaces;
    app.skills.loading = false;
    app.skills.last_error = None;
    app.skills.last_inventory_refresh_at = Some(Instant::now());
    app.skills.status_message = Some("Skills inventory refreshed".to_owned());
    clamp_selection(app);
}

pub(crate) fn apply_inventory_refresh_failure(app: &mut App, message: String) {
    app.skills.loading = false;
    app.skills.status_message = None;
    app.skills.last_error = Some(message);
}

pub(crate) fn clamp_selection(app: &mut App) {
    let installed_len = filtered_installed(&app.skills).len();
    let skill_len = filtered_marketplace_skills(&app.skills).len();
    let marketplace_len = visible_marketplaces(&app.skills).len();
    app.skills.installed_selected_index =
        clamp_index(app.skills.installed_selected_index, installed_len);
    app.skills.skills_selected_index = clamp_index(app.skills.skills_selected_index, skill_len);
    app.skills.marketplace_selected_index =
        clamp_index(app.skills.marketplace_selected_index, marketplace_len);
}

#[must_use]
pub(crate) fn filtered_installed(state: &SkillsState) -> Vec<&InstalledPluginEntry> {
    state
        .installed
        .iter()
        .filter(|entry| {
            installed_entry_matches(entry, state.search_query_for(SkillsViewTab::Installed))
        })
        .collect()
}

#[must_use]
pub(crate) fn filtered_marketplace_skills(state: &SkillsState) -> Vec<&MarketplaceEntry> {
    state
        .marketplace
        .iter()
        .filter(|entry| {
            marketplace_skill_matches(entry, state.search_query_for(SkillsViewTab::Skills))
        })
        .collect()
}

#[must_use]
pub(crate) fn visible_marketplaces(state: &SkillsState) -> Vec<&MarketplaceSourceEntry> {
    state.marketplaces.iter().collect()
}

#[must_use]
pub(crate) fn display_label(raw: &str) -> String {
    let normalized = raw.replace('@', " from ").replace('-', " ");
    let mut result = String::with_capacity(normalized.len());
    let mut capitalize_next = true;

    for ch in normalized.chars() {
        if ch == ' ' {
            capitalize_next = true;
            result.push(ch);
            continue;
        }

        if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.extend(ch.to_lowercase());
        }
    }

    result
}

fn reset_selection_for_active_tab(app: &mut App) {
    app.skills.set_selected_index_for(app.skills.active_tab, 0);
    clamp_selection(app);
}

fn move_selection(app: &mut App, delta: isize) {
    let tab = app.skills.active_tab;
    let len = match tab {
        SkillsViewTab::Installed => filtered_installed(&app.skills).len(),
        SkillsViewTab::Skills => filtered_marketplace_skills(&app.skills).len(),
        SkillsViewTab::Marketplace => visible_marketplaces(&app.skills).len(),
    };
    if len == 0 {
        app.skills.set_selected_index_for(tab, 0);
        return;
    }
    let current = app.skills.selected_index_for(tab);
    let next = if delta.is_negative() {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta.cast_unsigned()).min(len.saturating_sub(1))
    };
    app.skills.set_selected_index_for(tab, next);
}

fn clamp_index(current: usize, len: usize) -> usize {
    if len == 0 { 0 } else { current.min(len.saturating_sub(1)) }
}

fn installed_entry_matches(entry: &InstalledPluginEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_ascii_lowercase();
    entry.id.to_ascii_lowercase().contains(&query)
        || entry.scope.to_ascii_lowercase().contains(&query)
        || entry
            .version
            .as_deref()
            .is_some_and(|version| version.to_ascii_lowercase().contains(&query))
}

fn marketplace_skill_matches(entry: &MarketplaceEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let query = query.to_ascii_lowercase();
    entry.plugin_id.to_ascii_lowercase().contains(&query)
        || entry.name.to_ascii_lowercase().contains(&query)
        || entry
            .description
            .as_deref()
            .is_some_and(|description| description.to_ascii_lowercase().contains(&query))
        || entry
            .marketplace_name
            .as_deref()
            .is_some_and(|marketplace| marketplace.to_ascii_lowercase().contains(&query))
        || entry
            .version
            .as_deref()
            .is_some_and(|version| version.to_ascii_lowercase().contains(&query))
}

#[must_use]
pub(crate) const fn search_enabled(tab: SkillsViewTab) -> bool {
    !matches!(tab, SkillsViewTab::Marketplace)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skills_tabs_wrap_in_both_directions() {
        assert_eq!(SkillsViewTab::Installed.prev(), SkillsViewTab::Marketplace);
        assert_eq!(SkillsViewTab::Marketplace.next(), SkillsViewTab::Installed);
    }

    #[test]
    fn recent_inventory_snapshot_skips_refresh() {
        let mut app = crate::app::App::test_default();
        app.skills.active_tab = SkillsViewTab::Installed;
        app.skills.last_inventory_refresh_at = Some(Instant::now());

        request_inventory_refresh_if_needed(&mut app);

        assert!(!app.skills.loading);
    }

    #[test]
    fn display_label_normalizes_skill_and_marketplace_names() {
        assert_eq!(
            display_label("frontend-design@claude-plugins-official"),
            "Frontend Design From Claude Plugins Official"
        );
        assert_eq!(display_label("claude-plugins-official"), "Claude Plugins Official");
    }

    #[test]
    fn filtered_marketplace_skills_match_on_name_description_and_marketplace() {
        let state = SkillsState {
            skills_search_query: "official".to_owned(),
            marketplace: vec![MarketplaceEntry {
                plugin_id: "frontend-design@claude-plugins-official".to_owned(),
                name: "frontend-design".to_owned(),
                description: Some("Create distinctive interfaces".to_owned()),
                marketplace_name: Some("claude-plugins-official".to_owned()),
                version: Some("1.0.0".to_owned()),
                install_count: Some(42),
                source: None,
            }],
            ..SkillsState::default()
        };

        assert_eq!(filtered_marketplace_skills(&state).len(), 1);
    }

    #[test]
    fn installed_and_skills_search_queries_are_independent() {
        let state = SkillsState {
            installed_search_query: "installed".to_owned(),
            skills_search_query: "skills".to_owned(),
            ..SkillsState::default()
        };

        assert_eq!(state.search_query_for(SkillsViewTab::Installed), "installed");
        assert_eq!(state.search_query_for(SkillsViewTab::Skills), "skills");
    }
}
