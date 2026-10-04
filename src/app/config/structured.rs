// SPDX-License-Identifier: Apache-2.0

use crate::agent::settings::{EditorSchema, EditorType};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftKey {
    Field(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInput {
    pub path: Vec<DraftKey>,
    pub schema: EditorSchema,
    pub new_key: bool,
    pub draft: String,
    pub cursor: usize,
}
impl super::edit::TextInputOverlay for FieldInput {
    fn draft(&self) -> &str {
        &self.draft
    }
    fn draft_mut(&mut self) -> &mut String {
        &mut self.draft
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredEditor {
    pub schema: EditorSchema,
    pub path: Vec<DraftKey>,
    pub selected: usize,
    pub input: Option<FieldInput>,
    pub advanced: bool,
    pub read_only: bool,
    pub hook_creation: Option<Box<super::hooks::HookCreation>>,
}

impl FieldInput {
    pub fn options(&self) -> Vec<Value> {
        if self.new_key {
            self.schema.keys.iter().map(|key| json!(key)).collect()
        } else {
            self.schema.options.clone()
        }
    }
}

#[derive(Debug, Clone)]
pub struct FormRow {
    pub key: DraftKey,
    pub label: String,
    pub schema: EditorSchema,
    pub value: Option<Value>,
    pub required: bool,
}

fn scalar(kind: EditorType) -> EditorSchema {
    EditorSchema {
        description: None,
        kind,
        fields: vec![],
        item: None,
        options: vec![],
        keys: vec![],
        variants: std::collections::BTreeMap::default(),
    }
}

pub fn at<'a>(value: &'a Value, path: &[DraftKey]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, key| match key {
        DraftKey::Field(key) => value.get(key),
        DraftKey::Index(index) => value.get(*index),
    })
}
pub fn at_mut<'a>(value: &'a mut Value, path: &[DraftKey]) -> Option<&'a mut Value> {
    path.iter().try_fold(value, |value, key| match key {
        DraftKey::Field(key) => value.get_mut(key),
        DraftKey::Index(index) => value.get_mut(*index),
    })
}
pub fn empty(schema: &EditorSchema) -> Value {
    match schema.kind {
        EditorType::Array => json!([]),
        EditorType::Object | EditorType::Map => json!({}),
        EditorType::Variant => {
            json!({"type": schema.options.first().cloned().unwrap_or(Value::Null)})
        }
        EditorType::Boolean => json!(false),
        EditorType::Number => json!(0),
        _ => schema.options.first().cloned().unwrap_or_else(|| json!("")),
    }
}
pub fn container(schema: &EditorSchema) -> bool {
    matches!(
        schema.kind,
        EditorType::Array | EditorType::Object | EditorType::Map | EditorType::Variant
    )
}

impl StructuredEditor {
    pub fn supports_current_shape(&self, root: &Value) -> bool {
        let Some(schema) = self.schema_at(root) else {
            return false;
        };
        let Some(value) = at(root, &self.path) else {
            return false;
        };
        match schema.kind {
            EditorType::Array => value.is_array(),
            EditorType::Object | EditorType::Map | EditorType::Variant => value.is_object(),
            _ => false,
        }
    }
    pub fn schema_at(&self, root: &Value) -> Option<EditorSchema> {
        let mut schema = self.schema.clone();
        let mut value = root;
        for key in &self.path {
            if schema.kind == EditorType::Variant {
                schema = schema.variants.get(value.get("type")?.as_str()?)?.clone();
            }
            schema = match key {
                DraftKey::Field(key) if schema.kind == EditorType::Object => schema
                    .fields
                    .iter()
                    .find(|field| field.key == *key)
                    .map_or_else(|| scalar(EditorType::Json), |field| field.schema.clone()),
                _ => *schema.item?,
            };
            value = match key {
                DraftKey::Field(key) => value.get(key)?,
                DraftKey::Index(index) => value.get(*index)?,
            };
        }
        Some(schema)
    }
    pub fn rows(&self, root: &Value) -> Vec<FormRow> {
        let Some(mut schema) = self.schema_at(root) else {
            return vec![];
        };
        let Some(value) = at(root, &self.path) else {
            return vec![];
        };
        let mut rows = vec![];
        if schema.kind == EditorType::Variant {
            let mut type_schema = scalar(EditorType::String);
            type_schema.options.clone_from(&schema.options);
            rows.push(FormRow {
                key: DraftKey::Field("type".into()),
                label: "Action type".into(),
                schema: type_schema,
                value: value.get("type").cloned(),
                required: true,
            });
            schema = value
                .get("type")
                .and_then(Value::as_str)
                .and_then(|kind| schema.variants.get(kind))
                .cloned()
                .unwrap_or_else(|| scalar(EditorType::Object));
        }
        match schema.kind {
            EditorType::Object => {
                for field in &schema.fields {
                    rows.push(FormRow {
                        key: DraftKey::Field(field.key.clone()),
                        label: field.label.clone(),
                        schema: field.schema.clone(),
                        value: value.get(&field.key).cloned(),
                        required: field.required,
                    });
                }
                if let Some(fields) = value.as_object() {
                    for (key, value) in fields {
                        if !rows.iter().any(|row| row.key == DraftKey::Field(key.clone())) {
                            rows.push(FormRow {
                                key: DraftKey::Field(key.clone()),
                                label: format!("Advanced field: {key}"),
                                schema: scalar(EditorType::Json),
                                value: Some(value.clone()),
                                required: false,
                            });
                        }
                    }
                }
            }
            EditorType::Array => {
                if let Some(items) = value.as_array()
                    && let Some(item) = &schema.item
                {
                    for (index, value) in items.iter().enumerate() {
                        rows.push(FormRow {
                            key: DraftKey::Index(index),
                            label: format!("{}", index + 1),
                            schema: *item.clone(),
                            value: Some(value.clone()),
                            required: false,
                        });
                    }
                }
            }
            EditorType::Map => {
                if let Some(fields) = value.as_object()
                    && let Some(item) = &schema.item
                {
                    for (key, value) in fields {
                        rows.push(FormRow {
                            key: DraftKey::Field(key.clone()),
                            label: key.clone(),
                            schema: *item.clone(),
                            value: Some(value.clone()),
                            required: false,
                        });
                    }
                }
            }
            _ => {}
        }
        rows
    }
    pub fn breadcrumb(&self) -> String {
        self.path
            .iter()
            .map(|key| match key {
                DraftKey::Field(key) => key.clone(),
                DraftKey::Index(index) => format!("{}", index + 1),
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }

    pub fn display_breadcrumb(&self, root: &Value) -> String {
        let mut parent = self.clone();
        parent.path.clear();
        let mut labels = Vec::new();
        for key in &self.path {
            labels.push(parent.rows(root).into_iter().find(|row| &row.key == key).map_or_else(
                || match key {
                    DraftKey::Field(key) => key.clone(),
                    DraftKey::Index(index) => format!("Item {}", index + 1),
                },
                |row| {
                    if matches!(key, DraftKey::Index(_)) {
                        format!("Item {}", row.label)
                    } else {
                        row.label
                    }
                },
            ));
            parent.path.push(key.clone());
        }
        labels.join(" / ")
    }
}

pub fn patch(root: &mut Value, path: &[DraftKey], value: Option<Value>) -> Result<(), String> {
    let Some((key, parent_path)) = path.split_last() else {
        return Err("Choose a field first.".into());
    };
    let parent = at_mut(root, parent_path).ok_or("This draft item is no longer available.")?;
    match (key, parent) {
        (DraftKey::Field(key), Value::Object(fields)) => {
            if let Some(value) = value {
                fields.insert(key.clone(), value);
            } else {
                fields.remove(key);
            }
        }
        (DraftKey::Index(index), Value::Array(items)) if *index <= items.len() => {
            if let Some(value) = value {
                if *index == items.len() {
                    items.push(value);
                } else {
                    items[*index] = value;
                }
            } else if *index < items.len() {
                items.remove(*index);
            }
        }
        _ => return Err("This value needs advanced JSON editing.".into()),
    }
    Ok(())
}

pub fn summary(value: &Value) -> String {
    match value {
        Value::Object(fields) => {
            if let Some(kind) = fields.get("type").and_then(Value::as_str) {
                return format!("{kind} action · {} fields", fields.len());
            }
            if let Some(hooks) = fields.get("hooks").and_then(Value::as_array) {
                let matcher = fields.get("matcher").and_then(Value::as_str).unwrap_or("No matcher");
                return format!("{matcher} · {} handlers", hooks.len());
            }
            format!("{} fields", fields.len())
        }
        Value::String(value) => value.replace(['\n', '\r'], " "),
        Value::Array(items) => format!("{} items", items.len()),
        _ => value.to_string(),
    }
}
