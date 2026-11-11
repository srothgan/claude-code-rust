// SPDX-License-Identifier: Apache-2.0

use super::test_support::{has_multiple_content_fg_colors, read_tool_call, render_structured_read};
use super::*;
use crate::app::test_support::named_tool_call_info;

struct AliasCase {
    tool: ToolCallInfo,
    mode: Option<&'static str>,
    expected: &'static str,
    _response: Option<tokio::sync::oneshot::Receiver<model::RequestQuestionResponse>>,
}

fn alias_cases() -> Vec<AliasCase> {
    [
        ("Write", "WRITE_SOURCE", Some("plan"), "Create Plan"),
        ("Edit", "EDIT_SOURCE", Some("plan"), "Update Plan"),
        ("MultiEdit", "MULTI_SOURCE", Some("plan"), "Update Plan"),
        ("Read", "READ_SOURCE", Some("plan"), "READ_SOURCE"),
        ("Write", "WRITE_SOURCE", None, "WRITE_SOURCE"),
        ("Bash", "echo SHELL_ORCHID", Some("plan"), "echo SHELL_ORCHID"),
        (
            "mcp__claude_ai_Strava__list_activities",
            "mcp__claude_ai_Strava__list_activities",
            None,
            "Strava: List activities",
        ),
        ("mcp__fff__find_files", "mcp__fff__find_files", None, "fff: Find files"),
        (
            "mcp__claude_ai_Strava__list_activities",
            "Recent Strava activities",
            None,
            "Recent Strava activities",
        ),
        ("mcp__fff_find_files", "mcp__fff_find_files", None, "mcp__fff_find_files"),
    ]
    .into_iter()
    .map(|(name, title, mode, expected)| {
        let mut tool = named_tool_call_info("alias-target", name, model::ToolCallStatus::Completed);
        tool.title = title.into();
        if name == "Bash" {
            tool.terminal_command = Some(title.into());
        }
        AliasCase { tool, mode, expected, _response: None }
    })
    .collect()
}

fn question_alias_cases() -> Vec<AliasCase> {
    let mut pending = named_tool_call_info(
        "QUESTION_SOURCE",
        "AskUserQuestion",
        model::ToolCallStatus::InProgress,
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    pending.pending_question = Some(crate::app::InlineQuestion {
        idle_timeout: None,
        last_activity: std::time::Instant::now(),
        prompt: model::QuestionPrompt::new(
            "Choose a language",
            "Language",
            false,
            vec![model::QuestionOption::new("rust", "Rust")],
        ),
        response_tx: sender,
        focused_option_index: 0,
        selected_option_indices: std::collections::BTreeSet::new(),
        notes: String::new(),
        notes_cursor: 0,
        editing_notes: false,
        focused: true,
        question_index: 0,
        total_questions: 4,
    });
    let mut answered = named_tool_call_info(
        "QUESTION_SOURCE",
        "AskUserQuestion",
        model::ToolCallStatus::Completed,
    );
    answered.raw_input = Some(serde_json::json!({"question_results":[
        {"question":"First?","header":"First","selected_options":[]},
        {"question":"Second?","header":"Second","selected_options":[]},
    ]}));
    vec![
        AliasCase {
            tool: pending,
            mode: None,
            expected: "Questions (4)",
            _response: Some(receiver),
        },
        AliasCase {
            tool: answered,
            mode: None,
            expected: "Answered questions (2)",
            _response: None,
        },
    ]
}

#[test]
fn display_aliases_follow_mode_question_state_and_mcp_title_source() {
    for case in alias_cases().into_iter().chain(question_alias_cases()) {
        let context = ToolCallRenderContext { current_mode_id: case.mode, ..Default::default() };
        assert_eq!(tool_display_title(&case.tool, context), case.expected);
    }
}

fn body_texts(rows: &[Line<'_>]) -> Vec<String> {
    rows.iter()
        .map(|line| line.spans.iter().skip(1).map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn read_syntax_is_selected_from_the_preserved_path_without_changing_source() {
    for (path, source) in [
        ("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n"),
        ("Cargo.toml", "[package]\nname = \"demo\"\n"),
        ("frontend/App.tsx", "export function App() {\n    return <main>Hello</main>;\n}\n"),
        ("Dockerfile", "FROM rust:1\nRUN cargo build\n"),
    ] {
        let tool = read_tool_call(path, source);
        let rows = standard::ToolBodyCursor::default().render(
            &tool,
            120,
            &crate::ui::layout_control::LayoutControl::uninterrupted(),
        );
        assert!(has_multiple_content_fg_colors(&rows), "syntax missing for {path}");
        assert_eq!(
            body_texts(&rows),
            source.lines().collect::<Vec<_>>(),
            "literal Read source for {path}"
        );
        assert_eq!(tool.locations[0].path.to_string_lossy(), path);
    }
}

#[test]
fn read_image_status_matches_its_file_while_unrelated_source_remains_literal() {
    for (path, expected) in [
        (r"C:\work\captures\screen.png", "Viewed image screen.png"),
        ("notes/status.txt", "Viewed Image screen.png"),
    ] {
        let tool = read_tool_call(path, "Viewed Image screen.png");
        let rows = standard::ToolBodyCursor::default().render(
            &tool,
            120,
            &crate::ui::layout_control::LayoutControl::uninterrupted(),
        );
        assert_eq!(body_texts(&rows), [expected]);
        let content = body_texts(&rows).join("\n");
        assert!(content.find("Viewed").unwrap() < content.find("screen.png").unwrap());
    }
}

#[derive(Debug)]
struct SourceRow {
    number: Option<usize>,
    text: String,
    column: usize,
}

#[derive(Debug)]
enum PreviewRow {
    Source(SourceRow),
    Hidden(usize),
}

fn read_body_rows(rendered: &[Line<'static>]) -> Vec<PreviewRow> {
    rendered
        .iter()
        .skip(1)
        .map(|row| {
            // Strip the tool framing at the adapter boundary; expectations concern the source
            // column and its values rather than border glyphs, gutter padding or colors.
            let frame = row.spans.first().expect("tool frame");
            let mut content = row.spans.iter().skip(1);
            let gutter = content.next().expect("source gutter or omission notice");
            let number = gutter.content.trim().parse::<usize>().ok();
            if number.is_some() || gutter.content.trim().is_empty() {
                PreviewRow::Source(SourceRow {
                    number,
                    text: content.map(|s| s.content.as_ref()).collect(),
                    column: frame.width() + gutter.width(),
                })
            } else {
                let notice: String = row.spans.iter().skip(1).map(|s| s.content.as_ref()).collect();
                let count = notice
                    .split_whitespace()
                    .find_map(|word| word.parse().ok())
                    .expect("reported omitted source count");
                PreviewRow::Hidden(count)
            }
        })
        .collect()
}

#[derive(Debug)]
struct SourceLine {
    number: usize,
    text: String,
    columns: Vec<usize>,
}

fn source_lines(rows: &[PreviewRow]) -> Vec<SourceLine> {
    let mut source: Vec<SourceLine> = Vec::new();
    for row in rows {
        let PreviewRow::Source(row) = row else { continue };
        if let Some(number) = row.number {
            source.push(SourceLine { number, text: row.text.clone(), columns: vec![row.column] });
        } else {
            let line = source.last_mut().expect("continuation follows a numbered source line");
            line.text.push_str(&row.text);
            line.columns.push(row.column);
        }
    }
    source
}

fn assert_literal_source(tool: &ToolCallInfo, source: &str) {
    let model::ToolCallContent::Content(content) = &tool.content[0] else {
        panic!("source content")
    };
    let model::ContentBlock::Text(text) = &content.content else { panic!("literal text") };
    assert_eq!(text.text, source);
}

#[test]
fn numbered_read_keeps_literal_markdown_status_crlf_and_tab_expansion() {
    for (path, source, start, expected) in [
        (
            "src/lib.rs",
            "fn first() {}\r\nfn second() {}  \r\n",
            41,
            &[(41, "fn first() {}"), (42, "fn second() {}  ")][..],
        ),
        ("config.json", "{\"a\":1}", 7, &[(7, "{\"a\":1}")][..]),
        (
            "README.md",
            "# Heading\n\n- **Body** text\n```bash\n#!/bin/sh\n# comment\n```\n",
            1,
            &[
                (1, "# Heading"),
                (2, ""),
                (3, "- **Body** text"),
                (4, "```bash"),
                (5, "#!/bin/sh"),
                (6, "# comment"),
                (7, "```"),
            ][..],
        ),
        ("Makefile", "all:\n\tcargo build\n", 1, &[(1, "all:"), (2, "    cargo build")][..]),
        ("status.txt", "Viewed Image status.txt", 42, &[(42, "Viewed Image status.txt")][..]),
    ] {
        let (tool, rendered) = render_structured_read(path, source, start, 100);
        let parsed = read_body_rows(&rendered);
        assert!(parsed.iter().all(|row| matches!(row, PreviewRow::Source(_))));
        let lines = source_lines(&parsed);
        assert_eq!(lines.iter().map(|s| (s.number, s.text.as_str())).collect::<Vec<_>>(), expected);
        assert_literal_source(&tool, source);
    }
}

#[test]
fn wrapped_read_preserves_literal_spaces_and_continuations_share_the_source_column() {
    let first = "    let phrase = \"ALPHA  BETA  GAMMA  DELTA\";";
    let source = format!("{first}\nshort();");
    let (mut tool, _) = render_structured_read("src/main.rs", &source, 9, 30);
    for width in [30, 42, 80, 30] {
        let mut rendered = Vec::new();
        render_tool_call_cached(
            &mut tool,
            ToolCallRenderContext::default(),
            width,
            crate::ui::SpinnerState::Static,
            &mut rendered,
        );
        let parsed = read_body_rows(&rendered);
        let lines = source_lines(&parsed);
        assert_eq!(
            lines.iter().map(|line| (line.number, line.text.as_str())).collect::<Vec<_>>(),
            [(9, first), (10, "short();")]
        );
        if width == 30 {
            assert!(lines[0].columns.len() > 1, "narrow fixture must actually wrap");
        }
        let column = lines[0].columns[0];
        assert!(
            lines.iter().flat_map(|line| &line.columns).all(|&actual| actual == column),
            "source and continuation association: {lines:?}"
        );
        assert!(rendered.iter().all(|row| row.width() <= usize::from(width)));
        assert_literal_source(&tool, &source);
    }
}

#[test]
fn read_preview_counts_omitted_source_lines_before_wrapping_and_keeps_head_and_tail() {
    let source = (1..=20).map(|n| format!("L{n}")).collect::<Vec<_>>().join("\n");
    for width in [15, 20, 30] {
        let (_, rendered) = render_structured_read("notes.txt", &source, 1, width);
        assert_visible_preview_window(
            &rendered,
            &[(1, "L1"), (2, "L2"), (3, "L3"), (17, "L17"), (18, "L18"), (19, "L19"), (20, "L20")],
        );
        assert!(rendered.iter().all(|row| row.width() <= usize::from(width)));
    }
    let mut lines = (1..=20).map(|n| format!("line {n}")).collect::<Vec<_>>();
    lines[1] = "line 2 is long enough to wrap onto a second row".into();
    let (_, rendered) = render_structured_read("notes.txt", &lines.join("\n"), 1, 40);
    assert_visible_preview_window(
        &rendered,
        &[
            (1, "line 1"),
            (2, "line 2 is long enough to wrap onto a second row"),
            (3, "line 3"),
            (17, "line 17"),
            (18, "line 18"),
            (19, "line 19"),
            (20, "line 20"),
        ],
    );
    assert!(source_lines(&read_body_rows(&rendered))[1].columns.len() > 1);
}

fn assert_visible_preview_window(rendered: &[Line<'static>], expected: &[(usize, &str)]) {
    let rows = read_body_rows(rendered);
    assert_eq!(
        rows.iter()
            .filter_map(|row| if let PreviewRow::Hidden(n) = row { Some(*n) } else { None })
            .collect::<Vec<_>>(),
        [13]
    );
    let lines = source_lines(&rows);
    assert_eq!(
        lines.iter().map(|line| (line.number, line.text.as_str())).collect::<Vec<_>>(),
        expected
    );
    let hidden = rows.iter().position(|row| matches!(row, PreviewRow::Hidden(_))).unwrap();
    assert!(matches!(&rows[hidden-1],PreviewRow::Source(row) if row.number==Some(3)));
    assert!(matches!(&rows[hidden+1],PreviewRow::Source(row) if row.number==Some(17)));
}

#[test]
fn oversized_read_lines_each_remain_visible_and_report_their_own_clipping() {
    let source = format!("{}\n{}", "a".repeat(2_000), "b".repeat(2_000));
    let (tool, rendered) = render_structured_read("bundle.min.js", &source, 1, 40);
    let rows = read_body_rows(&rendered);
    assert!(rows.len() <= TOOL_BODY_MAX_LINES);
    assert!(rows.iter().all(|row| matches!(row, PreviewRow::Source(_))));
    let lines = source_lines(&rows);
    assert_eq!(lines.iter().map(|line| line.number).collect::<Vec<_>>(), [1, 2]);
    for (line, expected) in lines.iter().zip(['a', 'b']) {
        let shown = line.text.strip_suffix('…').expect("clipped line must disclose truncation");
        assert!(!shown.is_empty() && shown.len() < 2_000);
        assert!(shown.chars().all(|c| c == expected), "each line keeps its own source: {line:?}");
    }
    assert!(rendered.iter().all(|row| row.width() <= 40));
    assert_literal_source(&tool, &source);
}

#[test]
fn unstructured_read_does_not_consume_numeric_source_prefixes_as_line_numbers() {
    let tool = read_tool_call("notes.txt", "1st line\n2nd line");
    let rendered = standard::ToolBodyCursor::default().render(
        &tool,
        80,
        &crate::ui::layout_control::LayoutControl::uninterrupted(),
    );
    assert_eq!(body_texts(&rendered), ["1st line", "2nd line"]);
}
