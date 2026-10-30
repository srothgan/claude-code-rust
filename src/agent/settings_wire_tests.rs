// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::settings::{
    EditorField, EditorSchema, EditorType, SettingKind, SettingsApplication, SettingsMutation,
    SettingsOperation, SettingsPersistence, SettingsScope,
};
use serde_json::{Value, json};

#[test]
fn settings_mutations_preserve_scope_empty_override_and_reset_identity() {
    for (envelope, expected) in [
        (
            CommandEnvelope {
                request_id: Some("project-operation".into()),
                command: BridgeCommand::MutateSetting {
                    session_id: "project-session".into(),
                    app_settings_path: Some("/profiles/project.json".into()),
                    mutation: SettingsMutation {
                        context: "project-context".into(),
                        id: "permissions.allow".into(),
                        scope: SettingsScope::Project,
                        expected_revision: "project-revision".into(),
                        operation: SettingsOperation::Set,
                        value: Some(json!([])),
                    },
                },
            },
            json!({"request_id":"project-operation","command":"mutate_setting","session_id":"project-session",
                "app_settings_path":"/profiles/project.json","mutation":{"context":"project-context","id":"permissions.allow",
                    "scope":"project","expected_revision":"project-revision","operation":"set","value":[]}}),
        ),
        (
            CommandEnvelope {
                request_id: Some("local-operation".into()),
                command: BridgeCommand::MutateSetting {
                    session_id: "local-session".into(),
                    app_settings_path: Some("/profiles/local.json".into()),
                    mutation: SettingsMutation {
                        context: "local-context".into(),
                        id: "permissions.allow".into(),
                        scope: SettingsScope::Local,
                        expected_revision: "local-revision".into(),
                        operation: SettingsOperation::Remove,
                        value: None,
                    },
                },
            },
            json!({"request_id":"local-operation","command":"mutate_setting","session_id":"local-session",
                "app_settings_path":"/profiles/local.json","mutation":{"context":"local-context","id":"permissions.allow",
                    "scope":"local","expected_revision":"local-revision","operation":"remove"}}),
        ),
    ] {
        assert_eq!(serde_json::to_value(envelope).unwrap(), expected);
    }
}

fn field<'a>(schema: &'a EditorSchema, key: &str) -> &'a EditorField {
    schema.fields.iter().find(|field| field.key == key).expect("named schema field")
}

fn raw_field<'a>(schema: &'a Value, key: &str) -> &'a Value {
    schema["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == key)
        .expect("independent named schema field")
}

#[test]
fn settings_results_preserve_category_association_and_recursive_hook_schema() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/settings-ui-catalog.json"))
            .unwrap();
    let mut raw = fixture.clone();
    raw["cwd"] = json!("/project");
    raw["context"] = json!("catalog-context");
    for name in ["sources", "values", "resolution_sources", "diagnostics"] {
        raw[name] = json!([]);
    }
    raw["provenance"] = json!({});
    let envelope: EventEnvelope = serde_json::from_value(json!({"event":"settings_result","session_id":"catalog-session",
        "request_id":"inspect-operation","result":{"persistence":"not_requested","application":"blocked","snapshot":raw}})).unwrap();
    assert_eq!(envelope.request_id.as_deref(), Some("inspect-operation"));
    let BridgeEvent::SettingsResult { session_id, result } = envelope.event else {
        panic!("settings result")
    };
    assert_eq!(session_id, "catalog-session");
    assert_eq!(result.persistence, SettingsPersistence::NotRequested);
    assert_eq!(result.application, SettingsApplication::Blocked);
    assert_eq!(result.error, None);
    let snapshot = result.snapshot.unwrap();
    assert_eq!(snapshot.cwd, "/project");
    assert_eq!(snapshot.context, "catalog-context");
    for expected in fixture["categories"].as_array().unwrap() {
        let category =
            snapshot.categories.iter().find(|c| c.id == expected["id"].as_str().unwrap()).unwrap();
        assert_eq!(category.label, expected["label"].as_str().unwrap());
        assert_eq!(category.short_label, expected["short_label"].as_str().unwrap());
    }
    for expected in fixture["catalog"].as_array().unwrap() {
        let setting =
            snapshot.catalog.iter().find(|s| s.id == expected["id"].as_str().unwrap()).unwrap();
        assert_eq!(setting.category, expected["category"].as_str().unwrap(), "{}", setting.id);
        assert!(snapshot.categories.iter().any(|c| c.id == setting.category));
    }
    let hook = snapshot.catalog.iter().find(|s| s.id == "hooks").unwrap();
    let raw_hook =
        fixture["catalog"].as_array().unwrap().iter().find(|s| s["id"] == "hooks").unwrap();
    assert_eq!(hook.key_path, ["hooks"]);
    assert_eq!(hook.kind, SettingKind::Json);
    let editor = hook.editor.as_ref().unwrap();
    assert_eq!(editor.kind, EditorType::Map);
    let groups = editor.item.as_ref().unwrap();
    assert_eq!(groups.kind, EditorType::Array);
    let group = groups.item.as_ref().unwrap();
    assert_eq!(group.kind, EditorType::Object);
    let matcher = field(group, "matcher");
    assert!(!matcher.required);
    assert_eq!(matcher.schema.kind, EditorType::String);
    let handlers = &field(group, "hooks").schema;
    let raw_handlers = &raw_field(&raw_hook["editor"]["item"]["item"], "hooks")["schema"];
    assert_eq!(handlers.kind, EditorType::Array);
    let variant = handlers.item.as_ref().unwrap();
    assert_eq!(variant.kind, EditorType::Variant);
    assert_eq!(variant.options, raw_handlers["item"]["options"].as_array().unwrap().as_slice());
    let command = variant.variants.get("command").unwrap();
    assert_eq!(command.kind, EditorType::Object);
    let command_field = field(command, "command");
    let expected = raw_field(&raw_handlers["item"]["variants"]["command"], "command");
    assert_eq!(command_field.required, expected["required"].as_bool().unwrap());
    assert_eq!(command_field.schema.kind, EditorType::String);
    assert_eq!(
        command_field.schema.description.as_deref(),
        Some(expected["schema"]["description"].as_str().unwrap())
    );
}
