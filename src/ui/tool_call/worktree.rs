// SPDX-License-Identifier: Apache-2.0

//! Rendering helpers for SDK worktree context tools.

use crate::app::ToolCallInfo;
use ratatui::text::Line;

use super::fields::{self, ToolField};
use super::typed;

pub(super) fn is_worktree_tool(tc: &ToolCallInfo) -> bool {
    matches!(tc.sdk_tool_name.as_str(), "EnterWorktree" | "ExitWorktree")
}

pub(super) fn has_structured_body(tc: &ToolCallInfo) -> bool {
    match tc.sdk_tool_name.as_str() {
        "EnterWorktree" => typed::input_string(tc, "path").is_some() || !tc.content.is_empty(),
        "ExitWorktree" => {
            typed::input_string(tc, "action").is_some()
                || typed::input_bool(tc, "discard_changes") == Some(true)
                || !tc.content.is_empty()
        }
        _ => false,
    }
}

pub(super) fn render_tool_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut lines = match tc.sdk_tool_name.as_str() {
        "EnterWorktree" => render_enter_content(tc),
        "ExitWorktree" => render_exit_content(tc),
        _ => return Vec::new(),
    };
    lines.extend(typed::render_stripped_text_blocks(tc, render_text_line));
    lines
}

fn render_enter_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let fields = typed::input_string(tc, "path").map(|path| ToolField::new("Path", path));
    fields::render_fields(fields)
}

fn render_exit_content(tc: &ToolCallInfo) -> Vec<Line<'static>> {
    let mut fields = Vec::new();
    if let Some(action) = typed::input_string(tc, "action") {
        fields.push(ToolField::new("Action", action));
    }
    if typed::input_bool(tc, "discard_changes") == Some(true) {
        fields.push(ToolField::new("Discard changes", "yes"));
    }
    fields::render_fields(fields)
}

fn render_text_line(line: &str) -> Line<'static> {
    typed::render_colon_field_line(line, field_label, |_, value| value.to_owned())
}

fn field_label(label: &str) -> Option<&'static str> {
    match label {
        "Action" => Some("Action"),
        "Branch" => Some("Branch"),
        "Discard changes" => Some("Discard changes"),
        "Path" => Some("Path"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::ui::tool_call::test_support::tool_call;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn worktree_tool_call(
        sdk_tool_name: &str,
        raw_input: serde_json::Value,
        content: Option<&str>,
    ) -> ToolCallInfo {
        tool_call(
            "tc-worktree",
            "tc-worktree",
            sdk_tool_name,
            raw_input,
            content,
            model::ToolCallStatus::Completed,
        )
    }

    #[test]
    fn enter_worktree_cards_associate_requested_paths_and_returned_locations() {
        use crate::ui::tool_call::test_support::card_body;
        for (requested, output, status, expected) in [
            (
                "/repo/worktrees/feature one",
                None,
                model::ToolCallStatus::Pending,
                "Path: /repo/worktrees/feature one",
            ),
            (
                "/repo/worktrees/feature two",
                Some("Branch: actual-branch-two"),
                model::ToolCallStatus::Completed,
                "Branch: actual-branch-two",
            ),
            (
                "/repo/worktrees/feature three",
                Some("Path: /actual/worktree three"),
                model::ToolCallStatus::Completed,
                "Path: /actual/worktree three",
            ),
        ] {
            let mut tc = worktree_tool_call("EnterWorktree", json!({"path":requested}), output);
            tc.status = status;
            let rows = card_body(&mut tc, 120);
            assert!(rows.contains(&format!("Path: {requested}")), "{rows:?}");
            assert!(rows.contains(&expected.to_owned()), "{rows:?}");
            if !expected.contains("branch-two") {
                assert!(!rows.iter().any(|row| row.contains("actual-branch-two")));
            }
        }
    }

    #[test]
    fn exit_worktree_request_cards_distinguish_intent_from_confirmed_discards() {
        use crate::ui::tool_call::test_support::card_body;
        for (action, discard) in [
            ("keep", None),
            ("keep", Some(false)),
            ("remove", None),
            ("remove", Some(false)),
            ("remove", Some(true)),
        ] {
            let mut input = json!({"action":action});
            if let Some(discard) = discard {
                input["discard_changes"] = json!(discard);
            }
            let mut tc = worktree_tool_call("ExitWorktree", input, None);
            tc.status = model::ToolCallStatus::Pending;
            let rows = card_body(&mut tc, 100);
            assert!(rows.contains(&format!("Action: {action}")), "{rows:?}");
            assert_eq!(rows.contains(&"Discard changes: yes".to_owned()), discard == Some(true));
            assert!(
                !rows
                    .iter()
                    .any(|row| row.starts_with("Discarded")
                        || row.starts_with("Restored directory:"))
            );
        }
    }

    #[test]
    fn confirmed_worktree_receipts_preserve_action_locations_and_known_discard_counts() {
        use crate::ui::tool_call::test_support::{card_body, confirmed_receipt};
        for (id, action, path, files, commits) in [
            ("worktree-keep", "keep", "kept", None, None),
            ("worktree-remove-zero", "remove", "zero", Some("0"), Some("7")),
            ("worktree-remove-counts", "remove", "many", Some("5"), Some("2")),
        ] {
            let mut tc = confirmed_receipt(id);
            let rows = card_body(&mut tc, 120);
            assert!(rows.contains(&format!("Action: {action}")), "{rows:?}");
            assert_eq!(rows.iter().filter(|row| row.starts_with("Action:")).count(), 1);
            assert!(rows.contains(&format!("Worktree: /fixture/worktree/{path}")));
            assert!(rows.contains(&format!("Restored directory: /fixture/original/{path}")));
            for (label, value) in [("Discarded files", files), ("Discarded commits", commits)] {
                let fields: Vec<_> = rows
                    .iter()
                    .filter(|row| row.starts_with(&format!("{label}:")))
                    .cloned()
                    .collect();
                assert_eq!(
                    fields,
                    value.map(|value| format!("{label}: {value}")).into_iter().collect::<Vec<_>>()
                );
            }
            if action == "keep" {
                assert!(rows.contains(&"Branch: retained-branch".to_owned()));
            }
        }
        let mut tc = worktree_tool_call(
            "ExitWorktree",
            json!({"action":"remove","discard_changes":true}),
            Some("WORKTREE-REMOVE-REFUSED"),
        );
        tc.status = model::ToolCallStatus::Failed;
        let rows = card_body(&mut tc, 120);
        assert!(rows.contains(&"WORKTREE-REMOVE-REFUSED".to_owned()));
        assert!(
            !rows
                .iter()
                .any(|row| row.starts_with("Discarded") || row.starts_with("Restored directory:"))
        );
    }
}
