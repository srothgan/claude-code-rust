// SPDX-License-Identifier: Apache-2.0
use super::*;
use serde_json::json;

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
fn defaults_titled_choices_and_multi_select_keep_wire_values() {
    let mut form = form(&json!({"type":"object","properties":{
        "choice":{"type":"string","oneOf":[{"const":"wire-a","title":"Pretty A"},{"const":"wire-b","title":"Pretty B"}],"default":"wire-b"},
        "legacy":{"type":"string","enum":["old"],"enumNames":["Legacy"],"default":"old"},
        "multi":{"type":"array","items":{"anyOf":[{"const":"b","title":"Bee"},{"const":"a","title":"Aye"}]},"default":["a"],"minItems":1,"maxItems":1},
        "flag":{"type":"boolean","default":false},"number":{"type":"number","default":0.5}
    }}));
    assert_eq!(
        Value::Object(form.content().unwrap()),
        json!({"choice":"wire-b","legacy":"old","multi":["a"],"flag":false,"number":0.5})
    );
    let multi = form.fields.iter_mut().find(|f| f.name == "multi").unwrap();
    multi.adjust(0, true);
    assert!(form.content().unwrap_err().1.contains("at most"));
}

#[test]
fn required_choices_and_constraints_validate_without_changing_answers() {
    for (schema, bad, good) in [
        (json!({"type":"string","minLength":2,"maxLength":3}), "a", "\u{e9}\u{1f980}"),
        (json!({"type":"integer","minimum":0,"maximum":3}), "1.5", "0"),
        (json!({"type":"number","minimum":1,"maximum":3}), "4", "2.5"),
        (json!({"type":"string","format":"date"}), "2025-02-29", "2024-02-29"),
        (json!({"type":"string","format":"date-time"}), "tomorrow", "2026-10-18T12:00:00+02:00"),
        (json!({"type":"string","format":"uri"}), "relative/path", "https://example.test/a"),
        (json!({"type":"string","format":"email"}), "no-address", "test@example.test"),
        (json!({"type":"string","format":"email"}), "a..b@example.test", "a+b@example.test"),
        (json!({"type":"string","format":"email"}), "a@example..test", "a@[127.0.0.1]"),
        (json!({"type":"string","format":"email"}), "a@-example.test", "a@[IPv6:::1]"),
        (
            json!({"type":"string","format":"uri"}),
            "https://example.test/a b",
            "https://example.test/a%20b",
        ),
        (json!({"type":"string","format":"uri"}), "https://example.test/%GG", "urn:example:form"),
    ] {
        let mut form =
            form(&json!({"type":"object","properties":{"field":schema},"required":["field"]}));
        text(&mut form.fields[0], bad);
        assert!(form.content().is_err(), "accepted {bad}");
        assert_eq!(form.fields[0].text_mut().unwrap().text, bad);
        text(&mut form.fields[0], good);
        assert!(form.content().is_ok(), "rejected {good}");
    }
    let mut choice = form(
        &json!({"type":"object","properties":{"field":{"type":"string","enum":["a","b"]}},"required":["field"]}),
    );
    assert!(choice.content().is_err());
    choice.fields[0].adjust(1, false);
    assert_eq!(choice.content().unwrap()["field"], json!("a"));
}

#[test]
fn unsupported_constraints_and_invalid_defaults_never_offer_accept() {
    for field in [
        json!({"type":"object","properties":{}}),
        json!({"type":"string","pattern":"x"}),
        json!({"type":"array","items":{"type":"number","enum":["a"]}}),
        json!({"type":"boolean","default":"false"}),
        json!({"type":"integer","default":1.5}),
        json!({"type":"string","enum":["a"],"default":"b"}),
        json!({"type":"string","format":"password"}),
        json!({"type":"string","minLength":-1}),
        json!({"type":"array","items":{"type":"string","enum":["a"]},"default":["unknown"]}),
    ] {
        let entry = ElicitationEntry::new(ElicitationRequest {
            request_id: "a".into(),
            server_name: "test".into(),
            message: "test".into(),
            mode: ElicitationMode::Form,
            url: None,
            elicitation_id: None,
            field_order: vec![],
            requested_schema: Some(json!({"type":"object","properties":{"field":field}})),
        });
        assert!(entry.form.is_err());
        assert_eq!(entry.actions(), [ElicitationAction::Decline, ElicitationAction::Cancel]);
    }
}

#[test]
fn queue_deduplicates_preserves_drafts_and_removes_only_matching_requests() {
    let request = |id: &str| ElicitationRequest {
        request_id: id.into(),
        server_name: "test".into(),
        message: "test".into(),
        mode: ElicitationMode::Form,
        url: None,
        elicitation_id: None,
        field_order: vec![],
        requested_schema: Some(json!({"type":"object","properties":{"secret":{"type":"string"}}})),
    };
    let mut queue = ElicitationQueue::default();
    assert!(queue.insert(request("a")));
    text(
        &mut queue.active_mut().unwrap().form.as_mut().ok().unwrap().fields[0],
        "private-test-answer",
    );
    queue.active_mut().unwrap().focus = 1;
    assert!(queue.insert(request("b")));
    assert!(!queue.insert(request("a")));
    assert_eq!(queue.active().unwrap().focus, 1);
    assert!(!format!("{queue:?}").contains("private-test-answer"));
    assert!(queue.remove("b"));
    assert_eq!(queue.active().unwrap().request.request_id, "a");
    assert!(!queue.insert(request("b")));
    queue.insert(request("c"));
    assert!(queue.remove("a"));
    assert!(!queue.remove("a"));
    assert_eq!(queue.active().unwrap().request.request_id, "c");
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
