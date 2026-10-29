// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

mod preferences;
pub(crate) use preferences::{FooterPreferences, PendingSave, save};

pub(crate) const SETTING_ID: &str = "presentation.footer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FooterFieldId {
    PermissionMode,
    Model,
    Effort,
    Ultracode,
    FastMode,
    RunningSubagents,
    Directory,
    GitBranch,
    ContextRemaining,
}

impl FooterFieldId {
    pub(crate) const fn supports_color(self) -> bool {
        matches!(
            self,
            Self::Model | Self::Effort | Self::Directory | Self::GitBranch | Self::ContextRemaining
        )
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::PermissionMode => "Permission mode",
            Self::Model => "Model",
            Self::Effort => "Effort",
            Self::Ultracode => "Ultracode",
            Self::FastMode => "Fast mode",
            Self::RunningSubagents => "Running subagents",
            Self::Directory => "Working directory",
            Self::GitBranch => "Git branch",
            Self::ContextRemaining => "Context remaining",
        }
    }
}

macro_rules! palette {
    ($($name:ident => ($r:literal, $g:literal, $b:literal)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub(crate) enum FooterColor { $($name),+ }

        impl FooterColor {
            pub(crate) const ALL: &[Self] = &[$(Self::$name),+];
            pub(crate) const fn label(self) -> &'static str {
                match self { $(Self::$name => stringify!($name)),+ }
            }
            pub(crate) const fn color(self) -> ratatui::style::Color {
                match self { $(Self::$name => ratatui::style::Color::Rgb($r, $g, $b)),+ }
            }
        }
    };
}

palette! {
    Red => (248, 113, 113), Orange => (251, 146, 60), Amber => (251, 191, 36),
    Yellow => (250, 204, 21), Lime => (163, 230, 53), Green => (74, 222, 128),
    Emerald => (52, 211, 153), Teal => (45, 212, 191), Cyan => (34, 211, 238),
    Sky => (56, 189, 248), Blue => (96, 165, 250), Indigo => (129, 140, 248),
    Violet => (167, 139, 250), Purple => (192, 132, 252), Fuchsia => (232, 121, 249),
    Pink => (244, 114, 182), Rose => (251, 113, 133), White => (248, 250, 252),
    Gray => (161, 161, 170), Slate => (148, 163, 184),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<FooterColor>,
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
                (Id::RunningSubagents, Left),
                (Id::Directory, Right),
                (Id::GitBranch, Right),
                (Id::ContextRemaining, Right),
            ]
            .into_iter()
            .map(|(id, side)| FooterField { id, visible: true, side, color: None })
            .collect(),
        )
    }
}

impl<'de> Deserialize<'de> for FooterLayout {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = Vec::<FooterField>::deserialize(deserializer)?;
        for (index, field) in fields.iter().enumerate() {
            if field.color.is_some() && !field.id.supports_color() {
                return Err(serde::de::Error::custom(format!(
                    "{} uses its fixed semantic color",
                    field.id.label()
                )));
            }
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
    pub(crate) fn set_color(&mut self, index: usize, color: Option<FooterColor>) {
        if let Some(field) = self.0.get_mut(index).filter(|field| field.id.supports_color()) {
            field.color = color;
        }
    }

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
    fn only_informational_fields_accept_named_colors_and_old_entries_keep_their_appearance() {
        for (id, field_id) in [
            ("model", FooterFieldId::Model),
            ("effort", FooterFieldId::Effort),
            ("directory", FooterFieldId::Directory),
            ("gitBranch", FooterFieldId::GitBranch),
            ("contextRemaining", FooterFieldId::ContextRemaining),
        ] {
            let layout = FooterLayout::from_saved(Some(&json!([
                {"id":id,"visible":true,"side":"left","color":"cyan"}
            ])))
            .expect("eligible field");
            assert_eq!(
                layout
                    .fields()
                    .iter()
                    .find(|field| field.id == field_id)
                    .expect("colored field")
                    .color,
                Some(FooterColor::Cyan)
            );
            let plain = FooterLayout::from_saved(Some(&json!([
                {"id":id,"visible":true,"side":"left"}
            ])))
            .expect("existing format");
            assert!(plain.fields().iter().all(|field| field.color.is_none()));
            assert!(
                serde_json::to_value(plain)
                    .expect("serialize")
                    .as_array()
                    .expect("array")
                    .iter()
                    .all(|field| field.get("color").is_none())
            );
        }
        for id in ["permissionMode", "fastMode", "ultracode"] {
            assert!(
                FooterLayout::from_saved(Some(&json!([
                    {"id":id,"visible":true,"side":"left","color":"cyan"}
                ])))
                .is_err(),
                "fixed field {id}"
            );
        }
        for color in ["", "ultraviolet", "#123456"] {
            assert!(
                FooterLayout::from_saved(Some(&json!([
                    {"id":"model","visible":true,"side":"left","color":color}
                ])))
                .is_err()
            );
        }
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
}

#[cfg(test)]
#[path = "footer/layout_audit_tests.rs"]
mod audit_tests;
