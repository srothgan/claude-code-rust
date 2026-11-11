// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::model;
use crate::app::test_support::{app_with_connection, deliver_update, named_tool_call_info};
use crate::ui::tool_call::test_support::{
    assert_focused_option, attach_permission, interaction_app, press,
};
use crossterm::event::KeyCode;
use std::time::Duration;

fn assistant(blocks: Vec<MessageBlock>) -> ChatMessage {
    ChatMessage::new(MessageRole::Assistant, blocks, None)
}

fn render(app: &mut App, width: u16, excluded: &BTreeSet<HistoryOutputId>) -> SerializedLiveRows {
    serialize_live_rows_with_boundaries_excluding(app, width, excluded, None)
}

fn region(rows: &SerializedLiveRows, id: &HistoryOutputId) -> Vec<Line<'static>> {
    let selected: Vec<_> = rows.segments().iter().filter(|s| s.ids.contains(id)).collect();
    assert!(!selected.is_empty(), "missing output {id:?}");
    selected.into_iter().flat_map(|s| rows.segment_rows(s).unwrap().iter().cloned()).collect()
}

fn text(rows: &[Line<'_>]) -> Vec<String> {
    rows.iter().map(|r| r.to_string().trim_end().to_owned()).collect()
}

fn numbered_code() -> String {
    format!(
        "```text\n{}\n```",
        (0..400).map(|n| format!("SOURCE_{n:04}")).collect::<Vec<_>>().join("\n")
    )
}

fn assert_source(rows: &SerializedLiveRows, id: &HistoryOutputId, width: u16) {
    let own = region(rows, id);
    let actual: Vec<_> = text(&own)
        .into_iter()
        .filter_map(|s| s.trim().strip_prefix("SOURCE_").map(str::to_owned))
        .collect();
    assert_eq!(actual, (0..400).map(|n| format!("{n:04}")).collect::<Vec<_>>());
    assert!(rows.rows().iter().all(|r| r.width() <= usize::from(width)));
}

#[test]
fn replay_exposes_a_proper_prefix_and_cancelled_layout_restarts_from_canonical_source() {
    for width in [24, 80, 120] {
        let source = numbered_code();
        let block = TextBlock::from_complete(&source);
        let id = HistoryOutputId::Block(block.id);
        let mut app = App::test_default();
        app.push_message_tracked(assistant(vec![MessageBlock::Text(block)]));
        let mut cursor = LiveRowsCursor::new(Instant::now());
        let mut collected = SerializedLiveRows::new(Vec::new(), Vec::new());
        let mut prefix = false;
        for _ in 0..10_000 {
            let control = LayoutControl::with_budget(&|| false, Duration::ZERO);
            if let Some(chunk) = cursor.next(&mut app, width, &BTreeSet::new(), &control) {
                collected.append(chunk);
            }
            let visible = text(collected.rows()).join("\n");
            if visible.contains("SOURCE_0000") && !visible.contains("SOURCE_0399") {
                prefix = true;
                let cancel = LayoutControl::uninterrupted();
                cancel.cancel();
                assert!(cursor.next(&mut app, width, &BTreeSet::new(), &cancel).is_none());
                break;
            }
            assert!(!cursor.is_finished(), "first observable source must be a proper prefix");
        }
        assert!(prefix, "replay produced no early source prefix");
        // Discard the cancelled frame. A fresh cursor at another width must retain every line.
        let mut fresh = LiveRowsCursor::new(Instant::now());
        let mut complete = SerializedLiveRows::new(Vec::new(), Vec::new());
        for _ in 0..10_000 {
            let control = LayoutControl::with_budget(&|| false, Duration::ZERO);
            if let Some(chunk) = fresh.next(&mut app, 31, &BTreeSet::new(), &control) {
                complete.append(chunk);
            }
            if fresh.is_finished() {
                break;
            }
        }
        assert!(fresh.is_finished(), "resumed replay must finish");
        assert_source(&complete, &id, 31);
        let MessageBlock::Text(block) = &app.transcript.messages[0].blocks[0] else {
            panic!("source")
        };
        assert_eq!(HistoryOutputId::Block(block.id), id);
        assert_eq!(block.text, source);
        let direct = render(&mut app, width, &BTreeSet::new());
        assert_source(&direct, &id, width);
    }
}

#[test]
fn ready_text_precedes_expensive_read_preview_and_restart_retains_visible_preview() {
    let mut tool = named_tool_call_info("expensive", "Read", model::ToolCallStatus::Completed);
    tool.kind = model::ToolKind::Read;
    tool.locations = vec![model::ToolCallLocation::new("source.rs")];
    tool.content = vec![model::ToolCallContent::from(
        (0..2000).map(|n| format!("READ_{n:04}")).collect::<Vec<_>>().join("\n"),
    )];
    let mut app = App::test_default();
    app.push_message_tracked(assistant(vec![
        MessageBlock::Text(TextBlock::from_complete("FIRST")),
        MessageBlock::ToolCall(Box::new(tool)),
        MessageBlock::Text(TextBlock::from_complete("LAST")),
    ]));
    let mut cursor = LiveRowsCursor::new(Instant::now());
    let first =
        cursor.next(&mut app, 80, &BTreeSet::new(), &LayoutControl::uninterrupted()).unwrap();
    let visible = text(first.rows()).join("\n");
    assert!(visible.contains("FIRST"));
    assert!(!visible.contains("READ_0000") && !visible.contains("LAST"));
    let cancel = LayoutControl::uninterrupted();
    cancel.cancel();
    assert!(cursor.next(&mut app, 80, &BTreeSet::new(), &cancel).is_none());
    let fresh = render(&mut app, 31, &BTreeSet::new());
    let preview = text(&region(&fresh, &HistoryOutputId::ToolCall("expensive".into()))).join("\n");
    for n in [0, 1, 2, 1996, 1997, 1998, 1999] {
        assert_eq!(preview.matches(&format!("READ_{n:04}")).count(), 1, "{preview}");
    }
    assert!(!preview.contains("READ_0420"), "middle of a long preview is intentionally omitted");
    for marker in ["FIRST", "LAST"] {
        assert_eq!(text(fresh.rows()).join("\n").matches(marker).count(), 1);
    }
    assert!(fresh.rows().iter().all(|r| r.width() <= 31));
}

#[test]
fn reduced_motion_stabilizes_only_the_running_tool_region() {
    let mut running = named_tool_call_info("running", "Bash", model::ToolCallStatus::InProgress);
    running.title = "RUNNING_TARGET".into();
    let mut done = named_tool_call_info("done", "Bash", model::ToolCallStatus::Completed);
    done.title = "STATIC_NEIGHBOR".into();
    let mut app = interaction_app(vec![running, done]);
    let mut animated = Vec::new();
    let mut static_target = Vec::new();
    let mut neighbor = None;
    for reduced in [false, true] {
        app.config.snapshot = reduced.then(|| {
            crate::agent::settings::SettingsSnapshot::test_value(
                "prefersReducedMotion",
                serde_json::json!(true),
            )
        });
        for frame in [0, 3, 8, 13] {
            app.spinner_frame = frame;
            let rows = render(&mut app, 80, &BTreeSet::new());
            let own = text(&region(&rows, &HistoryOutputId::ToolCall("running".into())));
            assert!(own.iter().any(|r| r.contains("RUNNING_TARGET")));
            let other = text(&region(&rows, &HistoryOutputId::ToolCall("done".into())));
            assert!(other.iter().any(|r| r.contains("STATIC_NEIGHBOR")));
            if let Some(previous) = &neighbor {
                assert_eq!(&other, previous);
            }
            neighbor = Some(other);
            if reduced {
                static_target.push(own);
            } else {
                animated.push(own);
            }
        }
    }
    assert!(animated.windows(2).any(|pair| pair[0] != pair[1]));
    assert!(static_target.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn wrapped_code_preserves_indentation_and_internal_spaces_inside_the_message_column() {
    let code = "    ALPHA  BETA  GAMMA  DELTA  EPSILON  ZETA";
    let mut app = App::test_default();
    let block = TextBlock::from_complete(&format!("```text\n{code}\n```"));
    let id = HistoryOutputId::Block(block.id);
    app.push_message_tracked(assistant(vec![MessageBlock::Text(block)]));
    for width in [80, 30, 27, 22, 80] {
        let rows = render(&mut app, width, &BTreeSet::new());
        let own = region(&rows, &id);
        let label = own
            .iter()
            .position(|r| r.spans.iter().any(|s| s.content == "text"))
            .expect("language label");
        let heading = own[label].to_string();
        let column =
            unicode_width::UnicodeWidthStr::width(&heading[..heading.find("text").unwrap()]);
        let body: Vec<_> = own[label + 1..].iter().take_while(|r| !line_is_blank(r)).collect();
        assert!(!body.is_empty());
        let mut reconstructed = String::new();
        for row in &body {
            assert!(row.width() <= usize::from(width));
            let s = row.to_string();
            assert!(s[..column].trim().is_empty());
            // Remove only the shared outer framing. Code is ASCII here so byte and cell
            // columns coincide; an intermediate row ending in a source space must retain it.
            let outer_padding = row.spans[0].width();
            reconstructed.push_str(&s[column..usize::from(width) - outer_padding]);
        }
        // The source ends in a nonspace. Remove only final physical-row fill, keeping all
        // indentation, internal spaces and spaces at intermediate wrap boundaries.
        assert_eq!(reconstructed.trim_end(), code, "width {width}: {:?}", text(&own));
        if width < 40 {
            assert!(body.len() > 1);
        }
    }
}

#[test]
fn completed_prefix_can_commit_while_real_permission_choices_remain_interactive() {
    let mut completed = named_tool_call_info("prefix", "Write", model::ToolCallStatus::Completed);
    completed.title = "COMPLETED_PREFIX".into();
    let mut pending = named_tool_call_info("pending", "Write", model::ToolCallStatus::InProgress);
    pending.title = "PENDING_TARGET".into();
    let mut app = interaction_app(vec![completed, pending]);
    let options = vec![
        model::PermissionOption::new(
            "once",
            "Allow target once",
            model::PermissionOptionKind::AllowOnce,
        ),
        model::PermissionOption::new(
            "always",
            "Allow target always",
            model::PermissionOptionKind::AllowAlways,
        ),
        model::PermissionOption::new(
            "reject",
            "Reject target",
            model::PermissionOptionKind::RejectOnce,
        ),
    ];
    let mut receiver = attach_permission(&mut app, "pending", options, None);
    let prefix = HistoryOutputId::ToolCall("prefix".into());
    let target = HistoryOutputId::ToolCall("pending".into());
    let initial = render(&mut app, 80, &BTreeSet::new());
    assert!(text(&region(&initial, &prefix)).join("\n").contains("COMPLETED_PREFIX"));
    assert!(
        initial
            .segments()
            .iter()
            .filter(|s| s.ids.contains(&prefix))
            .all(|s| s.commit_ready && s.end_row <= initial.stable_row_count())
    );
    assert!(
        initial
            .segments()
            .iter()
            .filter(|s| s.ids.contains(&target))
            .all(|s| !s.commit_ready && s.start_row >= initial.stable_row_count())
    );
    let excluded: BTreeSet<_> =
        initial.segments().iter().filter(|s| s.commit_ready).flat_map(|s| s.ids.clone()).collect();
    let labels = ["Allow target once", "Allow target always", "Reject target"];
    assert_focused_option(&region(&initial, &target), labels, 0);
    press(&mut app, KeyCode::Right);
    let moved = render(&mut app, 80, &excluded);
    assert_focused_option(&region(&moved, &target), labels, 1);
    assert!(!text(moved.rows()).join("\n").contains("COMPLETED_PREFIX"));
    assert!(receiver.try_recv().is_err());
    press(&mut app, KeyCode::Enter);
    let response = receiver.try_recv().expect("selected permission response");
    assert!(
        matches!(response.outcome, model::RequestPermissionOutcome::Selected(ref selected) if selected.option_id == "always")
    );
    crate::app::handle_client_event(
        &mut app,
        crate::agent::events::ClientEvent::SessionUpdate {
            session_id: "interaction-render".into(),
            update: model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "pending",
                model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
            )),
        },
    );
    let resolved = render(&mut app, 80, &excluded);
    let own = region(&resolved, &target);
    assert!(text(&own).join("\n").contains("PENDING_TARGET"));
    assert!(labels.iter().all(|s| !text(&own).join("\n").contains(s)));
    assert!(resolved.segments().iter().filter(|s| s.ids.contains(&target)).all(|s| s.commit_ready));
}

#[test]
fn detached_launch_stays_frozen_after_exclusion_and_completion_owns_a_new_result() {
    let (mut app, _commands) = app_with_connection();
    deliver_update(
        &mut app,
        model::SessionUpdate::ToolCall(
            model::ToolCall::new("web", "DEFERRED_LAUNCH")
                .kind(model::ToolKind::Fetch)
                .status(model::ToolCallStatus::Detached)
                .meta(serde_json::json!({"claudeCode":{"toolName":"WebFetch"}})),
        ),
    );
    app.status = AppStatus::Ready;
    let launch_id = HistoryOutputId::ToolCall("web".into());
    let initial = render(&mut app, 100, &BTreeSet::new());
    let launch = text(&region(&initial, &launch_id));
    assert!(launch.iter().any(|r| r.contains("DEFERRED_LAUNCH")));
    assert_eq!(initial.stable_row_count(), initial.rows().len());
    let excluded = initial.segments().iter().flat_map(|s| s.ids.clone()).collect();
    assert!(render(&mut app, 100, &excluded).rows().is_empty());
    deliver_update(
        &mut app,
        model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
            "web",
            model::ToolCallUpdateFields::new()
                .status(model::ToolCallStatus::Completed)
                .content(vec![model::ToolCallContent::from("FRESH_WEB_RESULT")]),
        )),
    );
    let result_id = app
        .transcript
        .messages
        .iter()
        .flat_map(|m| &m.blocks)
        .find_map(|b| match b {
            MessageBlock::ToolResult { id, tool } if tool.id == "web" => {
                Some(HistoryOutputId::Block(*id))
            }
            _ => None,
        })
        .expect("separate completion result");
    let remaining = render(&mut app, 100, &excluded);
    assert_eq!(
        text(&region(&remaining, &result_id)).join("\n").matches("FRESH_WEB_RESULT").count(),
        1
    );
    assert!(remaining.segments().iter().all(|s| !s.ids.contains(&launch_id)));
    assert!(
        remaining.segments().iter().filter(|s| s.ids.contains(&result_id)).all(|s| s.commit_ready)
    );
    let full = render(&mut app, 100, &BTreeSet::new());
    assert_eq!(text(&region(&full, &launch_id)), launch);
    let mut all_excluded = excluded;
    all_excluded.insert(result_id);
    assert!(render(&mut app, 100, &all_excluded).rows().is_empty());
}

#[test]
fn separate_paragraph_and_list_blocks_keep_one_seam_owned_by_the_following_region() {
    for (left, last_left, right, first_right, list) in [
        ("SEAM_LEFT\n\n", "SEAM_LEFT", "SEAM_RIGHT", "SEAM_RIGHT", false),
        ("SEAM_LEFT\n\n", "SEAM_LEFT", "- ITEM_A\n- ITEM_B", "ITEM_A", true),
        ("- ITEM_A\n- ITEM_B\n\n", "ITEM_B", "SEAM_RIGHT", "SEAM_RIGHT", false),
    ] {
        let first =
            TextBlock::from_complete(left).with_trailing_spacing(TextBlockSpacing::ParagraphBreak);
        let first_id = HistoryOutputId::Block(first.id);
        let second = TextBlock::from_complete(right);
        let second_id = HistoryOutputId::Block(second.id);
        let msg = assistant(vec![MessageBlock::Text(first), MessageBlock::Text(second)]);
        let label = HistoryOutputId::AssistantLabel(msg.id);
        let mut app = App::test_default();
        app.push_message_tracked(msg);
        for width in [32, 80] {
            let rows = render(&mut app, width, &BTreeSet::new());
            let rendered = text(rows.rows());
            let end = rendered.iter().position(|r| r.contains(last_left)).unwrap();
            let start = rendered.iter().position(|r| r.contains(first_right)).unwrap();
            assert_eq!(start, end + 2, "one paragraph separator: {rendered:?}");
            assert!(rendered[end + 1].trim().is_empty());
            let column = |row: &str, needle: &str| {
                unicode_width::UnicodeWidthStr::width(&row[..row.find(needle).unwrap()])
            };
            let left_column = column(&rendered[end], last_left);
            let right_column = column(&rendered[start], first_right);
            let left_list = left.starts_with("- ");
            assert_eq!(
                left_column - if left_list { 2 } else { 0 },
                right_column - if list { 2 } else { 0 },
                "paragraphs and list markers share their content column"
            );
            if list {
                assert!(rendered[start + 1].contains("ITEM_B"));
            }
            let live = render(&mut app, width, &BTreeSet::from([first_id.clone(), label.clone()]));
            let own = region(&live, &second_id);
            assert!(line_is_blank(&own[0]), "following region owns its leading seam");
            assert!(text(&own).iter().any(|r| r.contains(first_right)));
            assert!(live.segments().iter().all(|s| !s.ids.contains(&first_id)));
        }
    }
}

#[test]
fn split_tables_and_literal_fences_preserve_independent_content_and_source_ownership() {
    for (first, second, required, literal) in [
        (
            "Name | Value\n- | -\nRED | ONE\n",
            "BLUE | TWO\n",
            vec!["Name", "Value", "RED", "ONE", "BLUE", "TWO"],
            false,
        ),
        ("Name | Value\n--- | ---\nRED\n", "BLUE\n", vec!["Name", "Value", "RED", "BLUE"], false),
        (
            "> Name | Value\n> --- | ---\n> RED | ONE\n",
            "> BLUE | TWO\n",
            vec!["Name", "Value", "RED", "ONE", "BLUE", "TWO"],
            false,
        ),
        (
            "````md\n```\nName | Value\n",
            "- | -\nRED | ONE\n````",
            vec!["```", "Name | Value", "- | -", "RED | ONE"],
            true,
        ),
    ] {
        let a = TextBlock::from_complete(first);
        let b = TextBlock::from_complete(second);
        let ids = [HistoryOutputId::Block(a.id), HistoryOutputId::Block(b.id)];
        let mut app = App::test_default();
        app.push_message_tracked(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete("TABLE_NEIGHBOR"))],
            None,
        ));
        app.push_message_tracked(assistant(vec![MessageBlock::Text(a), MessageBlock::Text(b)]));
        app.bind_active_turn_assistant(1);
        let mut whole = App::test_default();
        whole.push_message_tracked(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete("TABLE_NEIGHBOR"))],
            None,
        ));
        whole.push_message_tracked(assistant(vec![MessageBlock::Text(TextBlock::from_complete(
            &format!("{first}{second}"),
        ))]));
        whole.bind_active_turn_assistant(1);
        for running in [true, false] {
            app.status = if running { AppStatus::Running } else { AppStatus::Ready };
            whole.status = app.status;
            for width in [24, 80, 24] {
                let rows = render(&mut app, width, &BTreeSet::new());
                for id in &ids {
                    let owner = region(&rows, id);
                    let visible = text(&owner).join("\n");
                    for needle in &required {
                        assert!(visible.contains(needle), "missing {needle}: {visible}");
                    }
                    assert!(
                        rows.segments()
                            .iter()
                            .filter(|s| s.ids.contains(id))
                            .all(|s| s.commit_ready != running)
                    );
                    if !literal {
                        assert!(!visible.lines().any(|r| r.trim_start().starts_with("RED |")));
                    }
                    if !literal && width == 80 {
                        assert_table_columns(&text(&owner), &required);
                    }
                }
                assert!(rows.rows().iter().all(|r| r.width() <= usize::from(width)));
                assert_eq!(
                    text(rows.rows()),
                    text(render(&mut whole, width, &BTreeSet::new()).rows()),
                    "secondary split/whole invariance"
                );
                if !running {
                    let excluded: BTreeSet<_> = rows
                        .segments()
                        .iter()
                        .filter(|s| s.msg_idx == 1)
                        .flat_map(|s| s.ids.clone())
                        .collect();
                    let remaining = render(&mut app, width, &excluded);
                    assert!(text(remaining.rows()).join("\n").contains("TABLE_NEIGHBOR"));
                    assert!(
                        remaining
                            .segments()
                            .iter()
                            .all(|s| ids.iter().all(|id| !s.ids.contains(id)))
                    );
                }
            }
        }
    }
}

fn assert_table_columns(lines: &[String], required: &[&str]) {
    let header = lines.iter().find(|r| r.contains("Name")).unwrap();
    assert!(header.find("Name").unwrap() < header.find("Value").unwrap());
    for (left, right) in [("RED", "ONE"), ("BLUE", "TWO")] {
        let row = lines.iter().find(|r| r.contains(left)).unwrap();
        if required.contains(&right) {
            assert!(row.find(left).unwrap() < row.find(right).unwrap());
        } else {
            assert!(!row.contains("ONE") && !row.contains("TWO"), "missing cells must stay empty");
        }
    }
}

#[test]
fn closed_markdown_prefix_commits_without_swallowing_an_independent_pipe_tail() {
    for (source, spacing, required) in [
        (
            "Name | Value\n--- | ---\nRED | ONE\n",
            TextBlockSpacing::ParagraphBreak,
            vec!["RED", "ONE"],
        ),
        (
            "Name | Value\n--- | ---\nRED | literal **\n",
            TextBlockSpacing::ParagraphBreak,
            vec!["RED", "literal **"],
        ),
        (
            "Name | Value\n--- | ---\nRED | literal **",
            TextBlockSpacing::ParagraphBreak,
            vec!["RED", "literal **"],
        ),
        (
            "One | Two\n--- | --- | ---\nRED | ONE | TWO\n",
            TextBlockSpacing::None,
            vec!["RED", "ONE", "TWO"],
        ),
        ("````md\n```\nliteral **\n````\n", TextBlockSpacing::None, vec!["```", "literal **"]),
        ("````md\n```\nliteral **\n````", TextBlockSpacing::None, vec!["```", "literal **"]),
    ] {
        let first = TextBlock::from_complete(source).with_trailing_spacing(spacing);
        let prefix = HistoryOutputId::Block(first.id);
        let tail = TextBlock::from_complete("TAIL_ORCHID | independent");
        let tail_id = HistoryOutputId::Block(tail.id);
        let mut app = App::test_default();
        app.push_message_tracked(assistant(vec![
            MessageBlock::Text(first),
            MessageBlock::Text(tail),
        ]));
        app.bind_active_turn_assistant(0);
        app.status = AppStatus::Running;
        for width in [40, 80] {
            let rows = render(&mut app, width, &BTreeSet::new());
            let own = text(&region(&rows, &prefix)).join("\n");
            for needle in &required {
                assert!(own.contains(needle), "{needle}: {own}");
            }
            assert!(!own.contains("TAIL_ORCHID"));
            assert!(
                rows.segments()
                    .iter()
                    .filter(|s| s.ids.contains(&prefix))
                    .all(|s| s.commit_ready && s.end_row <= rows.stable_row_count())
            );
            assert!(
                rows.segments()
                    .iter()
                    .filter(|s| s.ids.contains(&tail_id))
                    .all(|s| !s.commit_ready)
            );
            let excluded = rows
                .segments()
                .iter()
                .filter(|s| s.commit_ready)
                .flat_map(|s| s.ids.clone())
                .collect();
            let remaining = render(&mut app, width, &excluded);
            assert!(
                text(&region(&remaining, &tail_id))
                    .join("\n")
                    .contains("TAIL_ORCHID | independent")
            );
            assert!(remaining.segments().iter().all(|s| !s.ids.contains(&prefix)));
        }
    }
}
