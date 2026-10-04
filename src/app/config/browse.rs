// SPDX-License-Identifier: Apache-2.0

use crate::agent::settings::{SettingDescriptor, SettingsSnapshot};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsFocus {
    CategoryTabs,
    Search,
    #[default]
    Content,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsPosition {
    pub selected: Option<String>,
    pub scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsBrowse {
    pub category: String,
    pub focus: SettingsFocus,
    pub query: String,
    pub cursor: usize,
    pub positions: BTreeMap<String, SettingsPosition>,
    pub search_position: SettingsPosition,
    pub visible_count: usize,
}

impl Default for SettingsBrowse {
    fn default() -> Self {
        Self {
            category: "general".into(),
            focus: SettingsFocus::Content,
            query: String::new(),
            cursor: 0,
            positions: BTreeMap::new(),
            search_position: SettingsPosition::default(),
            visible_count: 1,
        }
    }
}

impl SettingsBrowse {
    pub fn items<'a>(&self, snapshot: &'a SettingsSnapshot) -> Vec<&'a SettingDescriptor> {
        let tokens = self.query.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
        snapshot
            .catalog
            .iter()
            .filter(|setting| {
                setting.category == self.category
                    && (tokens.is_empty() || {
                        let metadata = format!(
                            "{} {} {} {}",
                            setting.label, setting.description, setting.id, self.category
                        )
                        .to_lowercase();
                        tokens.iter().all(|token| metadata.contains(token))
                    })
            })
            .collect::<Vec<_>>()
    }

    pub fn position(&self) -> Option<&SettingsPosition> {
        if self.query.is_empty() {
            self.positions.get(&self.category)
        } else {
            Some(&self.search_position)
        }
    }
    pub fn position_mut(&mut self) -> &mut SettingsPosition {
        if self.query.is_empty() {
            self.positions.entry(self.category.clone()).or_default()
        } else {
            &mut self.search_position
        }
    }
    pub fn selected_index(&self, snapshot: &SettingsSnapshot) -> usize {
        let items = self.items(snapshot);
        self.position()
            .and_then(|position| position.selected.as_ref())
            .and_then(|id| items.iter().position(|setting| setting.id == *id))
            .unwrap_or(0)
    }
    pub fn selected<'a>(&self, snapshot: &'a SettingsSnapshot) -> Option<&'a SettingDescriptor> {
        self.items(snapshot).get(self.selected_index(snapshot)).copied()
    }
    pub fn select(&mut self, id: String) {
        self.position_mut().selected = Some(id);
    }
    pub fn clear_search(&mut self) {
        self.query.clear();
        self.cursor = 0;
        self.search_position = SettingsPosition::default();
    }
    pub fn open_category(&mut self, category: &str) {
        self.clear_search();
        category.clone_into(&mut self.category);
        self.focus = SettingsFocus::Content;
    }
    pub fn reconcile(&mut self, snapshot: &SettingsSnapshot, previous_index: usize) {
        if !snapshot.categories.iter().any(|category| category.id == self.category)
            && let Some(category) = snapshot.categories.first()
        {
            self.open_category(&category.id);
        }
        let items = self.items(snapshot);
        let selected = self
            .position()
            .and_then(|position| position.selected.as_ref())
            .and_then(|id| items.iter().find(|setting| setting.id == *id))
            .or_else(|| items.get(previous_index.min(items.len().saturating_sub(1))))
            .map(|setting| setting.id.clone());
        self.position_mut().selected = selected;
        if self.position().is_none_or(|position| position.selected.is_none())
            && self.focus == SettingsFocus::Content
        {
            self.focus = SettingsFocus::Search;
        }
    }
}

impl super::edit::TextInputOverlay for SettingsBrowse {
    fn draft(&self) -> &str {
        &self.query
    }
    fn draft_mut(&mut self) -> &mut String {
        &mut self.query
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
}
