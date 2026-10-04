// SPDX-License-Identifier: Apache-2.0

use super::{
    StructuredEditor,
    structured::{self, DraftKey, FieldInput},
    structured_edit,
};
use crate::agent::settings::{EditorField, EditorSchema, EditorType};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct HookEntry {
    pub path: Vec<DraftKey>,
    pub event: String,
    pub matcher: Option<String>,
    pub action: String,
}

pub fn entries(root: &Value) -> Vec<HookEntry> {
    let mut entries = Vec::new();
    if let Some(events) = root.as_object() {
        for (event, groups) in events {
            for (group_index, group) in groups.as_array().into_iter().flatten().enumerate() {
                for (index, hook) in
                    group.get("hooks").and_then(Value::as_array).into_iter().flatten().enumerate()
                {
                    entries.push(HookEntry {
                        path: vec![
                            DraftKey::Field(event.clone()),
                            DraftKey::Index(group_index),
                            DraftKey::Field("hooks".into()),
                            DraftKey::Index(index),
                        ],
                        event: event.clone(),
                        matcher: group.get("matcher").and_then(Value::as_str).map(str::to_owned),
                        action: hook
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("Unrecognized action")
                            .to_owned(),
                    });
                }
            }
        }
    }
    entries
}

pub(super) fn handle_collection(
    form: &mut StructuredEditor,
    root: &mut Value,
    key: KeyEvent,
) -> Result<Option<bool>, String> {
    if key.code == KeyCode::Char('a')
        && key.modifiers.is_empty()
        && !form.read_only
        && form.path.is_empty()
    {
        form.hook_creation = Some(Box::new(HookCreation::new(form)?));
        return Ok(Some(false));
    }
    let hooks = entries(root);
    if key.code == KeyCode::Esc && key.modifiers.is_empty() && form.path.len() == 4 {
        form.selected = hooks.iter().position(|hook| hook.path == form.path).unwrap_or(0);
        form.path.clear();
        return Ok(Some(false));
    }
    if !form.path.is_empty() {
        return Ok(None);
    }
    form.selected = form.selected.min(hooks.len().saturating_sub(1));
    match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => return Ok(Some(true)),
        (KeyCode::Up, KeyModifiers::NONE) => form.selected = form.selected.saturating_sub(1),
        (KeyCode::Down, KeyModifiers::NONE) => {
            form.selected = (form.selected + 1).min(hooks.len().saturating_sub(1));
        }
        (KeyCode::Home, KeyModifiers::NONE) => form.selected = 0,
        (KeyCode::End, KeyModifiers::NONE) => form.selected = hooks.len().saturating_sub(1),
        (KeyCode::Enter | KeyCode::Char(' '), KeyModifiers::NONE) => {
            if let Some(hook) = hooks.get(form.selected) {
                form.path.clone_from(&hook.path);
                form.selected = 0;
            }
        }
        (KeyCode::Delete, KeyModifiers::NONE) if !form.read_only => {
            if let Some(hook) = hooks.get(form.selected) {
                structured::patch(root, &hook.path, None)?;
                form.selected = form.selected.min(hooks.len().saturating_sub(2));
            }
        }
        (KeyCode::Char('m'), KeyModifiers::NONE) if !form.read_only => {
            if let Some(hook) = hooks.get(form.selected) {
                let group = form
                    .schema
                    .item
                    .as_deref()
                    .and_then(|schema| schema.item.as_deref())
                    .ok_or("Matcher editor unavailable.")?;
                let field = group
                    .fields
                    .iter()
                    .find(|field| field.key == "matcher")
                    .ok_or("Matcher editor unavailable.")?;
                let mut path = hook.path[..2].to_vec();
                path.push(DraftKey::Field(field.key.clone()));
                let draft = hook.matcher.clone().unwrap_or_default();
                form.input = Some(FieldInput {
                    path,
                    schema: field.schema.clone(),
                    new_key: false,
                    cursor: draft.chars().count(),
                    draft,
                });
            }
        }
        _ => {}
    }
    Ok(Some(false))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationStep {
    Event,
    Matcher,
    Action,
    Field(usize),
    Review,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCreation {
    pub step: CreationStep,
    pub event: String,
    pub matcher: String,
    pub handler: Value,
    pub input: Option<FieldInput>,
}

fn action_schema(form: &StructuredEditor) -> Result<&EditorSchema, String> {
    form.schema
        .item
        .as_deref()
        .and_then(|schema| schema.item.as_deref())
        .and_then(|group| group.fields.iter().find(|field| field.key == "hooks"))
        .and_then(|field| field.schema.item.as_deref())
        .ok_or_else(|| "Hook action metadata is unavailable. Use advanced JSON.".into())
}

impl HookCreation {
    fn new(form: &StructuredEditor) -> Result<Self, String> {
        let event = form
            .path
            .first()
            .and_then(|key| if let DraftKey::Field(event) = key { Some(event) } else { None })
            .filter(|event| form.schema.keys.contains(event))
            .cloned()
            .or_else(|| form.schema.keys.first().cloned())
            .ok_or("Hook event choices are unavailable.")?;
        let mut creation = Self {
            step: CreationStep::Event,
            event,
            matcher: String::new(),
            handler: json!({}),
            input: None,
        };
        creation.set_step(form, CreationStep::Event)?;
        Ok(creation)
    }

    pub fn required_fields<'a>(
        &self,
        form: &'a StructuredEditor,
    ) -> Result<Vec<&'a EditorField>, String> {
        let kind = self.handler.get("type").and_then(Value::as_str).unwrap_or_default();
        Ok(action_schema(form)?
            .variants
            .get(kind)
            .ok_or("Choose a supported action.")?
            .fields
            .iter()
            .filter(|field| field.required)
            .collect())
    }

    pub fn title(&self, form: &StructuredEditor) -> String {
        match self.step {
            CreationStep::Event => "Add hook · 1/5 Choose event".into(),
            CreationStep::Matcher => "Add hook · 2/5 Matcher (optional)".into(),
            CreationStep::Action => "Add hook · 3/5 Choose action".into(),
            CreationStep::Field(index) => format!(
                "Add hook · 4/5 {} *",
                self.required_fields(form)
                    .ok()
                    .and_then(|fields| fields.get(index).map(|field| field.label.as_str()))
                    .unwrap_or("Required value")
            ),
            CreationStep::Review => "Add hook · 5/5 Review".into(),
        }
    }

    fn set_step(&mut self, form: &StructuredEditor, step: CreationStep) -> Result<(), String> {
        let (mut schema, draft) = match step {
            CreationStep::Event => (form.schema.clone(), self.event.clone()),
            CreationStep::Matcher => {
                let schema = form
                    .schema
                    .item
                    .as_deref()
                    .and_then(|schema| schema.item.as_deref())
                    .and_then(|schema| schema.fields.iter().find(|field| field.key == "matcher"))
                    .ok_or("Matcher metadata unavailable.")?
                    .schema
                    .clone();
                (schema, self.matcher.clone())
            }
            CreationStep::Action => {
                let schema = action_schema(form)?.clone();
                let draft = self
                    .handler
                    .get("type")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| schema.options.first().and_then(Value::as_str).map(str::to_owned))
                    .ok_or("Action choices unavailable.")?;
                (schema, draft)
            }
            CreationStep::Field(index) => {
                let fields = self.required_fields(form)?;
                let field = fields.get(index).ok_or("Required field unavailable.")?;
                (
                    field.schema.clone(),
                    self.handler
                        .get(&field.key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                )
            }
            CreationStep::Review => {
                self.step = step;
                self.input = None;
                return Ok(());
            }
        };
        if step == CreationStep::Event {
            schema.options = schema.keys.iter().map(|key| json!(key)).collect();
        }
        schema.kind = EditorType::String;
        self.input = Some(FieldInput {
            path: vec![],
            schema,
            new_key: false,
            cursor: draft.chars().count(),
            draft,
        });
        self.step = step;
        Ok(())
    }

    fn accept(&mut self, form: &StructuredEditor) -> Result<(), String> {
        let draft = self.input.as_ref().map(|input| input.draft.clone()).unwrap_or_default();
        let next = match self.step {
            CreationStep::Event => {
                self.event = draft;
                CreationStep::Matcher
            }
            CreationStep::Matcher => {
                self.matcher = draft;
                CreationStep::Action
            }
            CreationStep::Action => {
                if self.handler.get("type").and_then(Value::as_str) != Some(draft.as_str()) {
                    self.handler = json!({"type":draft});
                }
                if self.required_fields(form)?.is_empty() {
                    CreationStep::Review
                } else {
                    CreationStep::Field(0)
                }
            }
            CreationStep::Field(index) => {
                let fields = self.required_fields(form)?;
                let field = fields.get(index).ok_or("Required field unavailable.")?;
                if draft.is_empty() {
                    return Err(format!("Enter {} before continuing.", field.label.to_lowercase()));
                }
                self.handler[&field.key] = json!(draft);
                if index + 1 < fields.len() {
                    CreationStep::Field(index + 1)
                } else {
                    CreationStep::Review
                }
            }
            CreationStep::Review => return Ok(()),
        };
        self.set_step(form, next)
    }

    fn previous(&self, form: &StructuredEditor) -> CreationStep {
        match self.step {
            CreationStep::Event | CreationStep::Matcher => CreationStep::Event,
            CreationStep::Action => CreationStep::Matcher,
            CreationStep::Field(0) => CreationStep::Action,
            CreationStep::Field(index) => CreationStep::Field(index.saturating_sub(1)),
            CreationStep::Review => self
                .required_fields(form)
                .ok()
                .filter(|fields| !fields.is_empty())
                .map_or(CreationStep::Action, |fields| CreationStep::Field(fields.len() - 1)),
        }
    }
}

pub(super) fn update_creation(
    form: &mut StructuredEditor,
    root: &mut Value,
    key: KeyEvent,
) -> Result<(), String> {
    let Some(mut creation) = form.hook_creation.take() else {
        return Ok(());
    };
    let result = match (key.code, key.modifiers, creation.step) {
        (KeyCode::Esc, KeyModifiers::NONE, CreationStep::Event) => return Ok(()),
        (KeyCode::Esc, KeyModifiers::NONE, _) => creation.set_step(form, creation.previous(form)),
        (KeyCode::Enter, KeyModifiers::NONE, CreationStep::Review) => {
            let result = append_hook(form, root, &creation);
            if result.is_ok() {
                return Ok(());
            }
            result
        }
        (KeyCode::Enter, KeyModifiers::NONE, _) => creation.accept(form),
        _ => {
            if let Some(input) = &mut creation.input {
                structured_edit::edit_field(input, key);
            }
            Ok(())
        }
    };
    form.hook_creation = Some(creation);
    result
}

fn append_hook(
    form: &mut StructuredEditor,
    root: &mut Value,
    creation: &HookCreation,
) -> Result<(), String> {
    let events = root.as_object_mut().ok_or("Correct the hooks object in advanced JSON first.")?;
    let groups = events
        .entry(creation.event.clone())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("Correct this event's matcher groups in advanced JSON first.")?;
    let mut group = json!({"hooks":[creation.handler]});
    if !creation.matcher.is_empty() {
        group["matcher"] = json!(creation.matcher);
    }
    let index = groups.len();
    groups.push(group);
    form.path = vec![
        DraftKey::Field(creation.event.clone()),
        DraftKey::Index(index),
        DraftKey::Field("hooks".into()),
        DraftKey::Index(0),
    ];
    form.selected = 0;
    Ok(())
}
