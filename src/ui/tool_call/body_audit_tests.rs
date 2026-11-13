// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::test_support::named_tool_call_info;
use std::fmt::Write as _;

fn body(tool: &ToolCallInfo, width: u16) -> Vec<Line<'static>> {
    standard::ToolBodyCursor::default().render(
        tool,
        width,
        &crate::ui::layout_control::LayoutControl::uninterrupted(),
    )
}

fn text(rows: &[Line<'_>]) -> Vec<String> {
    rows.iter().map(ToString::to_string).collect()
}

#[test]
fn completed_question_previews_and_notes_follow_their_own_answers() {
    let mut tool = named_tool_call_info(
        "question-outcomes",
        "AskUserQuestion",
        model::ToolCallStatus::Completed,
    );
    tool.raw_input = Some(serde_json::json!({"question_results":[
        {"question":"QUESTION_ORCHID","header":"ORCHID","question_index":0,"total_questions":2,
         "selected_options":[{"option_id":"json","label":"ANSWER_JSON","description":"JSON_DESCRIPTION"}],
         "annotation":{"preview":"{\n  \"ORCHID_VALUE\": true\n}","notes":"ORCHID_NOTES"}},
        {"question":"QUESTION_JASMINE","header":"JASMINE","question_index":1,"total_questions":2,
         "selected_options":[{"option_id":"toml","label":"ANSWER_TOML","description":"TOML_DESCRIPTION"}],
         "annotation":{"preview":"JASMINE_PREVIEW","notes":"JASMINE_NOTES"}},
    ]}));
    for width in [60, 120] {
        let rendered = text(&body(&tool, width));
        let first = rendered.iter().position(|r| r.contains("QUESTION_ORCHID")).unwrap();
        let second = rendered.iter().position(|r| r.contains("QUESTION_JASMINE")).unwrap();
        for (rows, answer, description, preview, notes, other) in [
            (
                &rendered[first..second],
                "ANSWER_JSON",
                "JSON_DESCRIPTION",
                "ORCHID_VALUE",
                "ORCHID_NOTES",
                "JASMINE_",
            ),
            (
                &rendered[second..],
                "ANSWER_TOML",
                "TOML_DESCRIPTION",
                "JASMINE_PREVIEW",
                "JASMINE_NOTES",
                "ORCHID_",
            ),
        ] {
            let at = |value: &str| {
                rows.iter()
                    .position(|r| r.contains(value))
                    .unwrap_or_else(|| panic!("missing {value}: {rows:?}"))
            };
            assert!(at(answer) <= at(description));
            assert!(at(answer) < at(preview) && at(preview) < at(notes));
            assert!(
                !rows.iter().any(|r| r.contains(other)),
                "annotation crossed answer boundaries"
            );
        }
    }
}

#[test]
fn saved_resource_paths_appear_once_with_their_own_readable_content() {
    let path = r"C:\tmp\ORCHID_MANUAL.pdf";
    for source in ["RESOURCE_DOWNLOADED".to_owned(), format!("RESOURCE_DOWNLOADED Saved to {path}")]
    {
        let mut tool = named_tool_call_info(
            "resource-target",
            "ReadMcpResource",
            model::ToolCallStatus::Completed,
        );
        tool.content = vec![model::ToolCallContent::McpResource(
            model::McpResource::new("file://manual.pdf")
                .mime_type(Some("application/pdf".into()))
                .text(Some(source))
                .blob_saved_to(Some(path.into())),
        )];
        let rendered = text(&body(&tool, 120)).join("\n");
        assert_eq!(rendered.matches(path).count(), 1, "saved location must be shown exactly once");
        assert!(rendered.contains("RESOURCE_DOWNLOADED"));
    }
}

fn diff_tool(alpha: String, beta_old: Option<&str>) -> ToolCallInfo {
    let mut tool =
        named_tool_call_info("repository-target", "Write", model::ToolCallStatus::Completed);
    tool.content = vec![
        model::ToolCallContent::Diff(
            model::Diff::new("alpha.rs", alpha).repository(Some("alpha/repo".into())),
        ),
        model::ToolCallContent::Diff(
            model::Diff::new("beta.rs", "BETA_NEW")
                .old_text(beta_old)
                .repository(Some("beta/repo".into())),
        ),
    ];
    tool
}

fn repository_regions<'a>(rows: &'a [Line<'static>]) -> (&'a [Line<'static>], &'a [Line<'static>]) {
    let first = rows.iter().position(|r| r.to_string().contains("alpha/repo")).unwrap();
    let second = rows.iter().position(|r| r.to_string().contains("beta/repo")).unwrap();
    assert!(first < second);
    (&rows[first..second], &rows[second..])
}

#[test]
fn capped_diff_sources_and_counts_stay_with_their_repository() {
    let mut alpha = (0..79).fold(String::new(), |mut source, n| {
        writeln!(&mut source, "ALPHA_{n:02}_long_wrapped_source_ending").unwrap();
        source
    });
    alpha.push_str("ALPHA_LAST\n");
    let tool = diff_tool(alpha, Some("BETA_OLD"));
    for width in [48, 100] {
        let rows = body(&tool, width);
        let (first, second) = repository_regions(&rows);
        let first = text(first).join("\n");
        let second = text(second).join("\n");
        assert!(first.contains("+80") && first.contains("ALPHA_LAST"));
        assert!(second.contains("+1") && second.contains("-1"));
        assert!(second.contains("BETA_NEW") && second.contains("BETA_OLD"));
        assert!(!first.contains("BETA_") && !second.contains("ALPHA_"));
        assert!(rows.iter().all(|r| r.width() <= usize::from(width)));
    }
}

#[test]
fn wrapped_diff_source_keeps_literal_spaces_and_indentation_relative_to_plain_source() {
    let source = "        alpha  beta gamma delta epsilon zeta END";
    let tool = diff_tool(source.into(), None);
    for width in [40, 60] {
        let rows = body(&tool, width);
        let (first, second) = repository_regions(&rows);
        let first = text(first);
        let second = text(second);
        let neighbor = second.iter().find(|r| r.contains("BETA_NEW")).unwrap();
        let source_column =
            unicode_width::UnicodeWidthStr::width(&neighbor[..neighbor.find("BETA_NEW").unwrap()]);
        let start = first.iter().position(|r| r.contains("alpha  beta")).unwrap();
        let alpha_column = unicode_width::UnicodeWidthStr::width(
            &first[start][..first[start].find("alpha").unwrap()],
        );
        assert_eq!(
            alpha_column,
            source_column + 8,
            "source indentation belongs inside the shared diff column"
        );
        // These fixtures use ASCII source and single-cell framing. Strip only the shared
        // source-column prefix and the repeated hanging indentation on continuation rows.
        let mut recovered = first[start].chars().skip(source_column).collect::<String>();
        let mut continuations = 0;
        for row in &first[start + 1..] {
            let fragment = row.chars().skip(source_column).collect::<String>();
            if fragment.trim().is_empty() {
                continue;
            }
            recovered.push_str(
                fragment
                    .strip_prefix("        ")
                    .expect("continuation preserves source indentation"),
            );
            continuations += 1;
        }
        if width == 40 {
            assert!(continuations > 0, "fixture must actually wrap");
        }
        assert_eq!(recovered.trim_end(), source);
        assert!(!first.iter().any(|r| r.contains("BETA_NEW")));
        assert!(!second.iter().any(|r| r.contains("alpha  beta")));
        assert!(rows.iter().all(|r| r.width() <= usize::from(width)));
    }
}
