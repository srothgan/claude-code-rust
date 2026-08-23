// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::BlockCache;
use pretty_assertions::assert_eq;
use std::fmt::Write as _;
fn test_tool_call(id: &str, sdk_tool_name: &str, status: model::ToolCallStatus) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_owned(),
        source_message_uuids: Vec::new(),
        title: id.to_owned(),
        sdk_tool_name: sdk_tool_name.to_owned(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status,
        content: Vec::new(),
        hidden: false,
        terminal_id: None,
        terminal_command: None,
        terminal_output: None,
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    }
}

fn rendered_line_texts(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect()).collect()
}

fn rendered_line_texts_trimmed(lines: &[Line<'static>]) -> Vec<String> {
    rendered_line_texts(lines).into_iter().map(|line| line.trim_end().to_owned()).collect()
}

fn has_multiple_content_fg_colors(lines: &[Line<'static>]) -> bool {
    let mut colors = Vec::new();
    for color in
        lines.iter().flat_map(|line| line.spans.iter().skip(1)).filter_map(|span| span.style.fg)
    {
        if !colors.contains(&color) {
            colors.push(color);
        }
    }
    colors.len() > 1
}

fn read_tool_call(path: &str, text: &str) -> ToolCallInfo {
    let mut tc = test_tool_call("Read opaque-title", "Read", model::ToolCallStatus::Completed);
    tc.locations = vec![model::ToolCallLocation::new(path)];
    tc.content = vec![model::ToolCallContent::from(text.to_owned())];
    tc
}

fn assert_read_path_is_syntax_colored(path: &str, text: &str) {
    let tc = read_tool_call(path, text);
    let body = standard::render_tool_call_body(&tc, 120);
    assert!(has_multiple_content_fg_colors(&body), "expected syntax coloring for path {path:?}");
}

// status_icon

#[test]
fn status_icon_pending() {
    let (icon, color) = status_icon(model::ToolCallStatus::Pending, 0);
    assert!(!icon.is_empty());
    assert_eq!(color, theme::RUST_ORANGE);
}

#[test]
fn status_icon_in_progress() {
    let (icon, color) = status_icon(model::ToolCallStatus::InProgress, 3);
    assert!(!icon.is_empty());
    assert_eq!(color, theme::RUST_ORANGE);
}

#[test]
fn status_icon_completed() {
    let (icon, color) = status_icon(model::ToolCallStatus::Completed, 0);
    assert_eq!(icon, theme::ICON_COMPLETED);
    assert_eq!(color, theme::RUST_ORANGE);
}

#[test]
fn status_icon_failed() {
    let (icon, color) = status_icon(model::ToolCallStatus::Failed, 0);
    assert_eq!(icon, theme::ICON_FAILED);
    assert_eq!(color, theme::STATUS_ERROR);
}

#[test]
fn status_icon_killed() {
    let (icon, color) = status_icon(model::ToolCallStatus::Killed, 0);
    assert_eq!(icon, theme::ICON_FAILED);
    assert_eq!(color, theme::STATUS_ERROR);
}

#[test]
fn status_icon_spinner_wraps() {
    let (icon_a, _) = status_icon(model::ToolCallStatus::InProgress, 0);
    let (icon_b, _) = status_icon(model::ToolCallStatus::InProgress, SPINNER_STRS.len());
    assert_eq!(icon_a, icon_b);
}

#[test]
fn status_icon_all_spinner_frames_valid() {
    for i in 0..SPINNER_STRS.len() {
        let (icon, _) = status_icon(model::ToolCallStatus::InProgress, i);
        assert!(!icon.is_empty());
    }
}

/// Spinner frames are all distinct.
#[test]
fn status_icon_spinner_frames_distinct() {
    let frames: Vec<&str> = (0..SPINNER_STRS.len())
        .map(|i| status_icon(model::ToolCallStatus::InProgress, i).0)
        .collect();
    for i in 0..frames.len() {
        for j in (i + 1)..frames.len() {
            assert_ne!(frames[i], frames[j], "frames {i} and {j} are identical");
        }
    }
}

/// Large spinner frame number wraps correctly.
#[test]
fn status_icon_spinner_large_frame() {
    let (icon, _) = status_icon(model::ToolCallStatus::Pending, 999_999);
    assert!(!icon.is_empty());
}

#[test]
fn truncate_spans_adds_ellipsis_when_needed() {
    let spans = vec![Span::raw("abcdefghijklmnopqrstuvwxyz")];
    let out = truncate_spans_to_width(spans, 8);
    let rendered: String = out.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(rendered, "abcdefg\u{2026}");
    assert!(spans_width(&out) <= 8);
}

#[test]
fn markdown_inline_spans_removes_markdown_syntax() {
    let spans = markdown_inline_spans("**Allow** _once_");
    let rendered: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(rendered.contains("Allow"));
    assert!(rendered.contains("once"));
    assert!(!rendered.contains('*'));
    assert!(!rendered.contains('_'));
}

#[test]
fn render_tool_call_title_shows_backgrounded_badge() {
    let mut tc = test_tool_call("tc-bg", "Agent", model::ToolCallStatus::InProgress);
    tc.task_metadata = Some(model::TaskMetadata::new().backgrounded(Some(true)));

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);
    let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

    assert!(rendered.contains("[backgrounded]"));
}

#[test]
fn render_agent_title_shows_spawn_depth_with_background_state() {
    let mut tc = test_tool_call("tc-depth", "Agent", model::ToolCallStatus::InProgress);
    tc.task_metadata =
        Some(model::TaskMetadata::new().spawn_depth(Some(3)).backgrounded(Some(true)));

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(rendered.contains("[depth: 3]"));
    assert!(rendered.contains("[backgrounded]"));

    let narrow = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 20, 0);
    assert!(!narrow.spans.is_empty());
}

#[test]
fn render_unrelated_tool_title_omits_task_spawn_depth() {
    let mut tc = test_tool_call("tc-read-depth", "Read", model::ToolCallStatus::Completed);
    tc.task_metadata = Some(model::TaskMetadata::new().spawn_depth(Some(4)));

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(!rendered.contains("[depth:"));
}

#[test]
fn render_skill_title_shows_backgrounded_badge() {
    let mut tc = test_tool_call("tc-skill-bg", "Skill", model::ToolCallStatus::Completed);
    tc.output_metadata = Some(
        model::ToolOutputMetadata::new()
            .skill(Some(model::SkillOutputMetadata::new().background(Some(true)))),
    );

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(rendered.contains("[backgrounded]"));
}

#[test]
fn render_skill_title_uses_dedicated_icon_and_named_title() {
    let mut tc = test_tool_call("tc-skill", "Skill", model::ToolCallStatus::Completed);
    tc.title = "Skill: Frontend Design".to_owned();

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(rendered.contains("\u{2726} Skill: Frontend Design"));
    assert!(!rendered.contains("\u{25cb}"));
}

#[test]
fn render_tool_call_preserves_non_execution_reason_and_feedback() {
    let mut tc = test_tool_call("tc-rejected", "Bash", model::ToolCallStatus::Failed);
    tc.output_metadata = Some(
        model::ToolOutputMetadata::new().non_execution(Some(
            model::ToolNonExecutionMetadata::new("user-rejected")
                .user_feedback(Some("Use a read-only command.".to_owned())),
        )),
    );

    let mut rendered = Vec::new();
    render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 100, 0, &mut rendered);
    let text = rendered
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("Not executed: rejected by the user"));
    assert!(text.contains("Feedback: Use a read-only command."));
}

#[test]
fn render_tool_call_title_shows_resolved_model_badge_for_subagents() {
    let mut tc = test_tool_call("reviewer", "Agent", model::ToolCallStatus::Completed);
    tc.output_metadata = Some(model::ToolOutputMetadata::new().agent(Some(
        model::AgentOutputMetadata::new().resolved_model(Some("claude-sonnet-4-7".to_owned())),
    )));

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(rendered.contains("reviewer"));
    assert!(rendered.contains("[model: claude-sonnet-4-7]"));
}

#[test]
fn render_tool_call_title_shows_ordered_model_route_after_subagent_swap() {
    let mut tc = test_tool_call("reviewer", "Agent", model::ToolCallStatus::Completed);
    tc.raw_input = Some(serde_json::json!({ "model": "opus" }));
    tc.output_metadata = Some(
        model::ToolOutputMetadata::new().agent(Some(
            model::AgentOutputMetadata::new()
                .resolved_model(Some("claude-sonnet-4-7".to_owned()))
                .models_used(Some(vec![
                    "claude-opus-4-8".to_owned(),
                    "claude-sonnet-4-7".to_owned(),
                ])),
        )),
    );

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 160, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(rendered.contains("[models: claude-opus-4-8 -> claude-sonnet-4-7]"));
    assert!(!rendered.contains("[model:"));
}

#[test]
fn render_tool_call_title_shows_transient_subagent_retry() {
    let mut tc = test_tool_call("reviewer", "Agent", model::ToolCallStatus::InProgress);
    tc.task_metadata = Some(model::TaskMetadata::new().subagent_retry(Some(
        model::SubagentRetryUpdate::Waiting {
            agent_id: Some("agent-1".to_owned()),
            attempt: 2,
            max_retries: 4,
            retry_delay_ms: 1_500,
            error_status: Some(429),
            error_category: Some("rate_limit".to_owned()),
        },
    )));

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
    assert!(rendered.contains("[retry 2/4 in 1.5s]"));

    tc.status = model::ToolCallStatus::Completed;
    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
    assert!(!rendered.contains("[retry"));
}

#[test]
fn render_tool_call_title_shows_running_agent_type_and_requested_model_from_input() {
    let mut tc = test_tool_call("Agent: review-worker", "Agent", model::ToolCallStatus::InProgress);
    tc.raw_input = Some(serde_json::json!({
        "name": "review-worker",
        "subagent_type": "general-purpose",
        "model": "opus",
    }));

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 120, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(rendered.contains("Agent: review-worker"));
    assert!(rendered.contains("[type: general-purpose]"));
    assert!(rendered.contains("[model: opus]"));
}

#[test]
fn render_tool_call_title_prefers_resolved_model_over_requested_model() {
    let mut tc = test_tool_call("Agent: review-worker", "Agent", model::ToolCallStatus::Completed);
    tc.raw_input = Some(serde_json::json!({
        "name": "review-worker",
        "subagent_type": "general-purpose",
        "model": "opus",
    }));
    tc.output_metadata = Some(model::ToolOutputMetadata::new().agent(Some(
        model::AgentOutputMetadata::new().resolved_model(Some("claude-opus-4-8".to_owned())),
    )));

    let line = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 120, 0);
    let rendered: String = line.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(rendered.contains("[type: general-purpose]"));
    assert!(rendered.contains("[model: claude-opus-4-8]"));
    assert!(!rendered.contains("[model: opus]"));
}

#[test]
fn tool_display_title_uses_plan_aliases() {
    let write = test_tool_call("tc-plan-write", "Write", model::ToolCallStatus::Completed);
    let edit = test_tool_call("tc-plan-edit", "Edit", model::ToolCallStatus::Completed);
    let read = test_tool_call("tc-plan-read", "Read", model::ToolCallStatus::Completed);
    let plan = ToolCallRenderContext { current_mode_id: Some("plan") };

    assert_eq!(tool_display_title(&write, plan), "Create Plan");
    assert_eq!(tool_display_title(&edit, plan), "Update Plan");
    assert_eq!(tool_display_title(&read, plan), "tc-plan-read");
}

#[test]
fn tool_display_title_uses_stable_question_title_for_pending_ask_user_question() {
    let mut tc = test_tool_call(
        "What is your favorite language?",
        "AskUserQuestion",
        model::ToolCallStatus::InProgress,
    );
    let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
    tc.pending_question = Some(crate::app::InlineQuestion {
        prompt: model::QuestionPrompt::new(
            "What is your favorite language?",
            "Language",
            false,
            vec![model::QuestionOption::new("rust", "Rust")],
        ),
        response_tx,
        focused_option_index: 0,
        selected_option_indices: std::collections::BTreeSet::new(),
        notes: String::new(),
        notes_cursor: 0,
        editing_notes: false,
        focused: true,
        question_index: 0,
        total_questions: 4,
    });

    assert_eq!(tool_display_title(&tc, ToolCallRenderContext::default()), "Questions (4)");
}

#[test]
fn tool_display_title_uses_answered_questions_title_for_completed_ask_user_question() {
    let mut tc =
        test_tool_call("Which config format?", "AskUserQuestion", model::ToolCallStatus::Completed);
    tc.raw_input = Some(serde_json::json!({
        "question_results": [
            { "question": "First?", "header": "First", "selected_options": [] },
            { "question": "Second?", "header": "Second", "selected_options": [] }
        ]
    }));

    assert_eq!(tool_display_title(&tc, ToolCallRenderContext::default()), "Answered questions (2)");
}

#[test]
fn tool_display_title_formats_raw_mcp_titles() {
    let tc = test_tool_call(
        "mcp__claude_ai_Strava__list_activities",
        "mcp__claude_ai_Strava__list_activities",
        model::ToolCallStatus::Completed,
    );

    assert_eq!(
        tool_display_title(&tc, ToolCallRenderContext::default()),
        "Strava: List activities"
    );
}

#[test]
fn tool_display_title_keeps_non_raw_mcp_titles() {
    let mut tc = test_tool_call(
        "mcp__claude_ai_Strava__list_activities",
        "mcp__claude_ai_Strava__list_activities",
        model::ToolCallStatus::Completed,
    );
    tc.title = "Recent Strava activities".to_owned();

    assert_eq!(
        tool_display_title(&tc, ToolCallRenderContext::default()),
        "Recent Strava activities"
    );
}

#[test]
fn tool_display_title_keeps_malformed_mcp_titles_raw() {
    let tc = test_tool_call(
        "mcp__fff_find_files",
        "mcp__fff_find_files",
        model::ToolCallStatus::Completed,
    );

    assert_eq!(tool_display_title(&tc, ToolCallRenderContext::default()), "mcp__fff_find_files");
}

#[test]
fn standard_title_uses_plan_alias_for_write() {
    let tc = test_tool_call("Write notes/plan.md", "Write", model::ToolCallStatus::Completed);

    let rendered = standard::render_tool_call_title(
        &tc,
        ToolCallRenderContext { current_mode_id: Some("plan") },
        80,
        0,
    );
    let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(text.contains("Create Plan"));
    assert!(!text.contains("Write notes/plan.md"));
}

#[test]
fn standard_title_uses_generic_icon_for_unknown_tools() {
    let tc = test_tool_call("tc-unknown", "UnknownFutureTool", model::ToolCallStatus::Completed);

    let rendered = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);

    assert_eq!(rendered.spans.get(1).map(|span| span.content.as_ref()), Some("\u{25cb} "));
}

#[test]
fn generic_renderer_preserves_refresh_mcp_tools_json_output() {
    let mut tc =
        test_tool_call("RefreshMcpTools", "RefreshMcpTools", model::ToolCallStatus::Completed);
    let payload = r#"[{"server":"docs","status":"refreshed","toolCount":3,"added":["lookup"],"removed":["legacy_lookup"]}]"#;
    tc.content = vec![model::ToolCallContent::from(payload.to_owned())];

    let body = standard::render_tool_call_body(&tc, 300);
    let rendered = rendered_line_texts_trimmed(&body).join("\n");

    assert!(rendered.contains(payload));
}

#[test]
fn standard_title_uses_mcp_icon_and_readable_title() {
    let tc = test_tool_call(
        "mcp__fff__find_files",
        "mcp__fff__find_files",
        model::ToolCallStatus::Completed,
    );

    let rendered = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 80, 0);
    let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();

    assert_eq!(rendered.spans.get(1).map(|span| span.content.as_ref()), Some("\u{232c} "));
    assert!(text.contains("fff: Find files"));
    assert!(!text.contains("mcp__fff__find_files"));
}

#[test]
fn read_body_uses_preserved_location_for_rust_syntax() {
    assert_read_path_is_syntax_colored("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n");
}

#[test]
fn read_body_highlights_toml_from_two_face_syntaxes() {
    assert_read_path_is_syntax_colored("Cargo.toml", "[package]\nname = \"demo\"\n");
}

#[test]
fn read_body_highlights_tsx_from_two_face_syntaxes() {
    assert_read_path_is_syntax_colored(
        "frontend/App.tsx",
        "export function App() {\n    return <main>Hello</main>;\n}\n",
    );
}

#[test]
fn read_body_highlights_dockerfile_by_filename() {
    assert_read_path_is_syntax_colored("Dockerfile", "FROM rust:1\nRUN cargo build\n");
}

#[test]
fn read_body_strips_ansi_before_syntax_coloring() {
    let tc = read_tool_call("src/main.rs", "\u{1b}[31mfn\u{1b}[0m main() {}\n");
    let body = standard::render_tool_call_body(&tc, 120);
    let rendered = rendered_line_texts(&body).join("\n");

    assert!(rendered.contains("fn main"));
    assert!(!rendered.contains('\u{1b}'));
    assert!(has_multiple_content_fg_colors(&body));
}

#[test]
fn read_image_body_distinguishes_status_from_filename() {
    let tc = read_tool_call(r"C:\work\captures\screen.png", "Viewed Image screen.png");
    let body = standard::render_tool_call_body(&tc, 120);

    assert_eq!(
        rendered_line_texts_trimmed(&body),
        vec!["  \u{2514}\u{2500} Viewed image screen.png"]
    );
    assert_eq!(body.len(), 1);
    assert!(body[0].spans.len() >= 4);

    let status = &body[0].spans[1];
    assert_eq!(status.content.as_ref(), "Viewed image");
    assert_eq!(status.style.fg, Some(theme::DIM));
    assert!(status.style.add_modifier.contains(Modifier::ITALIC));

    let separator = &body[0].spans[2];
    assert_eq!(separator.content.as_ref(), " ");

    let file_name = &body[0].spans[3];
    assert_eq!(file_name.content.as_ref(), "screen.png");
    assert_eq!(file_name.style.fg, Some(Color::White));
    assert!(!file_name.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn read_body_does_not_style_unrelated_text_as_an_image_result() {
    let tc = read_tool_call("notes/status.txt", "Viewed Image screen.png");
    let body = standard::render_tool_call_body(&tc, 120);

    assert_eq!(
        rendered_line_texts_trimmed(&body),
        vec!["  \u{2514}\u{2500} Viewed Image screen.png"]
    );
    assert!(!body[0].spans.iter().any(|span| span.content.as_ref() == "Viewed image"));
}

#[test]
fn read_body_renders_sdk_line_numbers_as_dim_gutter() {
    let tc = read_tool_call("Cargo.toml", "1[package]\n2name = \"demo\"\n3version = \"0.1.0\"");
    let body = standard::render_tool_call_body(&tc, 120);
    let rendered = rendered_line_texts(&body);

    assert!(rendered[0].contains("1  [package]"));
    assert!(rendered[1].contains("2  name = \"demo\""));
    assert!(
        body[0]
            .spans
            .iter()
            .any(|span| span.content.contains('1') && span.style.fg == Some(theme::DIM))
    );
}

#[test]
fn shell_terminal_output_still_renders_ansi_sequences() {
    let rendered = crate::ui::highlight::render_terminal_output("\u{1b}[31mred\u{1b}[0m plain");

    assert_eq!(rendered.len(), 1);
    assert_eq!(rendered[0].spans[0].content.as_ref(), "red");
    assert_eq!(rendered[0].spans[0].style.fg, Some(Color::Red));
}

#[test]
fn mcp_resource_text_is_not_recolored_from_read_path() {
    let mut tc = read_tool_call("src/main.rs", "");
    tc.content = vec![model::ToolCallContent::McpResource(
        model::McpResource::new("mcp://resource").text(Some("fn main() {}\n".to_owned())),
    )];

    let body = standard::render_tool_call_body(&tc, 120);
    let rendered = rendered_line_texts(&body).join("\n");

    assert!(rendered.contains("fn main"));
    assert!(!has_multiple_content_fg_colors(&body));
}

#[test]
fn markdown_read_body_uses_markdown_renderer() {
    let tc = read_tool_call("README.md", "# Heading\n\n**Body text**\n");
    let body = standard::render_tool_call_body(&tc, 120);
    let rendered = rendered_line_texts(&body).join("\n");

    assert!(rendered.contains("Heading"));
    assert!(rendered.contains("Body text"));
    assert!(!rendered.contains("**"));
}

#[test]
fn ask_user_question_completed_body_renders_structured_answers() {
    let mut tc =
        test_tool_call("AskUserQuestion", "AskUserQuestion", model::ToolCallStatus::Completed);
    tc.raw_input = Some(serde_json::json!({
        "question_results": [
            {
                "question": "Pick deployment target",
                "header": "Target",
                "question_index": 0,
                "total_questions": 2,
                "selected_options": [
                    {
                        "option_id": "question_0",
                        "label": "Staging",
                        "description": "Low-risk validation",
                        "preview": "Deploy to staging first."
                    }
                ],
                "annotation": {
                    "preview": "Deploy to staging first.",
                    "notes": "Roll out here before production."
                }
            },
            {
                "question": "When should this run?",
                "header": "Timing",
                "question_index": 1,
                "total_questions": 2,
                "selected_options": [
                    {
                        "option_id": "question_1",
                        "label": "After tests pass",
                        "description": ""
                    }
                ]
            }
        ]
    }));
    tc.content =
        vec![model::ToolCallContent::from("Target: Staging\n  Pick deployment target".to_owned())];

    let body = standard::render_tool_call_body(&tc, 120);
    let rendered = rendered_line_texts_trimmed(&body);
    let joined = rendered.join("\n");

    assert!(joined.contains("? Target (1/2)"));
    assert!(joined.contains("Pick deployment target"));
    assert!(joined.contains("[x] Staging - Low-risk validation"));
    assert!(joined.contains("Preview"));
    assert!(joined.contains("Deploy to staging first."));
    assert!(joined.contains("Notes: Roll out here before production."));
    assert!(joined.contains("? Timing (2/2)"));
    assert!(joined.contains("[x] After tests pass"));
    assert!(!joined.contains("Target: Staging"));
}

#[test]
fn ask_user_question_completed_body_indents_preview_and_notes_with_answers() {
    let mut tc =
        test_tool_call("AskUserQuestion", "AskUserQuestion", model::ToolCallStatus::Completed);
    tc.raw_input = Some(serde_json::json!({
        "question_results": [
            {
                "question": "Which config format do you prefer?",
                "header": "Config fmt",
                "question_index": 0,
                "total_questions": 1,
                "selected_options": [
                    {
                        "option_id": "question_0",
                        "label": "JSON",
                        "description": "Ubiquitous, no comments."
                    }
                ],
                "annotation": {
                    "preview": "{\n  \"server\": { \"host\": \"127.0.0.1\" }\n}",
                    "notes": "Use this for generated config."
                }
            }
        ]
    }));

    let body = standard::render_tool_call_body(&tc, 120);
    let rendered = rendered_line_texts_trimmed(&body);

    assert!(rendered.iter().any(|line| line.contains("  [x] JSON - Ubiquitous, no comments.")));
    assert!(rendered.iter().any(|line| line.contains("      Preview")));
    assert!(rendered.iter().any(|line| line.contains("        {")));
    assert!(
        rendered.iter().any(|line| line.contains("      Notes: Use this for generated config."))
    );
}

#[test]
fn ask_user_question_pending_body_hides_answer_transcript() {
    let mut tc =
        test_tool_call("AskUserQuestion", "AskUserQuestion", model::ToolCallStatus::InProgress);
    tc.content = vec![model::ToolCallContent::from(
        "Log level: info\n  What log level should ship as the default?".to_owned(),
    )];
    let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
    tc.pending_question = Some(crate::app::InlineQuestion {
        prompt: model::QuestionPrompt::new(
            "Which features should be enabled by default?",
            "Features",
            true,
            vec![
                model::QuestionOption::new("question_0", "Streaming")
                    .description(Some("Stream responses as they generate.".to_owned())),
            ],
        ),
        response_tx,
        focused_option_index: 0,
        selected_option_indices: std::collections::BTreeSet::from([0]),
        notes: String::new(),
        notes_cursor: 0,
        editing_notes: false,
        focused: true,
        question_index: 1,
        total_questions: 4,
    });

    let body = standard::render_tool_call_body(&tc, 120);
    let joined = rendered_line_texts_trimmed(&body).join("\n");

    assert!(joined.contains("? Features (2/4)"));
    assert!(joined.contains("Which features should be enabled by default?"));
    assert!(joined.contains("[x] Streaming"));
    assert!(!joined.contains("Log level: info"));
    assert!(!joined.contains("What log level should ship as the default?"));
}

#[test]
fn ask_user_question_completed_body_falls_back_to_transcript_without_structured_answers() {
    let mut tc =
        test_tool_call("AskUserQuestion", "AskUserQuestion", model::ToolCallStatus::Completed);
    tc.content =
        vec![model::ToolCallContent::from("Target: Staging\n  Pick deployment target".to_owned())];

    let body = standard::render_tool_call_body(&tc, 120);
    let joined = rendered_line_texts_trimmed(&body).join("\n");

    assert!(joined.contains("Target: Staging"));
    assert!(joined.contains("Pick deployment target"));
}

#[test]
fn bash_title_does_not_wrap_for_long_title() {
    let tc = ToolCallInfo {
        id: "tc-1".into(),
        source_message_uuids: Vec::new(),
        title: "echo very long command title with markdown **bold** and path /a/b/c/d/e/f".into(),
        sdk_tool_name: "Bash".into(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status: model::ToolCallStatus::Pending,
        content: Vec::new(),
        hidden: false,
        terminal_id: None,
        terminal_command: None,
        terminal_output: None,
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    };

    let top = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 40, 0);
    assert!(spans_width(&top.spans) <= 40);
}

#[test]
fn bash_body_uses_plain_indent_without_box_borders() {
    let mut tc = test_tool_call("tc-bash-indent", "Bash", model::ToolCallStatus::Completed);
    tc.terminal_id = Some("term-indent".to_owned());
    tc.terminal_command = Some("echo hi".to_owned());
    tc.terminal_output = Some("hi".to_owned());

    let mut rendered = Vec::new();
    render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 80, 0, &mut rendered);
    let rendered_text: Vec<String> = rendered
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert_eq!(rendered_text.len(), 2);
    assert!(rendered_text[1].starts_with("      hi"));
    assert!(!rendered_text.iter().any(|line| line.contains("echo hi")));
    assert!(
        rendered_text.iter().all(|line| !line.contains('\u{256D}') && !line.contains('\u{2570}'))
    );
    assert!(rendered_text.iter().all(|line| !line.starts_with("  \u{2502}")));
}

#[test]
fn powershell_execute_body_omits_command_prompt() {
    let mut tc =
        test_tool_call("tc-powershell-prompt", "PowerShell", model::ToolCallStatus::Completed);
    tc.terminal_command = Some("Get-ChildItem".to_owned());
    tc.terminal_output = Some("Directory listing".to_owned());

    let lines = execute::render_execute_content(&tc);
    let rendered: Vec<String> = lines
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert_eq!(rendered.first().map(String::as_str), Some("Directory listing"));
    assert!(!rendered.iter().any(|line| line.contains("Get-ChildItem")));
    assert!(!rendered.iter().any(|line| line.contains("PS>")));
}

#[test]
fn read_body_caps_long_wrapped_line_by_physical_rows() {
    let mut tc = test_tool_call("tc-read-wrap", "Read", model::ToolCallStatus::Completed);
    tc.title = "output.txt".to_owned();
    let long_line = (0..80).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
    tc.content = vec![model::ToolCallContent::from(long_line)];

    let body = standard::render_tool_call_body(&tc, 24);
    let rendered = rendered_line_texts_trimmed(&body);

    assert_eq!(body.len(), TOOL_BODY_MAX_LINES);
    assert!(rendered.iter().any(|line| line.contains("wrapped")));
}

#[test]
fn body_cap_reports_hidden_source_lines_when_full_lines_are_omitted() {
    let mut tc =
        test_tool_call("tc-source-line-count", "CustomTool", model::ToolCallStatus::Completed);
    tc.title = "output.txt".to_owned();
    let text = (0..20).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n");
    tc.content = vec![model::ToolCallContent::from(text)];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered = rendered_line_texts_trimmed(&body);

    assert_eq!(body.len(), TOOL_BODY_MAX_LINES);
    assert!(rendered[0].contains("12 source lines hidden"));
}

#[test]
fn wrapped_content_cap_keeps_permission_rows_visible() {
    let mut tc =
        test_tool_call("tc-permission-after-cap", "CustomTool", model::ToolCallStatus::InProgress);
    tc.title = "output.txt".to_owned();
    let long_line = (0..80).map(|idx| format!("word{idx}")).collect::<Vec<_>>().join(" ");
    tc.content = vec![model::ToolCallContent::from(long_line)];

    let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
    tc.pending_permission = Some(crate::app::InlinePermission {
        options: vec![
            model::PermissionOption::new("allow", "Allow", model::PermissionOptionKind::AllowOnce),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        display: None,
        subagent_context: None,
        response_tx,
        selected_index: 0,
        focused: true,
    });

    let body = standard::render_tool_call_body(&tc, 24);
    let rendered = rendered_line_texts_trimmed(&body);

    assert!(rendered.iter().any(|line| line.contains("wrapped")));
    assert!(rendered.iter().any(|line| line.contains("Allow")));
}

#[test]
fn render_tool_call_cached_prefixes_hidden_subagent_child_permission_title() {
    let mut tc = test_tool_call(
        "Write probe text to result file",
        "Write",
        model::ToolCallStatus::InProgress,
    );
    tc.hidden = true;
    tc.content = vec![model::ToolCallContent::from("running...".to_owned())];
    let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
    tc.pending_permission = Some(crate::app::InlinePermission {
        options: vec![
            model::PermissionOption::new(
                "allow_once",
                "Allow once",
                model::PermissionOptionKind::AllowOnce,
            ),
            model::PermissionOption::new(
                "allow_always",
                "Always allow",
                model::PermissionOptionKind::AllowAlways,
            ),
            model::PermissionOption::new("deny", "Deny", model::PermissionOptionKind::RejectOnce),
        ],
        display: Some(
            model::PermissionDisplay::new()
                .display_name(Some("Write".to_owned()))
                .description(Some("probe file".to_owned())),
        ),
        subagent_context: Some(crate::app::SubagentPermissionContext {
            subagent_label: "general-purpose".to_owned(),
            child_tool_name: "Write".to_owned(),
            child_tool_title: "Write probe text to result file".to_owned(),
            parent_tool_call_id: "agent-1".to_owned(),
            parent_tool_title: Some("Agent: general-purpose".to_owned()),
            parent_model: Some("claude-opus-4-8".to_owned()),
            parent_raw_input: None,
        }),
        response_tx,
        selected_index: 0,
        focused: true,
    });

    let mut rendered = Vec::new();
    render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 100, 0, &mut rendered);
    let text = rendered_line_texts_trimmed(&rendered);

    let first = text.first().expect("missing tool title row");
    assert!(first.contains("Subagent ·"), "missing inline subagent marker: {text:?}");
    assert!(
        first.contains("Write probe text to result file"),
        "first row should be the child tool title: {text:?}"
    );
    let child_idx = text
        .iter()
        .position(|line| line.contains("Write probe text to result file"))
        .expect("missing child tool title");
    let content_idx =
        text.iter().position(|line| line.contains("running...")).expect("missing content");
    let permission_idx =
        text.iter().position(|line| line.contains("Allow once")).expect("missing permission");

    assert_eq!(child_idx, 0, "child title should be the first row: {text:?}");
    assert!(child_idx < content_idx, "content should follow child title: {text:?}");
    assert!(content_idx < permission_idx, "permission should follow child content: {text:?}");
    assert!(!text.iter().any(|line| line.contains("general-purpose")));
    assert!(!text.iter().any(|line| line.contains("[model:")));
    assert!(!text.iter().any(|line| line.contains("Agent:")));
    assert!(!text.iter().any(|line| line.contains("probe file")));
}

#[test]
fn cached_tool_body_rerenders_after_width_change() {
    let mut tc = test_tool_call("tc-cache-width", "Bash", model::ToolCallStatus::Completed);
    tc.terminal_id = Some("term-cache-width".to_owned());
    tc.terminal_command = Some("echo wrapped".to_owned());
    tc.terminal_output =
        Some("alpha beta gamma delta epsilon zeta eta theta iota kappa lambda".to_owned());

    let mut wide = Vec::new();
    render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 100, 0, &mut wide);

    let mut narrow = Vec::new();
    render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 24, 0, &mut narrow);

    assert!(narrow.len() > wide.len(), "narrow render should rebuild cached body at the new width");
}

#[test]
fn bash_title_renders_assistant_backgrounded_badge() {
    let mut tc = test_tool_call("tc-bash-bg", "Bash", model::ToolCallStatus::Completed);
    tc.output_metadata = Some(
        model::ToolOutputMetadata::new()
            .bash(Some(model::BashOutputMetadata::new().assistant_auto_backgrounded(Some(true)))),
    );

    let rendered = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
    let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();
    assert!(text.contains("[assistant backgrounded]"));
}

#[test]
fn bash_title_distinguishes_timeout_auto_backgrounding() {
    let mut tc = test_tool_call("tc-bash-timeout", "Bash", model::ToolCallStatus::Completed);
    tc.output_metadata = Some(
        model::ToolOutputMetadata::new().bash(Some(
            model::BashOutputMetadata::new()
                .assistant_auto_backgrounded(Some(true))
                .timed_out_after_ms(Some(10_000)),
        )),
    );

    let rendered = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
    let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();
    assert!(text.contains("[auto-backgrounded after 10,000 ms]"));
    assert!(!text.contains("[assistant backgrounded]"));
}

#[test]
fn bash_title_renders_final_response_lifetime() {
    let mut tc = test_tool_call("tc-bash-lifetime", "Bash", model::ToolCallStatus::Completed);
    tc.output_metadata = Some(model::ToolOutputMetadata::new().bash(Some(
        model::BashOutputMetadata::new().background_ends_with_final_response(Some(true)),
    )));

    let rendered = standard::render_tool_call_title(&tc, ToolCallRenderContext::default(), 100, 0);
    let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();
    assert!(text.contains("[ends with final response]"));
}

#[test]
fn bash_title_preserves_command_title_in_plan_mode() {
    let mut tc = test_tool_call("echo hi", "Bash", model::ToolCallStatus::Completed);
    tc.terminal_command = Some("echo hi".to_owned());

    let rendered = standard::render_tool_call_title(
        &tc,
        ToolCallRenderContext { current_mode_id: Some("plan") },
        80,
        0,
    );
    let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();

    assert!(text.contains("Bash"));
    assert!(text.contains("echo hi"));
    assert!(!text.contains("Create Plan"));
    assert!(!text.contains("Update Plan"));
}

#[test]
fn mcp_resource_body_renders_saved_path_hint_when_text_omits_it() {
    let mut tc =
        test_tool_call("tc-mcp-resource", "ReadMcpResource", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::McpResource(
        model::McpResource::new("file://manual.pdf")
            .mime_type(Some("application/pdf".to_owned()))
            .text(Some("Binary resource downloaded successfully.".to_owned()))
            .blob_saved_to(Some("C:\\tmp\\manual.pdf".to_owned())),
    )];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.contains("Binary resource downloaded successfully.")));
    assert!(rendered.iter().any(|line| line.contains("Saved to: C:\\tmp\\manual.pdf")));
}

#[test]
fn mcp_resource_body_avoids_duplicate_saved_path_hint_when_text_already_mentions_it() {
    let mut tc =
        test_tool_call("tc-mcp-resource-dupe", "ReadMcpResource", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::McpResource(
        model::McpResource::new("file://manual.pdf")
            .mime_type(Some("application/pdf".to_owned()))
            .text(Some(
                "[Resource from docs at file://manual.pdf] Saved to C:\\tmp\\manual.pdf".to_owned(),
            ))
            .blob_saved_to(Some("C:\\tmp\\manual.pdf".to_owned())),
    )];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert_eq!(
        rendered.iter().filter(|line| line.contains("Saved to: C:\\tmp\\manual.pdf")).count(),
        0
    );
}

#[test]
fn read_tool_renders_head_hidden_marker_and_tail() {
    let mut tc = test_tool_call("tc-read-body", "Read", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::from(
        (0..12).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n"),
    )];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered_text: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert_eq!(rendered_text.len(), 8);
    for visible in ["line 0", "line 1", "line 2", "line 8", "line 9", "line 10", "line 11"] {
        assert!(rendered_text.iter().any(|line| line.contains(visible)));
    }
    assert!(rendered_text.iter().any(|line| line.contains("5 lines hidden")));
    for hidden in ["line 3", "line 4", "line 5", "line 6", "line 7"] {
        assert!(!rendered_text.iter().any(|line| line.contains(hidden)));
    }
}

#[test]
fn compact_tools_render_only_summary_line() {
    for sdk_tool_name in ["Agent", "Task", "WebSearch", "WebFetch", "ExitPlanMode"] {
        let mut tc = test_tool_call(
            &format!("tc-{sdk_tool_name}"),
            sdk_tool_name,
            model::ToolCallStatus::Completed,
        );
        tc.content = vec![model::ToolCallContent::from("first line\nsecond line".to_owned())];

        let body = standard::render_tool_call_body(&tc, 80);
        let rendered: Vec<String> = body
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].contains("first line"));
        assert!(!rendered[0].contains("second line"));
    }
}

#[test]
fn diff_tool_renders_without_expand_hint() {
    let mut tc = test_tool_call("tc-diff", "Write", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::Diff(
        model::Diff::new("src/main.rs", "new".to_owned()).old_text(Some("old".to_owned())),
    )];

    let mut rendered = Vec::new();
    render_tool_call_cached(&mut tc, ToolCallRenderContext::default(), 80, 0, &mut rendered);
    let text: Vec<String> = rendered
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(!text.iter().any(|line| line.contains("expand")));
    assert!(text.iter().any(|line| line.contains("(+1, -1)")));
    assert!(text.iter().any(|line| line.contains("+  new")));
    assert!(text.len() > 2);
}

#[test]
fn diff_tool_body_adds_nested_indent_inside_tool_prefix() {
    let mut tc = test_tool_call("tc-diff-indent", "Edit", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::Diff(
        model::Diff::new("src/main.rs", "new".to_owned())
            .old_text(Some("old".to_owned()))
            .repository(Some("acme/project".to_owned())),
    )];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.starts_with("  │    [acme/project]")));
    assert!(rendered.iter().any(|line| line.starts_with("  │    (+1, -1)")));
    assert!(rendered.iter().any(|line| {
        (line.starts_with("  │   ") || line.starts_with("  └─   ")) && line.contains("+  new")
    }));
}

#[test]
fn diff_tool_body_preserves_source_code_indentation() {
    let mut tc = test_tool_call("tc-diff-code-indent", "Edit", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::Diff(model::Diff::new(
        "src/main.rs",
        "fn main() {\n    if true {\n        return;\n    }\n}\n".to_owned(),
    ))];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.contains("+      if true {")));
    assert!(rendered.iter().any(|line| line.contains("+          return;")));
}

#[test]
fn diff_tool_body_preserves_nested_indent_for_wrapped_continuations() {
    let mut tc = test_tool_call("tc-diff-wrap", "Edit", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::Diff(model::Diff::new(
        "src/main.rs",
        "        This is a long added line that should wrap onto another visual line.\n".to_owned(),
    ))];

    let body = standard::render_tool_call_body(&tc, 28);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.contains("diff lines")));
    assert!(
        rendered.iter().any(|line| line.starts_with("  │                  "))
            || rendered.iter().any(|line| line.starts_with("  └─                  "))
    );
    assert!(rendered.iter().any(|line| line.contains("another")));
    assert!(rendered.iter().any(|line| line.contains("line.")));
}

#[test]
fn write_diff_cap_keeps_omission_marker_nested_indented() {
    let new_text = (0..120).fold(String::new(), |mut text, idx| {
        let _ = writeln!(&mut text, "line {idx}");
        text
    });
    let mut tc = test_tool_call("tc-diff-cap", "Write", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::Diff(model::Diff::new("src/main.rs", new_text))];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(
        rendered.iter().any(|line| line.starts_with("  │    (+120)"))
            || rendered.iter().any(|line| line.starts_with("  └─   (+120)"))
    );
    assert!(
            rendered
                .iter()
                .any(|line| line.starts_with("  │    ... ") && line.contains("diff lines omitted"))
                || rendered
                    .iter()
                    .any(|line| line.starts_with("  └─    ... ")
                        && line.contains("diff lines omitted"))
        );
}

#[test]
fn plan_files_render_markdown_instead_of_diff() {
    let mut tc =
        test_tool_call("Write .claude/plans/launch.md", "Write", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::Diff(
        model::Diff::new(
            ".claude/plans/launch.md",
            "# Launch Plan\n\n- Ship aliases\n- Render plan markdown\n".to_owned(),
        )
        .old_text(Some("# Old Plan\n".to_owned())),
    )];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.contains("Launch Plan")));
    assert!(rendered.iter().any(|line| line.contains("Render plan markdown")));
    assert!(!rendered.iter().any(|line| line.contains("@@")));
    assert!(!rendered.iter().any(|line| line.starts_with("+ ")));
}

#[test]
fn plan_file_markdown_body_is_not_capped_by_tool_height() {
    let mut tc =
        test_tool_call("Write .claude/plans/long.md", "Write", model::ToolCallStatus::Completed);
    let plan_text = (0..24).map(|idx| format!("- Step {idx}")).collect::<Vec<_>>().join("\n");
    tc.content =
        vec![model::ToolCallContent::Diff(model::Diff::new(".claude/plans/long.md", plan_text))];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered = rendered_line_texts_trimmed(&body);

    assert!(body.len() > TOOL_BODY_MAX_LINES);
    assert!(rendered.iter().any(|line| line.contains("Step 23")));
    assert!(!rendered.iter().any(|line| line.contains("hidden")));
    assert!(!rendered.iter().any(|line| line.contains("omitted")));
}

#[test]
fn non_plan_write_diff_body_stays_capped_by_tool_height() {
    let mut tc = test_tool_call("Write notes/long.md", "Write", model::ToolCallStatus::Completed);
    let new_text = (0..80).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n");
    tc.content = vec![model::ToolCallContent::Diff(model::Diff::new("notes/long.md", new_text))];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered = rendered_line_texts_trimmed(&body);

    assert_eq!(body.len(), TOOL_BODY_MAX_LINES);
    assert!(rendered.iter().any(|line| line.contains("diff lines omitted")));
}

#[test]
fn internal_error_detection_accepts_xml_payload() {
    let payload = "<error><code>-32603</code><message>Adapter process crashed</message></error>";
    assert!(looks_like_internal_error(payload));
}

#[test]
fn internal_error_detection_rejects_plain_bash_failure() {
    let payload = "bash: unknown_command: command not found";
    assert!(!looks_like_internal_error(payload));
}

#[test]
fn summarize_internal_error_prefers_xml_message() {
    let payload = "<error><code>-32603</code><message>Adapter process crashed</message></error>";
    assert_eq!(summarize_internal_error(payload), "Adapter process crashed");
}

#[test]
fn summarize_internal_error_reads_json_rpc_message() {
    let payload = r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"internal rpc fault"}}"#;
    assert_eq!(summarize_internal_error(payload), "internal rpc fault");
}

#[test]
fn extract_tool_use_error_message_reads_inner_text() {
    let payload = "<tool_use_error>Sibling tool call errored</tool_use_error>";
    assert_eq!(
        extract_tool_use_error_message(payload).as_deref(),
        Some("Sibling tool call errored")
    );
}

#[test]
fn failed_tool_text_summary_reads_common_xml_error_wrappers() {
    let failed = model::ToolCallStatus::Failed;
    assert_eq!(
        failed_tool_text_summary(
            failed,
            "<tool_use_error>Sibling tool call errored</tool_use_error>"
        )
        .as_deref(),
        Some("Sibling tool call errored")
    );
    assert_eq!(
        failed_tool_text_summary(
            failed,
            "<error><code>-32603</code><message>Adapter process crashed</message></error>"
        )
        .as_deref(),
        Some("Adapter process crashed")
    );
    assert_eq!(
        failed_tool_text_summary(failed, "<fault>Remote call failed</fault>").as_deref(),
        Some("Remote call failed")
    );
    assert_eq!(
        failed_tool_text_summary(failed, "<custom_error>Wrapped failure</custom_error>").as_deref(),
        Some("Wrapped failure")
    );
    assert_eq!(
        failed_tool_text_summary(
            model::ToolCallStatus::Completed,
            "<message>Successful XML output</message>",
        ),
        None
    );
    assert_eq!(failed_tool_text_summary(failed, "<message>missing close"), None);
}

#[test]
fn render_tool_use_error_content_shows_only_inner_text_lines() {
    let lines = render_tool_use_error_content("Line A\nLine B");
    let rendered: Vec<String> =
        lines.iter().map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
    assert_eq!(rendered.len(), 2);
    assert!(rendered.iter().any(|line| line == "Line A"));
    assert!(rendered.iter().any(|line| line == "Line B"));
}

#[test]
fn content_summary_only_extracts_tool_use_error_for_failed_execute() {
    let tc = ToolCallInfo {
        id: "tc-1".into(),
        source_message_uuids: Vec::new(),
        title: "Bash".into(),
        sdk_tool_name: "Bash".into(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status: model::ToolCallStatus::Completed,
        content: Vec::new(),
        hidden: false,
        terminal_id: Some("term-1".into()),
        terminal_command: Some("echo done".into()),
        terminal_output: Some("<tool_use_error>bad</tool_use_error>\ndone".into()),
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    };
    assert_eq!(content_summary(&tc), "done");
}

#[test]
fn content_summary_extracts_tool_use_error_for_failed_execute() {
    let tc = ToolCallInfo {
        id: "tc-1".into(),
        source_message_uuids: Vec::new(),
        title: "Bash".into(),
        sdk_tool_name: "Bash".into(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status: model::ToolCallStatus::Failed,
        content: Vec::new(),
        hidden: false,
        terminal_id: Some("term-1".into()),
        terminal_command: Some("echo done".into()),
        terminal_output: Some("<tool_use_error>bad</tool_use_error>\ndone".into()),
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    };
    assert_eq!(content_summary(&tc), "bad");
}

#[test]
fn content_summary_extracts_xml_message_for_failed_text_tool() {
    let mut tc = test_tool_call("tc-web", "WebFetch", model::ToolCallStatus::Failed);
    tc.content = vec![model::ToolCallContent::from(
        "<error><message>Fetch failed</message></error>".to_owned(),
    )];

    assert_eq!(content_summary(&tc), "Fetch failed");
}

#[test]
fn content_summary_uses_first_terminal_line_for_failed_execute() {
    let tc = ToolCallInfo {
        id: "tc-2".into(),
        source_message_uuids: Vec::new(),
        title: "Bash".into(),
        sdk_tool_name: "Bash".into(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status: model::ToolCallStatus::Failed,
        content: Vec::new(),
        hidden: false,
        terminal_id: Some("term-2".into()),
        terminal_command: Some("cd path with spaces".into()),
        terminal_output: Some(
            "Exit code 1\n/usr/bin/bash: line 1: cd: too many arguments\nmore detail".into(),
        ),
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    };
    assert_eq!(content_summary(&tc), "Exit code 1");
}

#[test]
fn content_summary_uses_higher_limit_for_in_progress_agent() {
    let mut tc = test_tool_call("tc-agent", "Agent", model::ToolCallStatus::InProgress);
    let long_line = "a".repeat(150);
    tc.content = vec![model::ToolCallContent::from(long_line.clone())];

    assert_eq!(content_summary(&tc), long_line);
}

#[test]
fn content_summary_keeps_normal_limit_for_completed_agent() {
    let mut tc = test_tool_call("tc-agent-done", "Agent", model::ToolCallStatus::Completed);
    let long_line = "a".repeat(150);
    tc.content = vec![model::ToolCallContent::from(long_line)];

    let summary = content_summary(&tc);
    assert_eq!(summary.chars().count(), 60);
    assert!(summary.ends_with("..."));
}

#[test]
fn render_execute_content_keeps_tail_output() {
    let tc = ToolCallInfo {
        id: "tc-3".into(),
        source_message_uuids: Vec::new(),
        title: "Bash".into(),
        sdk_tool_name: "Bash".into(),
        raw_input: None,
        raw_input_bytes: 0,
        locations: Vec::new(),
        output_metadata: None,
        task_metadata: None,
        status: model::ToolCallStatus::Failed,
        content: Vec::new(),
        hidden: false,
        terminal_id: Some("term-3".into()),
        terminal_command: Some("cd path with spaces".into()),
        terminal_output: Some(
            (0..30).map(|idx| format!("line {idx}")).collect::<Vec<_>>().join("\n"),
        ),
        terminal_output_len: 0,
        cache: BlockCache::default(),
        pending_permission: None,
        pending_question: None,
    };

    let lines = execute::render_execute_content(&tc);
    let rendered: Vec<String> =
        lines.iter().map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect()).collect();
    assert_eq!(rendered.len(), super::TOOL_BODY_MAX_LINES);
    assert!(rendered[0].contains("lines hidden"));
    assert!(!rendered.iter().any(|line| line == "line 0"));
    assert!(!rendered.iter().any(|line| line.contains("cd path with spaces")));
    assert!(rendered.iter().any(|line| line == "line 22"));
    assert_eq!(rendered.last().map(String::as_str), Some("line 29"));
}

#[test]
fn render_execute_content_extracts_tool_use_error() {
    let mut tc = test_tool_call("tc-xml", "Bash", model::ToolCallStatus::Failed);
    tc.terminal_id = Some("term-xml".into());
    tc.terminal_command = Some("cd path with spaces".into());
    tc.terminal_output = Some(
            "<tool_use_error>Cancelled: parallel tool call Bash(cd path) errored</tool_use_error>\nraw fallback"
                .into(),
        );

    let lines = execute::render_execute_content(&tc);
    let rendered: Vec<String> =
        lines.iter().map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect()).collect();

    assert!(!rendered.iter().any(|line| line.contains("$ cd path with spaces")));
    assert!(
        rendered.iter().any(|line| line == "Cancelled: parallel tool call Bash(cd path) errored")
    );
    assert!(!rendered.iter().any(|line| line.contains("<tool_use_error>")));
    assert!(!rendered.iter().any(|line| line.contains("</tool_use_error>")));
    assert!(!rendered.iter().any(|line| line.contains("raw fallback")));
}

#[test]
fn render_execute_content_extracts_xml_message_error() {
    let mut tc = test_tool_call("tc-xml-message", "Bash", model::ToolCallStatus::Failed);
    tc.terminal_id = Some("term-xml".into());
    tc.terminal_output = Some("<error><message>Adapter process crashed</message></error>".into());

    let lines = execute::render_execute_content(&tc);
    let rendered: Vec<String> =
        lines.iter().map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect()).collect();

    assert!(rendered.iter().any(|line| line == "Adapter process crashed"));
    assert!(!rendered.iter().any(|line| line.contains("<message>")));
    assert!(!rendered.iter().any(|line| line.contains("<error>")));
}

#[test]
fn failed_text_tool_body_extracts_xml_error_message() {
    let mut tc = test_tool_call("tc-read-error", "Read", model::ToolCallStatus::Failed);
    tc.content = vec![model::ToolCallContent::from(
        "<error><message>Read failed</message></error>".to_owned(),
    )];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.contains("Read failed")));
    assert!(!rendered.iter().any(|line| line.contains("<message>")));
    assert!(!rendered.iter().any(|line| line.contains("<error>")));
}

#[test]
fn successful_text_tool_body_preserves_xml_like_output() {
    let mut tc = test_tool_call("tc-read-xml", "Read", model::ToolCallStatus::Completed);
    tc.content = vec![model::ToolCallContent::from("<message>not an error</message>".to_owned())];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.contains("<message>not an error</message>")));
}

#[test]
fn diff_tool_body_preserves_xml_like_source() {
    let mut tc = test_tool_call("tc-diff-xml", "Write", model::ToolCallStatus::Failed);
    tc.content = vec![model::ToolCallContent::Diff(model::Diff::new(
        "src/main.xml",
        "<message>source text</message>".to_owned(),
    ))];

    let body = standard::render_tool_call_body(&tc, 80);
    let rendered: Vec<String> = body
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
        .collect();

    assert!(rendered.iter().any(|line| line.contains("<message>source text</message>")));
}

#[test]
fn write_diff_cap_keeps_tail_with_omission_marker() {
    use standard::WRITE_DIFF_MAX_LINES;

    let lines: Vec<Line<'static>> = (0..120).map(|idx| Line::from(format!("line {idx}"))).collect();
    let capped = cap_write_diff_lines(lines);
    let rendered: Vec<String> =
        capped.iter().map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect()).collect();

    assert_eq!(rendered.len(), WRITE_DIFF_MAX_LINES);
    assert!(rendered[0].contains("diff lines omitted"));
    assert!(!rendered.iter().any(|line| line == "line 0"));
    assert!(rendered.iter().any(|line| line == "line 112"));
    assert_eq!(rendered.last().map(String::as_str), Some("line 119"));
}

#[test]
fn write_diff_cap_preserves_diff_count_header() {
    use standard::WRITE_DIFF_MAX_LINES;

    let mut lines: Vec<Line<'static>> = vec![Line::from("(+120)")];
    lines.extend((0..120).map(|idx| Line::from(format!("line {idx}"))));

    let capped = cap_write_diff_lines(lines);
    let rendered: Vec<String> =
        capped.iter().map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect()).collect();

    assert_eq!(rendered.len(), WRITE_DIFF_MAX_LINES);
    assert_eq!(rendered[0], "(+120)");
    assert!(rendered[1].contains("diff lines omitted"));
    assert!(!rendered.iter().any(|line| line == "line 0"));
    assert!(rendered.iter().any(|line| line == "line 113"));
    assert_eq!(rendered.last().map(String::as_str), Some("line 119"));
}
