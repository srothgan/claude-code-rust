// SPDX-License-Identifier: Apache-2.0
use super::*;
use serde_json::json;

#[path = "audit_tests.rs"]
mod audit;

fn form(schema: &Value) -> Form {
    Form::parse(Some(schema), &[]).expect("supported schema")
}
fn text(field: &mut Field, value: &str) {
    field.present = true;
    field.text_mut().unwrap().text = value.to_owned();
}

#[test]
fn content_preserves_false_zero_empty_strings_arrays_and_omission() {
    let mut form = form(&json!({"type":"object", "properties":{
        "flag":{"type":"boolean"}, "count":{"type":"integer"}, "empty":{"type":"string"},
        "absent":{"type":"string"}, "tags":{"type":"array","items":{"type":"string","enum":["a","b"]}}
    }, "required":["flag","count","empty","tags"]}));
    text(form.fields.iter_mut().find(|f| f.name == "count").unwrap(), "0");
    assert_eq!(
        Value::Object(form.content().unwrap()),
        json!({"flag":false,"count":0,"empty":"","tags":[]})
    );
    let absent = form.fields.iter_mut().find(|f| f.name == "absent").unwrap();
    absent.present = true;
    assert_eq!(form.content().unwrap()["absent"], json!(""));
}

#[test]
fn public_field_order_survives_sorted_json_maps_and_duplicates() {
    let schema = json!({"type":"object","properties":{"alpha":{"type":"string"},"zulu":{"type":"boolean"},"middle":{"type":"integer"}}});
    let form = Form::parse(
        Some(&schema),
        &["zulu".into(), "alpha".into(), "zulu".into(), "unknown".into()],
    )
    .ok()
    .unwrap();
    assert_eq!(
        form.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["zulu", "alpha", "middle"]
    );
}

#[test]
fn a_representable_default_can_be_corrected_when_it_violates_a_constraint() {
    let mut form = form(&json!({"type":"object","properties":{
        "count":{"type":"integer","minimum":0,"maximum":3,"default":5}
    },"required":["count"]}));
    assert!(form.content().is_err());
    text(&mut form.fields[0], "0");
    assert_eq!(form.content().unwrap()["count"], json!(0));
}
