// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::{Value, json};

fn position(layout: &FooterLayout, id: FooterFieldId) -> usize {
    layout.fields().iter().position(|f| f.id == id).unwrap()
}
fn assert_saved_semantics(value: &Value) {
    let fields = value.as_array().unwrap();
    for expected in [
        json!({"id":"effort","visible":false,"side":"right","color":"cyan"}),
        json!({"id":"model","visible":true,"side":"left","color":"orange"}),
        json!({"id":"gitBranch","visible":true,"side":"right"}),
    ] {
        assert_eq!(fields.iter().find(|f| f["id"] == expected["id"]).unwrap(), &expected);
    }
    let right: Vec<_> =
        fields.iter().filter(|f| f["side"] == "right").map(|f| f["id"].as_str().unwrap()).collect();
    assert!(
        right.iter().position(|id| *id == "effort").unwrap()
            < right.iter().position(|id| *id == "gitBranch").unwrap()
    );
    assert!(
        fields
            .iter()
            .filter(|f| !["model", "gitBranch"].contains(&f["id"].as_str().unwrap()))
            .all(|f| f["visible"] == false)
    );
    assert!(fields.iter().any(|f| f["id"] == "directory" && f["visible"] == false));
}

#[test]
fn persisted_fields_retain_independent_identity_order_visibility_and_color_after_reload() {
    let input = json!([
        {"id":"effort","visible":false,"side":"right","color":"cyan"},
        {"id":"model","visible":true,"side":"left","color":"orange"},
        {"id":"gitBranch","visible":true,"side":"right"}
    ]);
    let layout = FooterLayout::from_saved(Some(&input)).unwrap();
    let saved = serde_json::to_value(&layout).unwrap();
    assert_saved_semantics(&saved);
    let reloaded = FooterLayout::from_saved(Some(&saved)).unwrap();
    assert!(
        position(&reloaded, FooterFieldId::Effort) < position(&reloaded, FooterFieldId::GitBranch)
    );
    assert!(!reloaded.fields()[position(&reloaded, FooterFieldId::Directory)].visible);
    assert!(reloaded.fields().iter().all(|field| field.visible
        == matches!(field.id, FooterFieldId::Model | FooterFieldId::GitBranch)));
    assert_saved_semantics(&serde_json::to_value(&reloaded).unwrap());
    for (id, visible, side, color) in [
        (FooterFieldId::Effort, false, FooterSide::Right, Some(FooterColor::Cyan)),
        (FooterFieldId::Model, true, FooterSide::Left, Some(FooterColor::Orange)),
        (FooterFieldId::GitBranch, true, FooterSide::Right, None),
    ] {
        assert_eq!(
            reloaded.fields()[position(&reloaded, id)],
            FooterField { id, visible, side, color }
        );
    }
}

#[test]
fn missing_empty_and_partial_preferences_have_distinct_editable_visibility() {
    let defaults = FooterLayout::from_saved(None).unwrap();
    for id in [
        FooterFieldId::PermissionMode,
        FooterFieldId::Model,
        FooterFieldId::Effort,
        FooterFieldId::Ultracode,
        FooterFieldId::FastMode,
        FooterFieldId::RunningSubagents,
        FooterFieldId::Directory,
        FooterFieldId::GitBranch,
        FooterFieldId::ContextRemaining,
    ] {
        assert!(defaults.fields()[position(&defaults, id)].visible);
    }
    for (id, side) in [
        (FooterFieldId::Model, FooterSide::Left),
        (FooterFieldId::Directory, FooterSide::Right),
        (FooterFieldId::ContextRemaining, FooterSide::Right),
    ] {
        let field = defaults.fields()[position(&defaults, id)];
        assert!(field.visible);
        assert_eq!(field.side, side);
        assert_eq!(field.color, None);
    }
    for input in [json!([]), json!([{"id":"model","visible":true,"side":"right","color":"cyan"}])] {
        let mut layout = FooterLayout::from_saved(Some(&input)).unwrap();
        for id in [
            FooterFieldId::PermissionMode,
            FooterFieldId::Model,
            FooterFieldId::Effort,
            FooterFieldId::Ultracode,
            FooterFieldId::FastMode,
            FooterFieldId::RunningSubagents,
            FooterFieldId::Directory,
            FooterFieldId::GitBranch,
            FooterFieldId::ContextRemaining,
        ] {
            let field = layout.fields()[position(&layout, id)];
            assert_eq!(
                field.visible,
                id == FooterFieldId::Model && !input.as_array().unwrap().is_empty()
            );
        }
        let before = layout.fields().to_vec();
        layout.toggle(position(&layout, FooterFieldId::Directory));
        for field in layout.fields() {
            let mut expected = *before.iter().find(|f| f.id == field.id).unwrap();
            if field.id == FooterFieldId::Directory {
                expected.visible = true;
            }
            assert_eq!(*field, expected);
        }
    }
}

#[test]
fn moving_and_switching_sides_preserve_selection_metadata_and_group_boundaries() {
    let mut layout = FooterLayout::from_saved(Some(&json!([
        {"id":"model","visible":false,"side":"left","color":"cyan"},
        {"id":"effort","visible":true,"side":"left","color":"orange"},
        {"id":"directory","visible":true,"side":"right"},
        {"id":"gitBranch","visible":false,"side":"right","color":"pink"}
    ])))
    .unwrap();
    let original = layout.fields().to_vec();
    let moved = layout.move_field(position(&layout, FooterFieldId::Model), true);
    assert_eq!(layout.fields()[moved].id, FooterFieldId::Model);
    assert!(position(&layout, FooterFieldId::Effort) < position(&layout, FooterFieldId::Model));
    let left_end = layout.fields().iter().rposition(|f| f.side == FooterSide::Left).unwrap();
    let right_start = layout.fields().iter().position(|f| f.side == FooterSide::Right).unwrap();
    let boundary = layout.clone();
    for (id, down) in [
        (layout.fields().first().unwrap().id, false),
        (layout.fields().last().unwrap().id, true),
        (layout.fields()[left_end].id, true),
        (layout.fields()[right_start].id, false),
    ] {
        let selected = layout.move_field(position(&layout, id), down);
        assert_eq!(layout.fields()[selected].id, id);
        assert_eq!(layout, boundary);
    }
    for destination in [FooterSide::Right, FooterSide::Left] {
        let moved = layout.switch_side(position(&layout, FooterFieldId::Model));
        assert_eq!(layout.fields()[moved].id, FooterFieldId::Model);
        assert_eq!(
            layout.fields().iter().rfind(|f| f.side == destination).unwrap().id,
            FooterFieldId::Model
        );
        for field in layout.fields() {
            let mut expected = *original.iter().find(|f| f.id == field.id).unwrap();
            if field.id == FooterFieldId::Model {
                expected.side = destination;
            }
            assert_eq!(*field, expected);
        }
        for side in [FooterSide::Left, FooterSide::Right] {
            let peers = |fields: &[FooterField]| {
                fields
                    .iter()
                    .filter(|f| f.side == side && f.id != FooterFieldId::Model)
                    .map(|f| f.id)
                    .collect::<Vec<_>>()
            };
            assert_eq!(peers(layout.fields()), peers(&original));
        }
    }
}
