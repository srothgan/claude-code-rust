// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

pub(crate) const SETTING_ID: &str = "presentation.footer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FooterFieldId {
    PermissionMode,
    Model,
    Effort,
    Ultracode,
    FastMode,
    Directory,
    GitBranch,
    ContextRemaining,
}

impl FooterFieldId {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::PermissionMode => "Permission mode",
            Self::Model => "Model",
            Self::Effort => "Effort",
            Self::Ultracode => "Ultracode",
            Self::FastMode => "Fast mode",
            Self::Directory => "Working directory",
            Self::GitBranch => "Git branch",
            Self::ContextRemaining => "Context remaining",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FooterSide {
    Left,
    Right,
}

impl FooterSide {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FooterField {
    pub(crate) id: FooterFieldId,
    pub(crate) visible: bool,
    pub(crate) side: FooterSide,
}

/// Each field occurs once. Array order is reading order within each side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct FooterLayout(Vec<FooterField>);

impl Default for FooterLayout {
    fn default() -> Self {
        use FooterFieldId as Id;
        use FooterSide::{Left, Right};
        Self(
            [
                (Id::PermissionMode, Left),
                (Id::Model, Left),
                (Id::Effort, Left),
                (Id::Ultracode, Left),
                (Id::FastMode, Left),
                (Id::Directory, Right),
                (Id::GitBranch, Right),
                (Id::ContextRemaining, Right),
            ]
            .into_iter()
            .map(|(id, side)| FooterField { id, visible: true, side })
            .collect(),
        )
    }
}

impl<'de> Deserialize<'de> for FooterLayout {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Vec::<FooterField>::deserialize(deserializer)?;
        for (index, field) in fields.iter().enumerate() {
            if fields[..index].iter().any(|previous| previous.id == field.id) {
                return Err(serde::de::Error::custom(format!(
                    "{} occurs more than once",
                    field.id.label()
                )));
            }
        }
        // Omitted fields stay available in the editor without enabling new content.
        for mut field in Self::default().0 {
            if !fields.iter().any(|entry| entry.id == field.id) {
                field.visible = false;
                fields.push(field);
            }
        }
        fields.sort_by_key(|field| field.side);
        Ok(Self(fields))
    }
}

impl FooterLayout {
    pub(crate) fn from_saved(value: Option<&serde_json::Value>) -> Result<Self, serde_json::Error> {
        value.map_or_else(|| Ok(Self::default()), |value| serde_json::from_value(value.clone()))
    }

    pub(crate) fn fields(&self) -> &[FooterField] {
        &self.0
    }

    pub(crate) fn toggle(&mut self, index: usize) {
        if let Some(field) = self.0.get_mut(index) {
            field.visible = !field.visible;
        }
    }

    /// Changing sides appends to the destination group and follows the field.
    pub(crate) fn switch_side(&mut self, index: usize) -> usize {
        let Some(field) = self.0.get(index) else {
            return index;
        };
        let side = match field.side {
            FooterSide::Left => FooterSide::Right,
            FooterSide::Right => FooterSide::Left,
        };
        let mut field = self.0.remove(index);
        field.side = side;
        let next = self.0.iter().take_while(|entry| entry.side <= side).count();
        self.0.insert(next, field);
        next
    }

    pub(crate) fn move_field(&mut self, index: usize, down: bool) -> usize {
        let next = if down { index.checked_add(1) } else { index.checked_sub(1) };
        let Some(next) = next.filter(|&next| {
            self.0.get(index).zip(self.0.get(next)).is_some_and(|(a, b)| a.side == b.side)
        }) else {
            return index;
        };
        self.0.swap(index, next);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn saved_order_and_disabled_placement_survive_round_trip() {
        let fixture = json!([
            {"id":"effort","visible":false,"side":"right"},
            {"id":"model","visible":true,"side":"left"},
            {"id":"gitBranch","visible":true,"side":"right"}
        ]);
        let layout = FooterLayout::from_saved(Some(&fixture)).expect("layout");
        let right: Vec<_> =
            layout.fields().iter().filter(|f| f.side == FooterSide::Right).collect();
        assert_eq!(right[0].id, FooterFieldId::Effort);
        assert!(!right[0].visible);
        assert_eq!(right[1].id, FooterFieldId::GitBranch);
        assert_eq!(layout.fields().iter().filter(|f| f.visible).count(), 2);
        let saved = serde_json::to_value(&layout).expect("serialize");
        assert_eq!(FooterLayout::from_saved(Some(&saved)).expect("reload"), layout);
    }

    #[test]
    fn empty_layout_disables_fields_and_missing_setting_restores_defaults() {
        let empty = FooterLayout::from_saved(Some(&json!([]))).expect("empty layout");
        assert_eq!(empty.fields().len(), 8);
        assert!(empty.fields().iter().all(|f| !f.visible));
        assert!(
            FooterLayout::from_saved(None).expect("defaults").fields().iter().all(|f| f.visible)
        );
    }

    #[test]
    fn invalid_fields_and_duplicates_are_rejected() {
        for fixture in [
            json!([{"id":"model","visible":true,"side":"middle"}]),
            json!([{"id":"unknown","visible":true,"side":"left"}]),
            json!([
                {"id":"model","visible":true,"side":"left"},
                {"id":"model","visible":false,"side":"right"}
            ]),
        ] {
            assert!(FooterLayout::from_saved(Some(&fixture)).is_err());
        }
    }

    #[test]
    fn reordering_stays_within_side_and_switching_appends_to_destination() {
        let mut layout = FooterLayout::default();
        assert_eq!(layout.move_field(0, false), 0);
        assert_eq!(layout.move_field(4, true), 4);
        assert_eq!(layout.move_field(7, true), 7);
        let moved = layout.move_field(1, true);
        assert_eq!(moved, 2);
        assert_eq!(layout.fields()[moved].id, FooterFieldId::Model);
        layout.toggle(moved);
        let moved = layout.switch_side(moved);
        assert_eq!(moved, 7);
        assert_eq!(layout.fields()[moved].id, FooterFieldId::Model);
        assert!(!layout.fields()[moved].visible);
        let moved = layout.switch_side(moved);
        assert_eq!(moved, 4);
        assert_eq!(layout.fields()[moved].side, FooterSide::Left);
    }
}
