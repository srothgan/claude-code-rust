// SPDX-License-Identifier: Apache-2.0
use super::*;

#[test]
fn edited_choices_preserve_wire_values_across_reversible_item_limits() {
    let mut form = form(&json!({"type":"object","properties":{
        "choice":{"type":"string","oneOf":[{"const":"wire-a","title":"Pretty A"},{"const":"wire-b","title":"Pretty B"}],"default":"wire-b"},
        "legacy":{"type":"string","enum":["old"],"enumNames":["Legacy"],"default":"old"},
        "multi":{"type":"array","title":"Destinations","items":{"anyOf":[{"const":"b","title":"Bee"},{"const":"a","title":"Aye"}]},"default":["a"],"minItems":1,"maxItems":1},
        "flag":{"type":"boolean","default":false},"number":{"type":"number","default":0.5}
    }}));
    assert_eq!(
        Value::Object(form.content().unwrap()),
        json!({"choice":"wire-b","legacy":"old","multi":["a"],"flag":false,"number":0.5})
    );
    form.fields.iter_mut().find(|f| f.name == "choice").unwrap().adjust(-1, false);
    let index = form.fields.iter().position(|f| f.name == "multi").unwrap();
    // The initial cursor is on Bee; adding it exceeds the maximum.
    form.fields[index].adjust(0, true);
    let (failed, reason) = form.content().unwrap_err();
    assert_eq!(failed, index);
    assert!(reason.contains("Destinations") && reason.contains("at most 1"), "{reason}");
    assert!(
        matches!(&form.fields[index].draft, Draft::Multi { selected, .. } if selected.contains(&0) && selected.contains(&1))
    );
    form.fields[index].adjust(1, false);
    form.fields[index].adjust(0, true);
    assert_eq!(
        Value::Object(form.content().unwrap()),
        json!({"choice":"wire-a","legacy":"old","multi":["b"],"flag":false,"number":0.5})
    );
    form.fields[index].adjust(-1, false);
    form.fields[index].adjust(0, true);
    let (failed, reason) = form.content().unwrap_err();
    assert_eq!(failed, index);
    assert!(reason.contains("Destinations") && reason.contains("at least 1"), "{reason}");
    assert!(
        matches!(&form.fields[index].draft, Draft::Multi { selected, .. } if selected.is_empty())
    );
    form.fields[index].adjust(1, false);
    form.fields[index].adjust(0, true);
    assert_eq!(
        Value::Object(form.content().unwrap()),
        json!({"choice":"wire-a","legacy":"old","multi":["a"],"flag":false,"number":0.5})
    );
}

#[test]
fn constrained_answers_report_the_later_field_and_preserve_literal_typed_boundaries() {
    let cases = [
        (
            json!({"type":"string","minLength":2,"maxLength":3}),
            vec!["a", "abcd"],
            vec![("é🦀", json!("é🦀")), ("é🦀z", json!("é🦀z"))],
        ),
        (
            json!({"type":"integer","minimum":0,"maximum":3}),
            vec!["-1", "4", "1.5"],
            vec![("0", json!(0)), ("3", json!(3))],
        ),
        (
            json!({"type":"number","minimum":1,"maximum":3}),
            vec!["0.5", "3.5"],
            vec![("1", json!(1)), ("3", json!(3)), ("2.5", json!(2.5))],
        ),
    ];
    for (schema, bad, good) in cases {
        check_answers(schema, bad, good);
    }
    for (format, bad, good) in [
        ("date", "2025-02-29", "2024-02-29"),
        ("date-time", "tomorrow", "2026-10-18T12:00:00+02:00"),
        ("uri", "relative/path", "https://example.test/a"),
        ("uri", "https://example.test/a b", "https://example.test/a%20b"),
        ("uri", "https://example.test/%GG", "urn:example:form"),
        ("email", "no-address", "test@example.test"),
        ("email", "a..b@example.test", "a+b@example.test"),
        ("email", "a@example..test", "a@[127.0.0.1]"),
        ("email", "a@-example.test", "a@[IPv6:::1]"),
    ] {
        check_answers(
            json!({"type":"string","format":format}),
            vec![bad],
            vec![(good, json!(good))],
        );
    }
    let mut form = ordered_form(json!({"type":"string","enum":["wire-a","wire-b"]}));
    let (index, reason) = form.content().unwrap_err();
    assert_eq!(index, 1);
    assert!(reason.contains("Destination"));
    form.fields[1].adjust(1, false);
    assert_eq!(
        Value::Object(form.content().unwrap()),
        json!({"leading":"retained","target":"wire-a"})
    );
}

fn ordered_form(mut schema: Value) -> Form {
    schema["title"] = json!("Destination");
    Form::parse(
        Some(&json!({"type":"object","properties":{
        "leading":{"type":"string","default":"retained"},"target":schema
    },"required":["leading","target"]})),
        &["leading".into(), "target".into()],
    )
    .ok()
    .unwrap()
}

fn check_answers(schema: Value, bad: Vec<&str>, good: Vec<(&str, Value)>) {
    let mut form = ordered_form(schema);
    for answer in bad {
        text(&mut form.fields[1], answer);
        let (index, reason) = form.content().unwrap_err();
        assert_eq!(index, 1, "{answer}");
        assert!(reason.starts_with("Destination:") && reason.len() > "Destination:".len());
        assert_eq!(form.fields[1].text_mut().unwrap().text, answer);
        assert_eq!(form.fields[0].text_mut().unwrap().text, "retained");
    }
    for (answer, expected) in good {
        text(&mut form.fields[1], answer);
        assert_eq!(
            Value::Object(form.content().unwrap()),
            json!({"leading":"retained","target":expected}),
            "{answer}"
        );
    }
}

fn entry(schema: Option<Value>) -> ElicitationEntry {
    ElicitationEntry::new(ElicitationRequest {
        request_id: "schema-request".into(),
        server_name: "schema-server".into(),
        message: "Schema request".into(),
        mode: ElicitationMode::Form,
        url: None,
        elicitation_id: None,
        field_order: vec![],
        requested_schema: schema,
    })
}

#[test]
fn unsupported_remote_schemas_refuse_accept_but_allow_content_free_exit() {
    let mut schemas = vec![
        None,
        Some(json!(false)),
        Some(json!({"type":"array","properties":{}})),
        Some(json!({"type":"object"})),
        Some(json!({"type":"object","properties":[]})),
        Some(json!({"type":"object","properties":{},"required":["missing"]})),
    ];
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
        json!({"type":"string","enum":["a"],"oneOf":[{"const":"b","title":"Bee"}]}),
    ] {
        schemas.push(Some(json!({"type":"object","properties":{"field":field}})));
    }
    for schema in schemas {
        let mut entry = entry(schema.clone());
        assert!(entry.form.is_err(), "{schema:?}");
        assert!(!entry.actions().contains(&ElicitationAction::Accept));
        assert!(entry.response(ElicitationAction::Accept).is_none(), "{schema:?}");
        for action in [ElicitationAction::Decline, ElicitationAction::Cancel] {
            assert!(entry.actions().contains(&action));
            let response = entry.response(action).unwrap();
            assert_eq!(response.action, action);
            assert_eq!(response.content, None);
        }
    }
    let mut supported = entry(Some(
        json!({"type":"object","properties":{"enabled":{"type":"boolean","default":false}}}),
    ));
    assert!(supported.actions().contains(&ElicitationAction::Accept));
    assert_eq!(
        supported.response(ElicitationAction::Accept).unwrap().content,
        Some(json!({"enabled":false}))
    );
}

#[test]
fn queue_diagnostics_exclude_request_schema_defaults_and_edited_answers() {
    let mut request = entry(Some(json!({"type":"object","properties":{"answer":{"type":"string","default":"private-default-903"}}}))).request;
    request.message = "private-message-714".into();
    let mut queue = ElicitationQueue::default();
    queue.insert(request);
    text(
        &mut queue.active_mut().unwrap().form.as_mut().ok().unwrap().fields[0],
        "private-edit-528",
    );
    assert_eq!(
        Value::Object(queue.active().unwrap().form.as_ref().ok().unwrap().content().unwrap()),
        json!({"answer":"private-edit-528"})
    );
    let debug = format!("{queue:?}");
    for private in ["private-message-714", "private-default-903", "private-edit-528"] {
        assert!(!debug.contains(private), "request data leaked: {debug}");
    }
}
