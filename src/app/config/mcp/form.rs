// SPDX-License-Identifier: Apache-2.0

//! The host owns schema support, editable drafts, omission, and typed validation.
use crate::agent::types::{
    ElicitationAction, ElicitationMode, ElicitationRequest, ElicitationResponse,
};
use serde_json::{Map, Value};
mod formats;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, PartialEq, Default)]
pub struct ElicitationQueue {
    // None records settlement without retaining answers; duplicate delivery cannot reopen it.
    entries: BTreeMap<String, Option<ElicitationEntry>>,
    order: VecDeque<String>,
}
impl std::fmt::Debug for ElicitationQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElicitationQueue").field("count", &self.len()).finish_non_exhaustive()
    }
}
impl ElicitationQueue {
    pub fn active(&self) -> Option<&ElicitationEntry> {
        self.entries.get(self.order.front()?)?.as_ref()
    }
    pub fn active_mut(&mut self) -> Option<&mut ElicitationEntry> {
        self.entries.get_mut(self.order.front()?)?.as_mut()
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut ElicitationEntry> {
        self.entries.get_mut(id)?.as_mut()
    }
    pub fn insert(&mut self, request: ElicitationRequest) -> bool {
        let id = request.request_id.clone();
        if self.entries.contains_key(&id) {
            return false;
        }
        self.entries.insert(id.clone(), Some(ElicitationEntry::new(request)));
        self.order.push_back(id);
        true
    }
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(entry) = self.entries.get_mut(id) else {
            return false;
        };
        if entry.take().is_none() {
            return false;
        }
        self.order.retain(|key| key != id);
        true
    }
}

#[derive(Clone, PartialEq)]
pub struct ElicitationEntry {
    pub request: ElicitationRequest,
    pub form: Result<Form, String>,
    pub focus: usize,
    pub sending: bool,
    pub error: Option<String>,
    pub browser_opened: bool,
    pub browser_open_error: Option<String>,
}
impl ElicitationEntry {
    fn new(request: ElicitationRequest) -> Self {
        let form = if request.mode == ElicitationMode::Form {
            Form::parse(request.requested_schema.as_ref(), &request.field_order)
        } else {
            Ok(Form { fields: vec![] })
        };
        Self {
            request,
            form,
            focus: 0,
            sending: false,
            error: None,
            browser_opened: false,
            browser_open_error: None,
        }
    }
    pub fn field_count(&self) -> usize {
        self.form.as_ref().map_or(0, |form| form.fields.len())
    }
    pub fn actions(&self) -> &[ElicitationAction] {
        if self.form.is_ok() {
            &[ElicitationAction::Accept, ElicitationAction::Decline, ElicitationAction::Cancel]
        } else {
            &[ElicitationAction::Decline, ElicitationAction::Cancel]
        }
    }
    pub fn move_focus(&mut self, delta: isize) {
        self.focus = self
            .focus
            .saturating_add_signed(delta)
            .min(self.field_count() + self.actions().len() - 1);
    }
    pub fn field_mut(&mut self) -> Option<&mut Field> {
        self.form.as_mut().ok()?.fields.get_mut(self.focus)
    }
    pub fn response(&mut self, action: ElicitationAction) -> Option<ElicitationResponse> {
        if self.sending {
            return None;
        }
        if action != ElicitationAction::Accept || self.request.mode == ElicitationMode::Url {
            return Some(ElicitationResponse { action, content: None });
        }
        let Ok(form) = &self.form else {
            return None;
        };
        match form.content() {
            Ok(content) => {
                self.error = None;
                Some(ElicitationResponse { action, content: Some(Value::Object(content)) })
            }
            Err((index, message)) => {
                self.focus = index;
                self.error = Some(message);
                None
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Form {
    pub fields: Vec<Field>,
}
#[derive(Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    pub required: bool,
    pub present: bool,
    pub draft: Draft,
    schema: Value,
}
#[derive(Clone, PartialEq)]
pub enum Draft {
    Text(TextDraft),
    Boolean(bool),
    Choice { options: Vec<(String, String)>, selected: Option<usize> },
    Multi { options: Vec<(String, String)>, selected: Vec<usize>, cursor: usize },
}
#[derive(Clone, PartialEq)]
pub struct TextDraft {
    pub text: String,
    pub cursor: usize,
}
impl super::super::edit::TextInputOverlay for TextDraft {
    fn draft(&self) -> &str {
        &self.text
    }
    fn draft_mut(&mut self) -> &mut String {
        &mut self.text
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
}

impl Form {
    pub fn parse(schema: Option<&Value>, order: &[String]) -> Result<Self, String> {
        let schema = schema.ok_or("The server did not provide a form schema.")?;
        check_keys(schema, &["type", "properties", "required", "title", "description", "$schema"])?;
        if schema["type"] != "object" {
            return Err("Only an object with primitive fields is supported.".to_owned());
        }
        let properties = schema["properties"].as_object().ok_or("Missing form properties.")?;
        let required = match schema.get("required") {
            None => vec![],
            Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
                items.iter().filter_map(Value::as_str).collect()
            }
            _ => return Err("Invalid required fields.".to_owned()),
        };
        if required.iter().any(|key| !properties.contains_key(*key)) {
            return Err("A required field is missing from properties.".to_owned());
        }
        let mut names: Vec<&String> = vec![];
        for name in order.iter().chain(properties.keys()) {
            if properties.contains_key(name) && !names.contains(&name) {
                names.push(name);
            }
        }
        let fields = names
            .into_iter()
            .map(|name| Field::parse(name, &properties[name], required.contains(&name.as_str())))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { fields })
    }
    pub fn content(&self) -> Result<Map<String, Value>, (usize, String)> {
        let mut content = Map::new();
        for (index, field) in self.fields.iter().enumerate() {
            match field.answer() {
                Ok(Some(value)) => {
                    content.insert(field.name.clone(), value);
                }
                Ok(None) => {}
                Err(error) => return Err((index, format!("{}: {error}", field.title))),
            }
        }
        Ok(content)
    }
}

fn check_keys(schema: &Value, keys: &[&str]) -> Result<(), String> {
    let object = schema.as_object().ok_or("Invalid field schema.")?;
    if let Some(key) = object.keys().find(|key| !keys.contains(&key.as_str())) {
        return Err(format!("Unsupported schema keyword: {key}."));
    }
    Ok(())
}
fn choices(schema: &Value, titled: &str) -> Result<Vec<(String, String)>, String> {
    let mut options = vec![];
    if let Some(values) = schema["enum"].as_array() {
        for (index, value) in values.iter().enumerate() {
            let value = value.as_str().ok_or("Enum choices must be strings.")?;
            let label = schema["enumNames"][index].as_str().unwrap_or(value);
            options.push((value.to_owned(), label.to_owned()));
        }
    } else if let Some(values) = schema[titled].as_array() {
        for value in values {
            check_keys(value, &["const", "title"])?;
            let key = value["const"].as_str().ok_or("Enum choices must be strings.")?;
            let label = value["title"].as_str().ok_or("A titled choice needs a title.")?;
            options.push((key.to_owned(), label.to_owned()));
        }
    }
    if options.is_empty()
        || options.iter().map(|item| &item.0).collect::<BTreeSet<_>>().len() != options.len()
    {
        return Err("Choices must be nonempty and unique.".to_owned());
    }
    Ok(options)
}
impl Field {
    fn parse(name: &str, schema: &Value, required: bool) -> Result<Self, String> {
        let kind = schema["type"].as_str().ok_or("A field type is required.")?;
        let mut keys = vec!["type", "title", "description", "default"];
        match kind {
            "string" => {
                keys.extend(["minLength", "maxLength", "format", "enum", "enumNames", "oneOf"]);
            }
            "number" | "integer" => keys.extend(["minimum", "maximum"]),
            "array" => keys.extend(["items", "minItems", "maxItems"]),
            "boolean" => {}
            _ => return Err(format!("Unsupported field type for {name}: {kind}.")),
        }
        check_keys(schema, &keys)?;
        if schema.get("enum").is_some() && schema.get("oneOf").is_some() {
            return Err("Conflicting enum definitions.".to_owned());
        }
        if let Some(names) = schema.get("enumNames")
            && names.as_array().is_none_or(|names| {
                names.iter().any(|name| !name.is_string())
                    || schema["enum"].as_array().is_none_or(|values| values.len() != names.len())
            })
        {
            return Err("Invalid enum labels.".to_owned());
        }
        for key in ["minLength", "maxLength", "minItems", "maxItems"] {
            if schema.get(key).is_some_and(|value| value.as_u64().is_none()) {
                return Err(format!("Invalid {key}."));
            }
        }
        for key in ["minimum", "maximum"] {
            if schema.get(key).is_some_and(|value| value.as_f64().is_none()) {
                return Err(format!("Invalid {key}."));
            }
        }
        if let Some(format) = schema.get("format")
            && format.as_str().and_then(formats::validator).is_none()
        {
            return Err("Unsupported string format.".to_owned());
        }
        let default = schema.get("default");
        let draft = match kind {
            "boolean" => Draft::Boolean(default.and_then(Value::as_bool).unwrap_or(false)),
            "array" => {
                check_keys(&schema["items"], &["type", "enum", "anyOf"])?;
                if schema["items"].get("type").is_some_and(|kind| kind != "string")
                    || (schema["items"].get("enum").is_some()
                        && schema["items"].get("anyOf").is_some())
                {
                    return Err("Only string choices are supported in arrays.".to_owned());
                }
                let options = choices(&schema["items"], "anyOf")?;
                let selected: Vec<usize> =
                    default.and_then(Value::as_array).map_or_else(Vec::new, |values| {
                        values
                            .iter()
                            .filter_map(|value| {
                                options.iter().position(|item| value.as_str() == Some(&item.0))
                            })
                            .collect()
                    });
                if selected.iter().collect::<BTreeSet<_>>().len() != selected.len() {
                    return Err("Duplicate default choices.".to_owned());
                }
                Draft::Multi { options, selected, cursor: 0 }
            }
            "string" if schema.get("enum").is_some() || schema.get("oneOf").is_some() => {
                let options = choices(schema, "oneOf")?;
                let selected = options
                    .iter()
                    .position(|item| default.and_then(Value::as_str) == Some(&item.0));
                Draft::Choice { options, selected }
            }
            _ => {
                let text = default.map_or_else(String::new, |value| {
                    value.as_str().map_or_else(|| value.to_string(), str::to_owned)
                });
                let cursor = text.chars().count();
                Draft::Text(TextDraft { text, cursor })
            }
        };
        let field = Self {
            name: name.to_owned(),
            title: schema["title"].as_str().unwrap_or(name).to_owned(),
            description: schema["description"].as_str().map(str::to_owned),
            required,
            present: required || default.is_some(),
            draft,
            schema: schema.clone(),
        };
        if let Some(default) = default {
            match field.value() {
                Ok(value) if &value == default => {}
                _ => return Err(format!("Invalid default for {name}.")),
            }
        }
        Ok(field)
    }
    pub fn text_mut(&mut self) -> Option<&mut TextDraft> {
        if let Draft::Text(text) = &mut self.draft { Some(text) } else { None }
    }
    pub fn adjust(&mut self, delta: isize, toggle: bool) {
        self.present = true;
        match &mut self.draft {
            Draft::Boolean(value) => *value = if toggle { !*value } else { delta > 0 },
            Draft::Choice { options, selected } => {
                *selected =
                    Some(selected.map_or(0, |value| {
                        value.saturating_add_signed(delta).min(options.len() - 1)
                    }));
            }
            Draft::Multi { options, selected, cursor } => {
                if toggle {
                    if selected.contains(cursor) {
                        selected.retain(|index| index != cursor);
                    } else {
                        selected.push(*cursor);
                    }
                } else {
                    *cursor = cursor.saturating_add_signed(delta).min(options.len() - 1);
                }
            }
            Draft::Text(_) => {}
        }
    }
    pub fn answer(&self) -> Result<Option<Value>, String> {
        if !self.present && !self.required {
            return Ok(None);
        }
        let value = self.value()?;
        self.validate(&value)?;
        Ok(Some(value))
    }
    fn value(&self) -> Result<Value, String> {
        let value = match &self.draft {
            Draft::Boolean(value) => Value::Bool(*value),
            Draft::Choice { options, selected } => Value::String(
                options.get(selected.ok_or("Choose a value.")?).ok_or("Choose a value.")?.0.clone(),
            ),
            Draft::Multi { options, selected, .. } => Value::Array(
                selected
                    .iter()
                    .filter_map(|index| options.get(*index))
                    .map(|item| Value::String(item.0.clone()))
                    .collect(),
            ),
            Draft::Text(text) if self.schema["type"] == "string" => {
                Value::String(text.text.clone())
            }
            Draft::Text(text) => {
                let number: serde_json::Number =
                    text.text.trim().parse().map_err(|_| "Enter a finite number.")?;
                if self.schema["type"] == "integer"
                    && number.as_f64().is_none_or(|n| n.fract() != 0.0)
                {
                    return Err("Enter a whole number.".to_owned());
                }
                Value::Number(number)
            }
        };
        Ok(value)
    }
    fn validate(&self, value: &Value) -> Result<(), String> {
        let length = value
            .as_str()
            .map(|text| text.chars().count())
            .or_else(|| value.as_array().map(Vec::len));
        for (low, high) in [("minLength", "maxLength"), ("minItems", "maxItems")] {
            if let Some(length) = length {
                if self.schema[low].as_u64().is_some_and(|min| (length as u64) < min) {
                    return Err(format!(
                        "Requires at least {} {}.",
                        self.schema[low],
                        if low == "minItems" { "choices" } else { "characters" }
                    ));
                }
                if self.schema[high].as_u64().is_some_and(|max| (length as u64) > max) {
                    return Err(format!(
                        "Allows at most {} {}.",
                        self.schema[high],
                        if high == "maxItems" { "choices" } else { "characters" }
                    ));
                }
            }
        }
        if let Some(number) = value.as_f64() {
            if self.schema["minimum"].as_f64().is_some_and(|min| number < min) {
                return Err(format!("Minimum: {}.", self.schema["minimum"]));
            }
            if self.schema["maximum"].as_f64().is_some_and(|max| number > max) {
                return Err(format!("Maximum: {}.", self.schema["maximum"]));
            }
        }
        if let (Some(text), Some(format)) = (value.as_str(), self.schema["format"].as_str()) {
            let valid = formats::validator(format).is_some_and(|validate| validate(text));
            if !valid {
                return Err(format!("Enter a valid {format}."));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
