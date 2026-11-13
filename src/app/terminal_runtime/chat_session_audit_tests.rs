// SPDX-License-Identifier: Apache-2.0

use super::super::chat_terminal::test_io::TestIo;
use crate::ui::layout_control::LayoutControl;
use std::fmt::Write as _;

fn alignment_fixture(
    tool_first: bool,
) -> (App, crate::app::HistoryOutputId, crate::app::ChatMessageId, crate::app::HistoryOutputId) {
    use crate::app::{
        ChatMessage, HistoryOutputId, ImageAttachmentBlock, MessageBlock, MessageRole, TextBlock,
    };
    let mut app = App::test_default();
    app.input.set_text("DRAFT_ALIGNMENT");
    let user = ChatMessage::new(
        MessageRole::User,
        vec![
            MessageBlock::Text(TextBlock::from_complete("ALPHA_PARAGRAPH\n\nOMEGA_PARAGRAPH")),
            MessageBlock::ImageAttachment(ImageAttachmentBlock::new(2)),
        ],
        None,
    );
    let user_id = HistoryOutputId::Message(user.id);
    app.push_message_tracked(user);
    let mut tool = crate::app::test_support::named_tool_call_info(
        "aligned-tool",
        "Bash",
        crate::agent::model::ToolCallStatus::Completed,
    );
    tool.title = "TOOL_ALIGNMENT".into();
    let mut blocks = vec![
        MessageBlock::Text(TextBlock::from_complete("ASSISTANT_ALIGNMENT")),
        MessageBlock::ToolCall(Box::new(tool)),
    ];
    if tool_first {
        blocks.reverse();
    }
    let assistant = ChatMessage::new(MessageRole::Assistant, blocks, None);
    let assistant_id = assistant.id;
    app.push_message_tracked(assistant);
    let neighbor = ChatMessage::new(
        MessageRole::System(Some(crate::app::SystemSeverity::Info)),
        vec![MessageBlock::Text(TextBlock::from_complete("STATIC_ALIGNMENT"))],
        None,
    );
    let neighbor_id = HistoryOutputId::Message(neighbor.id);
    app.push_message_tracked(neighbor);
    (app, user_id, assistant_id, neighbor_id)
}

fn alignment_position(buffer: &ratatui::buffer::Buffer, region: Rect, needle: &str) -> (u16, u16) {
    let hits: Vec<_> = (region.y..region.bottom())
        .filter_map(|y| {
            let rendered_row =
                (region.x..region.right()).map(|x| buffer[(x, y)].symbol()).collect::<String>();
            rendered_row.find(needle).map(|x| {
                (
                    region.x
                        + u16::try_from(unicode_width::UnicodeWidthStr::width(&rendered_row[..x]))
                            .unwrap(),
                    y,
                )
            })
        })
        .collect();
    assert_eq!(hits.len(), 1, "{needle} must appear once in its actual region");
    hits[0]
}

#[test]
fn submitted_and_assistant_regions_align_with_the_actual_composer_without_background_leaks() {
    use crate::app::HistoryOutputId;
    use crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding;
    for width in [38, 80] {
        for tool_first in [false, true] {
            let (mut app, user_id, assistant_id, neighbor_id) = alignment_fixture(tool_first);
            let serialized = serialize_live_rows_with_boundaries_excluding(
                &mut app,
                width,
                &BTreeSet::new(),
                None,
            );
            let live = serialized.rows();
            let surface = ChatTerminalSession::build_composer_surface(&mut app, width);
            let plan = MutableLayoutPlan::new(live, &surface, 64);
            let area = Rect::new(0, 0, width, 64);
            let (la, _, _, _, ea, _) = plan.areas(area);
            let visible = plan.live_visible_rows(live).to_vec();
            assert_eq!(visible, live, "fixture must keep every target visible");
            let mut terminal = Terminal::new(TestBackend::new(width, 64)).unwrap();
            terminal
                .draw(|frame| {
                    super::render_mutable_frame(frame, &mut app, &surface, plan, &visible, area);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let locate = |region, needle| alignment_position(buffer, region, needle);
            let (composer_x, composer_y) = locate(ea, "DRAFT_ALIGNMENT");
            let background = buffer[(composer_x, composer_y)].bg;
            assert_ne!(background, ratatui::style::Color::Reset);
            let segments = |id: &HistoryOutputId| {
                serialized.segments().iter().filter(|s| s.ids.contains(id)).collect::<Vec<_>>()
            };
            let user_segments = segments(&user_id);
            assert!(!user_segments.is_empty());
            for needle in ["ALPHA_PARAGRAPH", "OMEGA_PARAGRAPH", "[img] 2 images attached"] {
                let (x, y) = locate(la, needle);
                assert_eq!(x, composer_x);
                assert!(
                    user_segments.iter().any(|s| y >= la.y + u16::try_from(s.start_row).unwrap()
                        && y < la.y + u16::try_from(s.end_row).unwrap())
                );
            }
            for segment in &user_segments {
                for y in segment.start_row..segment.end_row {
                    for x in 0..width {
                        assert_eq!(buffer[(x, la.y + u16::try_from(y).unwrap())].bg, background);
                    }
                }
            }
            let (assistant_x, assistant_y) = locate(la, "ASSISTANT_ALIGNMENT");
            assert_eq!(assistant_x, composer_x);
            assert_ne!(buffer[(assistant_x, assistant_y)].bg, background);
            let (_, tool_y) = locate(la, "TOOL_ALIGNMENT");
            let status_x =
                (0..width).find(|x| !buffer[(*x, tool_y)].symbol().trim().is_empty()).unwrap();
            // In a tool-first response the speaker marker precedes the status; the tool's
            // own framed content still begins at the composer column.
            let tool_row = live[usize::from(tool_y - la.y)].to_string();
            let status = tool_row.trim_start_matches([' ', '\u{25cf}']).chars().next().unwrap();
            let actual_status_x = (status_x..width)
                .find(|x| buffer[(*x, tool_y)].symbol() == status.to_string())
                .unwrap();
            assert_eq!(actual_status_x, composer_x);
            let labels = segments(&HistoryOutputId::AssistantLabel(assistant_id));
            assert!(!labels.is_empty());
            assert_eq!(
                (la.y..la.bottom())
                    .flat_map(|y| (0..width).map(move |x| (x, y)))
                    .filter(|&(x, y)| buffer[(x, y)].symbol() == "\u{25cf}")
                    .count(),
                1
            );
            let (nx, ny) = locate(la, "STATIC_ALIGNMENT");
            assert_ne!(buffer[(nx, ny)].bg, background);
            assert!(!segments(&neighbor_id).is_empty());
        }
    }
}

struct SessionFixture {
    session: super::ChatTerminalSession<TestIo>,
    io: TestIo,
    app: App,
}

impl SessionFixture {
    fn new(width: u16, height: u16) -> Self {
        let io = TestIo::new(width, height);
        let mut app = App::test_default();
        app.chat_render.line_wrap_disabled = true;
        app.chat_render.terminal_width = width;
        app.chat_render.terminal_height = height;
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("test-session"));
        Self {
            session: super::ChatTerminalSession {
                terminal: ChatTerminal::with_io(0, io.clone()),
                history: HistoryCommitState::default(),
                preparation: None,
            },
            io,
            app,
        }
    }

    fn message(&mut self, text: &str) {
        self.app.push_message_tracked(assistant_blocks_message(vec![MessageBlock::Text(
            TextBlock::from_complete(text),
        )]));
    }

    fn settle(&mut self) {
        for _ in 0..1000 {
            self.session.apply_rebuild(&mut self.app);
            self.session
                .update_with_budget(&mut self.app, Some(std::time::Duration::ZERO))
                .unwrap();
            if !self.session.has_pending_work() && !self.app.surface_dirty.chat.repaint {
                return;
            }
        }
        panic!("session did not settle");
    }

    fn replay(&mut self) {
        self.app.request_chat_purge_replay_rebuild(
            crate::app::ChatPurgeReplayOptions::terminal_history_out_of_sync(),
        );
    }

    fn resize(&mut self, width: u16, height: u16) {
        self.io.resize(width, height);
        self.app.chat_render.terminal_width = width;
        self.app.chat_render.terminal_height = height;
        self.replay();
        self.settle();
    }

    fn live(&self) -> Vec<String> {
        let area = self.session.terminal.rendered_area();
        self.io.screen_rows()[usize::from(area.y)
            ..usize::from(area.y + self.app.chat_render.live_region.last_rendered_rows)]
            .to_vec()
    }

    fn history(&self) -> Vec<String> {
        let rows = self.io.transcript();
        let end = rows.len() - self.io.screen_rows().len()
            + usize::from(self.session.terminal.rendered_area().y);
        rows[..end].to_vec()
    }

    fn press(&mut self, code: crossterm::event::KeyCode) {
        press_composer_key(&mut self.app, code, crossterm::event::KeyModifiers::NONE);
        self.settle();
    }

    fn complete_turn(&mut self) {
        crate::app::events::handle_client_event(
            &mut self.app,
            crate::agent::events::ClientEvent::TurnComplete {
                session_id: "test-session".into(),
                queued_turn_count: Some(0),
                terminal_reason: None,
            },
        );
    }
}

fn occurrences(rows: &[String], text: &str) -> usize {
    rows.iter().filter(|row| row.contains(text)).count()
}

#[test]
fn failed_session_draw_restores_canonical_history_only_after_resize_recovery() {
    for resize in [true, false] {
        let mut f = SessionFixture::new(80, 20);
        f.message("PREVIOUS_COMMIT");
        f.app.input.set_text("UNSENT_DRAFT");
        f.settle();
        let confirmed = f.session.history.confirmed_ids().clone();
        f.app.push_message_tracked(assistant_blocks_message(vec![
            MessageBlock::Text(TextBlock::from_complete("NEW_STABLE")),
            MessageBlock::Text(TextBlock::new("MUTABLE_TAIL".into())),
        ]));
        f.app.bind_active_turn_assistant(1);
        f.app.status = AppStatus::Running;
        f.io.take_operations();
        if resize {
            f.io.resize_after_draw(60, 20);
        } else {
            f.io.fail_on(Some("draw-after"));
        }
        let outcome = f.session.update(&mut f.app);
        assert!(f.session.history.confirmed_ids().is_subset(&confirmed));
        assert_eq!(
            occurrences(&f.io.transcript(), "NEW_STABLE"),
            1,
            "failure must happen after insertion starts"
        );
        assert!(!f.app.chat_render.live_region.anchor_valid, "failed frame cannot complete");
        assert_eq!(f.app.input.text(), "UNSENT_DRAFT");
        if resize {
            outcome.unwrap();
            // Consume the real requested rebuild after the resize owner reconciles dimensions.
            f.app.chat_render.terminal_width = 60;
            f.settle();
            let transcript = f.io.transcript();
            for text in ["PREVIOUS_COMMIT", "NEW_STABLE", "MUTABLE_TAIL", "UNSENT_DRAFT"] {
                assert_eq!(occurrences(&transcript, text), 1, "{text}: {transcript:?}");
            }
            assert!(f.live().iter().any(|row| row.contains("MUTABLE_TAIL")));
            f.app.request_chat_repaint();
            f.settle();
            assert_eq!(occurrences(&f.io.transcript(), "NEW_STABLE"), 1);
            f.complete_turn();
            f.settle();
            assert!(f.live().iter().all(|row| !row.contains("MUTABLE_TAIL")));
            assert_eq!(occurrences(&f.io.transcript(), "MUTABLE_TAIL"), 1);
        } else {
            let error = outcome.unwrap_err();
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().to_string(),
                "injected draw-after failure"
            );
            // Ordinary writer errors propagate; there is no invented retry workflow.
        }
    }
}

#[test]
fn cooperative_read_preparation_keeps_input_usable_and_delivers_one_preview() {
    use crate::agent::model;
    let mut f = SessionFixture::new(80, 20);
    let mut tool =
        crate::app::test_support::tool_call_info("preview.txt", model::ToolCallStatus::Completed);
    let mut content = String::new();
    for n in 1..=107 {
        writeln!(content, "READ_ROW_{n:03}").unwrap();
    }
    tool.content = vec![model::ToolCallContent::from(content)];
    f.app.push_message_tracked(assistant_blocks_message(vec![MessageBlock::ToolCall(Box::new(
        tool,
    ))]));
    f.replay();
    f.session.apply_rebuild(&mut f.app);
    f.app.surface_dirty.chat.take_repaint();
    let mut edited = false;
    for _ in 0..1000 {
        let control = LayoutControl::with_budget(&|| false, std::time::Duration::ZERO);
        let excluded = f.session.base_history_excluded_ids();
        let prepared = f.session.prepare_rows(&mut f.app, 80, &excluded, &control).unwrap();
        if prepared.rows().is_empty() && !f.session.needs_draw(&f.app, &prepared) && !edited {
            crate::app::events::handle_terminal_event(
                &mut f.app,
                crossterm::event::Event::Paste("DRAFT_DURING_PREPARATION".into()),
            );
            crate::app::finalize_pending_paste_event(&mut f.app);
            assert!(f.session.needs_draw(&f.app, &prepared));
            f.session
                .draw_incremental(&mut f.app, (80, 20), 80, 20, &prepared, excluded.clone())
                .unwrap();
            assert_eq!(f.app.input.text(), "DRAFT_DURING_PREPARATION");
            let area = f.session.terminal.rendered_area();
            let editor_bottom = area.bottom() - f.app.chat_render.composer.footer_rows;
            let editor_top = editor_bottom - f.app.chat_render.composer.editor_rows;
            assert!(
                f.io.screen_rows()[usize::from(editor_top)..usize::from(editor_bottom)]
                    .iter()
                    .any(|s| s.contains("DRAFT_DURING_PREPARATION"))
            );
            assert!(f.session.has_pending_work());
            edited = true;
        } else if f.session.needs_draw(&f.app, &prepared) {
            f.session.draw_incremental(&mut f.app, (80, 20), 80, 20, &prepared, excluded).unwrap();
        }
        if !f.session.has_pending_work() {
            break;
        }
    }
    assert!(edited, "fixture must yield before preview completion");
    assert!(!f.session.has_pending_work());
    let transcript = f.io.transcript();
    let actual: Vec<_> = transcript
        .iter()
        .filter_map(|s| s.split_once("READ_ROW_"))
        .map(|(_, n)| n.trim().parse::<usize>().unwrap())
        .collect();
    assert_eq!(actual, [1, 2, 3, 104, 105, 106, 107]);
    let omissions: Vec<_> = transcript.iter().filter(|s| s.contains("hidden")).collect();
    assert_eq!(omissions.len(), 1);
    assert!(omissions[0].contains("100"));
}

#[test]
fn replay_completion_paints_the_live_suffix_without_another_event() {
    let mut f = SessionFixture::new(60, 20);
    f.message("STABLE_PREFIX");
    f.message("LIVE_RESPONSE");
    f.app.bind_active_turn_assistant(1);
    f.app.status = AppStatus::Running;
    f.replay();
    f.settle();
    assert_eq!(occurrences(&f.io.transcript(), "STABLE_PREFIX"), 1);
    assert_eq!(occurrences(&f.live(), "LIVE_RESPONSE"), 1);
    assert_eq!(occurrences(&f.live(), "STABLE_PREFIX"), 0);
}

#[test]
fn replay_backpressure_delivers_same_id_continuations_in_source_order() {
    let mut f = SessionFixture::new(80, 20);
    f.message("FIRST_ITEM");
    let mut body = String::new();
    for n in 0..70 {
        writeln!(body, "CHUNK_{n:03}").unwrap();
    }
    f.message(&format!("```text\n{body}```"));
    f.message("LAST_ITEM");
    f.replay();
    f.session.apply_rebuild(&mut f.app);
    let mut seen_ids = BTreeSet::new();
    let mut continuation = false;
    for _ in 0..1000 {
        let excluded = f.session.base_history_excluded_ids();
        let control = LayoutControl::with_budget(&|| false, std::time::Duration::ZERO);
        let rows = f.session.prepare_rows(&mut f.app, 80, &excluded, &control).unwrap();
        for segment in rows.segments() {
            for id in &segment.ids {
                if seen_ids.contains(id)
                    && rows.rows().iter().any(|r| line_text(r).contains("CHUNK_"))
                {
                    continuation = true;
                    assert!(!f.session.history.confirmed_ids().contains(id));
                }
                seen_ids.insert(id.clone());
            }
        }
        let mut plan = f.session.prepare_history_flush(&rows, 80, excluded.clone());
        f.session.queue_history_plan(plan.take_action());
        if f.session.terminal.has_pending_history_rows() {
            let blocked = f.session.prepare_rows(&mut f.app, 80, &excluded, &control).unwrap();
            assert!(blocked.rows().is_empty());
        }
        f.session.draw_incremental(&mut f.app, (80, 20), 80, 20, &rows, excluded).unwrap();
        if !f.session.has_pending_work() {
            break;
        }
    }
    assert!(continuation, "fixture must deliver one semantic block across chunks");
    assert!(!f.session.has_pending_work());
    let transcript = f.io.transcript();
    let actual: Vec<_> = transcript
        .iter()
        .filter_map(|row| {
            if row.contains("FIRST_ITEM") {
                Some("FIRST_ITEM".to_owned())
            } else if row.contains("LAST_ITEM") {
                Some("LAST_ITEM".to_owned())
            } else {
                row.find("CHUNK_").map(|i| row[i..].trim().to_owned())
            }
        })
        .collect();
    let expected: Vec<_> = std::iter::once("FIRST_ITEM".to_owned())
        .chain((0..70).map(|n| format!("CHUNK_{n:03}")))
        .chain(std::iter::once("LAST_ITEM".to_owned()))
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn stable_batches_keep_table_cells_and_exclude_confirmed_and_mutable_content() {
    for content in [
        "NEW_PROSE",
        "| ITEM | LEFT | RIGHT |\n| --- | --- | --- |\n| ALPHA | A1 | B1 |\n| BETA | | B2 |\n",
    ] {
        let mut f = SessionFixture::new(80, 20);
        f.message("ALREADY_COMMITTED");
        f.settle();
        f.app.push_message_tracked(assistant_blocks_message(vec![
            MessageBlock::Text(
                TextBlock::from_complete(content)
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::new("LIVE_UNFINISHED".into())),
        ]));
        f.app.bind_active_turn_assistant(1);
        f.app.status = AppStatus::Running;
        f.settle();
        let history = f.history();
        assert_eq!(occurrences(&history, "ALREADY_COMMITTED"), 1);
        assert_eq!(occurrences(&history, "LIVE_UNFINISHED"), 0);
        assert_eq!(occurrences(&f.live(), "LIVE_UNFINISHED"), 1);
        if content == "NEW_PROSE" {
            assert_eq!(occurrences(&history, content), 1);
        } else {
            let header = history.iter().find(|r| r.contains("ITEM")).unwrap();
            let left = header.find("LEFT").unwrap();
            let right = header.find("RIGHT").unwrap();
            let alpha = history.iter().find(|r| r.contains("ALPHA")).unwrap();
            let beta = history.iter().find(|r| r.contains("BETA")).unwrap();
            assert!(alpha[..left].contains("ALPHA"));
            assert!(alpha[left..right].contains("A1"));
            assert!(alpha[right..].contains("B1"));
            assert!(beta[..left].contains("BETA"));
            assert!(!beta[left..right].chars().any(char::is_alphanumeric), "empty cell: {beta}");
            assert!(beta[right..].contains("B2"));
            assert_eq!(occurrences(&history, "ALPHA"), 1);
            assert_eq!(occurrences(&history, "BETA"), 1);
        }
        f.app.request_chat_repaint();
        f.settle();
        assert_eq!(f.history(), history, "successful confirmation prevents reinsertion");
        f.complete_turn();
        f.settle();
        assert_eq!(occurrences(&f.history(), "LIVE_UNFINISHED"), 1);
        assert_eq!(occurrences(&f.live(), "LIVE_UNFINISHED"), 0);
    }
}

#[test]
fn completed_btw_exchange_stays_stable_across_streaming_and_replay() {
    use crate::agent::{events::ClientEvent, model};
    let mut f = SessionFixture::new(80, 20);
    f.app.push_message_tracked(assistant_blocks_message(vec![
        MessageBlock::Text(TextBlock::new("BEFORE_CARD".into())),
        MessageBlock::BtwExchange(crate::app::BtwExchangeBlock::new(
            "SIDE_QUESTION".into(),
            "SIDE_ANSWER".into(),
        )),
    ]));
    f.app.bind_active_turn_assistant(0);
    f.app.status = AppStatus::Running;
    f.settle();
    for text in ["BEFORE_CARD", "SIDE_QUESTION", "SIDE_ANSWER"] {
        assert_eq!(occurrences(&f.history(), text), 1);
    }
    crate::app::events::handle_client_event(
        &mut f.app,
        ClientEvent::SessionUpdate {
            session_id: "test-session".into(),
            update: model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("AFTER_CARD")),
            )),
        },
    );
    f.settle();
    assert_eq!(occurrences(&f.live(), "AFTER_CARD"), 1);
    assert_eq!(occurrences(&f.history(), "AFTER_CARD"), 0);
    f.resize(24, 20);
    let history = f.history();
    for text in ["SIDE_QUESTION", "SIDE_ANSWER"] {
        assert_eq!(occurrences(&history, text), 1);
    }
    assert!(
        history.iter().position(|r| r.contains("SIDE_QUESTION"))
            < history.iter().position(|r| r.contains("SIDE_ANSWER"))
    );
    assert_eq!(occurrences(&history, "AFTER_CARD"), 0);
    assert_eq!(occurrences(&f.live(), "AFTER_CARD"), 1);
    f.complete_turn();
    f.settle();
    for text in ["SIDE_QUESTION", "SIDE_ANSWER", "AFTER_CARD"] {
        assert_eq!(occurrences(&f.history(), text), 1);
    }
    assert!(f.live().iter().all(|r| !r.contains("AFTER_CARD")));
}

#[test]
fn active_turn_activity_is_visible_but_never_enters_committed_history() {
    use crate::agent::{events::ClientEvent, model};
    let mut f = SessionFixture::new(80, 20);
    f.app.push_message_tracked(assistant_blocks_message(vec![
        MessageBlock::Text(TextBlock::from_complete("COMPLETED_PREFIX")),
        MessageBlock::Text(TextBlock::new("ACTIVE_TAIL".into())),
    ]));
    f.app.bind_active_turn_assistant(0);
    f.app.status = AppStatus::Running;
    f.app.begin_turn_activity(std::time::Instant::now());
    crate::app::events::handle_client_event(
        &mut f.app,
        ClientEvent::SessionUpdate {
            session_id: "test-session".into(),
            update: model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        },
    );
    for width in [80, 32] {
        if width == 32 {
            f.resize(width, 20);
        } else {
            f.settle();
        }
        let area = f.session.terminal.rendered_area();
        let activity_top = area.y + f.app.chat_render.live_region.last_rendered_rows;
        let screen = f.io.screen_rows();
        assert!(f.live().iter().any(|r| r.contains("Thinking")), "{screen:?}");
        assert_eq!(occurrences(&f.live(), "ACTIVE_TAIL"), 1);
        assert!(!screen[usize::from(activity_top)].trim().is_empty());
        let history = f.history();
        assert_eq!(occurrences(&history, "COMPLETED_PREFIX"), 1);
        for text in ["Thinking", "ACTIVE_TAIL"] {
            assert_eq!(occurrences(&history, text), 0);
        }
    }
    f.complete_turn();
    f.settle();
    assert_eq!(occurrences(&f.history(), "ACTIVE_TAIL"), 1);
}

#[test]
fn composed_regions_keep_queue_btw_draft_and_selected_live_tail_visible() {
    let mut app = App::test_default();
    app.input.set_text("DRAFT");
    app.input.set_cursor(0, 5);
    app.pending_user_messages
        .try_push_sending(crate::app::PendingUserMessage::sending(
            "q".into(),
            "QUEUED_BODY".into(),
            Vec::new(),
        ))
        .unwrap();
    app.btw.try_push("failed".into(), "FAILED_SIDE".into()).unwrap();
    app.btw.take_next().unwrap();
    assert!(app.btw.fail("failed", "fixture failure".into(), std::time::Instant::now()));
    app.btw.try_push("active".into(), "ACTIVE_SIDE".into()).unwrap();
    app.btw.take_next().unwrap();
    app.btw.try_push("waiting".into(), "WAITING_SIDE".into()).unwrap();
    app.status = AppStatus::Running;
    app.begin_turn_activity(std::time::Instant::now());
    let live = vec![Line::from("LIVE_FIRST"), Line::from("LIVE_MIDDLE"), Line::from("LIVE_LAST")];
    let mut observed = (false, false, false);
    for height in [4, 5, 6, 7, 8, 9, 12, 13, 14, 20] {
        let surface = ChatTerminalSession::build_composer_surface(&mut app, 80);
        let plan = MutableLayoutPlan::new(&live, &surface, height);
        let area = Rect::new(0, 0, 80, height);
        let (la, aa, ha, ba, ea, fa) = plan.areas(area);
        let mut terminal = Terminal::new(TestBackend::new(80, height)).unwrap();
        let visible = plan.live_visible_rows(&live).to_vec();
        terminal
            .draw(|frame| {
                super::render_mutable_frame(frame, &mut app, &surface, plan, &visible, area);
            })
            .unwrap();
        let view = ComposerFrame {
            backend: terminal.backend().clone(),
            hints: ha,
            editor: ea,
            text: input::compute_render_geometry(ea).text,
            footer: fa,
        };
        assert_eq!(view.draft(), "DRAFT");
        view.assert_caret_after("DRAFT");
        assert!(!view.region(fa).trim().is_empty(), "footer controls must remain usable");
        if height == 20 {
            assert!(!view.region(aa).trim().is_empty());
            assert!(view.region(ha).contains("QUEUED_BODY"));
            assert!(view.region(ba).contains("ACTIVE_SIDE"));
            assert!(ha.bottom() <= ba.y && ba.bottom() <= ea.y);
            assert_eq!(
                view.region(la).lines().filter(|s| !s.trim().is_empty()).collect::<Vec<_>>(),
                ["LIVE_FIRST", "LIVE_MIDDLE", "LIVE_LAST"]
            );
            observed.0 = true;
        }
        if ba.height == 1 {
            assert!(view.region(ba).contains("ACTIVE_SIDE"));
            assert!(
                !view.region(ba).contains("FAILED_SIDE")
                    && !view.region(ba).contains("WAITING_SIDE")
            );
            observed.1 = true;
        }
        if la.height == 1 {
            assert_eq!(view.region(la), "LIVE_LAST");
            observed.2 = true;
        }
    }
    assert_eq!(observed, (true, true, true));
    let mut f = SessionFixture::new(80, 20);
    // Initialize at the bottom, then grow with real live content and composer controls.
    f.session.terminal = ChatTerminal::with_io(19, f.io.clone());
    f.message("BOTTOM_FIRST\nBOTTOM_SECOND\nBOTTOM_THIRD");
    f.app.bind_active_turn_assistant(0);
    f.app.status = AppStatus::Running;
    f.app.input.set_text("BOTTOM_DRAFT");
    f.settle();
    for text in ["BOTTOM_FIRST", "BOTTOM_SECOND", "BOTTOM_THIRD"] {
        assert_eq!(occurrences(&f.live(), text), 1);
    }
    assert_eq!(occurrences(&f.io.screen_rows(), "BOTTOM_DRAFT"), 1);
}

impl SessionFixture {
    fn wheel(&mut self, down: bool) {
        use crossterm::event::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        crate::app::events::handle_terminal_event(
            &mut self.app,
            Event::Mouse(MouseEvent {
                kind: if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp },
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }),
        );
        self.settle();
    }

    fn oldest(&mut self) {
        for _ in 0..30 {
            self.press(crossterm::event::KeyCode::PageUp);
        }
    }

    fn pin(&mut self, marker: &str) {
        // Navigate only through public input to an independently named source row.
        for (pages, down) in [(0, true), (1, true), (2, true), (1, false), (2, false)] {
            self.oldest();
            for _ in 0..pages {
                self.press(crossterm::event::KeyCode::PageDown);
            }
            for _ in 0..60 {
                if self.live().first().is_some_and(|r| r.contains(marker)) {
                    return;
                }
                self.wheel(down);
            }
        }
        panic!("cannot navigate to {marker}: {:?}", self.live());
    }

    fn follow(&mut self) {
        press_composer_key(
            &mut self.app,
            crossterm::event::KeyCode::End,
            crossterm::event::KeyModifiers::CONTROL,
        );
        self.settle();
    }

    fn stream(&mut self, text: &str) {
        crate::app::events::handle_client_event(
            &mut self.app,
            crate::agent::events::ClientEvent::SessionUpdate {
                session_id: "test-session".into(),
                update: crate::agent::model::SessionUpdate::AgentMessageChunk(
                    crate::agent::model::ContentChunk::new(
                        crate::agent::model::ContentBlock::Text(
                            crate::agent::model::TextContent::new(text),
                        ),
                    ),
                ),
            },
        );
        self.settle();
    }
}

fn reading_fixture() -> SessionFixture {
    let mut f = SessionFixture::new(80, 20);
    let mut source = String::from("\u{60}\u{60}\u{60}text\n");
    for n in 0..60 {
        if n == 30 {
            source.push_str(
                "ANCHOR_界 alpha beta gamma delta epsilon zeta eta theta\n\nCODE_CONTEXT\n",
            );
        } else {
            writeln!(source, "SOURCE_{n:03}").unwrap();
        }
    }
    source.push_str("\u{60}\u{60}\u{60}\n");
    f.message(&source);
    f.message("CURRENT_TAIL");
    f.app.bind_active_turn_assistant(1);
    f.app.status = AppStatus::Running;
    f.settle();
    f
}

#[test]
fn reading_navigation_reflows_the_recorded_source_and_follows_new_output() {
    let mut f = reading_fixture();
    assert_eq!(occurrences(&f.live(), "CURRENT_TAIL"), 1);
    f.pin("ANCHOR_界");
    for (width, height) in [(24, 20), (80, 14), (24, 20), (80, 14)] {
        f.resize(width, height);
        assert!(f.live()[0].contains("ANCHOR_界"), "{:?}", f.live());
        let context = f.live().iter().position(|r| r.contains("CODE_CONTEXT")).unwrap();
        assert!(f.live()[context - 1].trim().is_empty(), "intentional blank code row survives");
        f.app.request_chat_repaint();
        f.settle();
        assert!(f.live()[0].contains("ANCHOR_界"), "redraw must not consume old scroll again");
    }
    let before = f.live();
    let last = before
        .iter()
        .rev()
        .find_map(|r| r.split_once("SOURCE_").map(|(_, n)| n.trim().parse::<usize>().unwrap()))
        .unwrap();
    f.press(crossterm::event::KeyCode::PageDown);
    assert!(
        f.live()[0].contains(&format!("SOURCE_{:03}", last + 1)),
        "page uses current painted height: {:?}",
        f.live()
    );
    let down = f.live();
    f.app.request_chat_repaint();
    f.settle();
    assert_eq!(f.live(), down);
    f.press(crossterm::event::KeyCode::PageUp);
    assert!(f.live()[0].contains("ANCHOR_界"));
    f.oldest();
    let oldest = f.live();
    f.wheel(false);
    assert_eq!(f.live(), oldest, "upper boundary clamps");
    for _ in 0..30 {
        f.press(crossterm::event::KeyCode::PageDown);
    }
    let bottom = f.live();
    f.wheel(true);
    assert_eq!(f.live(), bottom, "lower boundary clamps");
    f.pin("ANCHOR_界");
    f.stream("\nNEW_WHILE_READING");
    assert!(f.live()[0].contains("ANCHOR_界"));
    f.follow();
    assert_eq!(occurrences(&f.live(), "NEW_WHILE_READING"), 1);
    f.stream("\nNEW_AFTER_FOLLOW");
    assert_eq!(occurrences(&f.live(), "NEW_AFTER_FOLLOW"), 1);
}

#[test]
fn remapped_reading_hint_returns_the_rendered_reader_to_the_current_tail() {
    use crate::app::keymap::{
        AppAction, KeyAction, KeyBinding, KeyBindingSource, KeyContext, ResolvedKeymap,
    };
    let mut f = reading_fixture();
    f.pin("ANCHOR_界");
    f.app.keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
        KeyContext::Global,
        "f7".parse().unwrap(),
        KeyAction::App(AppAction::FollowChat),
        KeyBindingSource::Config,
    )])
    .unwrap();
    f.resize(18, 24);
    assert!(f.live()[0].contains("ANCHOR_界"));
    let area = f.session.terminal.rendered_area();
    let start = area.y
        + f.app.chat_render.live_region.last_rendered_rows
        + f.app.chat_render.composer.activity_rows;
    let end = start + f.app.chat_render.composer.hint_rows;
    let screen = f.io.screen_rows();
    let hint = screen[usize::from(start)..usize::from(end)].join(" ");
    assert!(hint.contains("F7 live"), "{hint}");
    assert!(end - start > 1, "exercise wrapped hint");
    press_composer_key(
        &mut f.app,
        crossterm::event::KeyCode::F(7),
        crossterm::event::KeyModifiers::NONE,
    );
    assert!(f.app.surface_dirty.chat.repaint, "action must schedule a frame");
    f.settle();
    assert_eq!(occurrences(&f.live(), "CURRENT_TAIL"), 1);
    assert_eq!(occurrences(&f.live(), "ANCHOR_界"), 0);
}

#[test]
fn reading_retention_preserves_survivors_and_falls_back_to_available_history() {
    for (remove_anchor, timestamps, marker) in [
        (false, false, "RETAINED_030"),
        (true, false, "RETAINED_030"),
        (false, true, "RETAINED_030"),
        (true, true, "RETAINED_030"),
        (false, false, "RETAINED_000"),
        (false, true, "RETAINED_000"),
    ] {
        let mut f = SessionFixture::new(80, 20);
        let mut snapshot = crate::agent::settings::SettingsSnapshot::test_value(
            "showMessageTimestamps",
            serde_json::json!(timestamps),
        );
        snapshot.values.extend(
            crate::agent::settings::SettingsSnapshot::test_value(
                "timeFormat",
                serde_json::json!("24-hour-utc"),
            )
            .values,
        );
        f.app.config.snapshot = Some(snapshot);
        for (capacity, text) in [
            (131_072, "OLD_ALLOCATION".to_owned()),
            (
                65_536,
                format!(
                    "\u{60}\u{60}\u{60}text\n{}\n\u{60}\u{60}\u{60}",
                    (0..50).map(|n| format!("RETAINED_{n:03}")).collect::<Vec<_>>().join("\n")
                ),
            ),
            (1_024, "OLDEST_SURVIVOR".to_owned()),
            (1_024, "CURRENT_TAIL".to_owned()),
        ] {
            let mut allocated = String::with_capacity(capacity);
            allocated.push_str(&text);
            let block = TextBlock::new(allocated);
            let mut message = assistant_blocks_message(vec![MessageBlock::Text(block)]);
            crate::app::presentation::MessageTimestamp::apply_sdk(
                &mut message.timing.timestamp,
                "2026-10-03T15:05:00Z",
            );
            f.app.push_message_tracked(message);
        }
        f.app.bind_active_turn_assistant(3);
        f.app.status = AppStatus::Running;
        f.settle();
        if timestamps {
            assert!(f.io.transcript().iter().any(|row| row.contains("15:05")));
        }
        f.pin(marker);
        f.app.history_retention.max_bytes = if remove_anchor { 8_192 } else { 100_000 };
        f.app.enforce_history_retention_tracked();
        assert!(!f.app.transcript.messages.iter().any(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b,MessageBlock::Text(t) if t.text.contains("OLD_ALLOCATION")))
        }));
        f.settle();
        for width in [80, 32, 80] {
            if width != f.app.chat_render.terminal_width {
                f.resize(width, 20);
            }
            if remove_anchor {
                assert!(f.live().iter().all(|r| !r.contains("RETAINED_")));
                let notice = f.live()[..3].join(" ");
                assert!(
                    notice.contains("Older messages") && notice.contains("hidden"),
                    "fallback includes retention notice: {:?}",
                    f.live()
                );
                assert_eq!(occurrences(&f.live(), "OLDEST_SURVIVOR"), 1);
            } else {
                assert!(f.live()[0].contains(marker), "{:?}", f.live());
            }
            let before = f.live();
            f.app.request_chat_repaint();
            f.settle();
            assert_eq!(f.live(), before);
        }
        f.wheel(true);
        f.wheel(false);
        f.follow();
        assert_eq!(occurrences(&f.live(), "CURRENT_TAIL"), 1);
        f.stream("\nAFTER_RETENTION");
        assert_eq!(occurrences(&f.live(), "AFTER_RETENTION"), 1);
    }
}

#[test]
fn blocked_composer_keeps_narrow_drafts_and_restores_editing() {
    use crate::agent::{events::ClientEvent, model};
    for width in [20, 80] {
        for label in [None, Some("Applying mode")] {
            let mut app = App::test_default();
            app.session_runtime.session_id = Some(model::SessionId::new("test-session"));
            app.input.set_text("UNSENT_DRAFT");
            app.input.set_cursor(0, 12);
            app.status = AppStatus::CommandPending;
            app.turn.pending_command_label = label.map(str::to_owned);
            app.turn.pending_command_ack = Some(crate::app::PendingCommandAck::CurrentMode);
            let blocked = draw_composer_fixture(&mut app, width, 18);
            assert!(!blocked.backend.cursor_visible());
            assert_eq!(app.input.text(), "UNSENT_DRAFT");
            if let Some(label) = label {
                assert!(
                    blocked
                        .region(blocked.editor)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .contains(label)
                );
            } else {
                assert_eq!(blocked.draft(), "UNSENT_DRAFT");
            }
            crate::app::events::handle_client_event(
                &mut app,
                ClientEvent::SessionUpdate {
                    session_id: "test-session".into(),
                    update: model::SessionUpdate::ModeStateUpdate(crate::app::ModeState {
                        current_mode_id: "default".into(),
                        current_mode_name: "Default".into(),
                        available_modes: vec![],
                    }),
                },
            );
            let restored = draw_composer_fixture(&mut app, width, 18);
            restored.assert_caret_after("UNSENT_DRAFT");
            app.input.textarea_insert_char('!');
            assert_eq!(draw_composer_fixture(&mut app, width, 18).draft(), "UNSENT_DRAFT!");
        }
        let mut app = App::test_default();
        app.input.set_text("ERROR_DRAFT");
        app.status = AppStatus::Error;
        let error = draw_composer_fixture(&mut app, width, 18);
        let body = error.region(error.editor);
        assert!(body.contains("error") && body.contains("ERROR_DRAFT"), "{body}");
        assert!(!error.backend.cursor_visible());
        assert_eq!(app.input.text(), "ERROR_DRAFT");
    }
}

#[test]
fn unicode_composer_cursor_follows_focus_and_recovers_from_clipped_text() {
    let mut app = App::test_default();
    let draft = "α界 e\u{301} wrapped words\nTAIL";
    app.input.set_text(draft);
    app.input.set_cursor(1, 4);
    let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
    for area in [Rect::new(5, 3, 18, 14), Rect::new(5, 3, 60, 14)] {
        app.input.set_cursor(0, 2);
        let unicode = paint_composer_fixture(&mut app, &mut terminal, area);
        let (glyph_x, glyph_y) = (unicode.text.y..unicode.text.bottom())
            .flat_map(|y| (unicode.text.x..unicode.text.right()).map(move |x| (x, y)))
            .find(|xy| unicode.backend.buffer()[*xy].symbol() == "\u{754c}")
            .unwrap();
        assert!(unicode.backend.cursor_visible());
        assert_eq!(unicode.backend.cursor_position(), (glyph_x + 2, glyph_y).into());
        app.input.set_cursor(1, 4);
        let before = paint_composer_fixture(&mut app, &mut terminal, area);
        before.assert_caret_after("TAIL");
        assert!(before.draft().contains("α界"));
        let position = before.backend.cursor_position();
        let _rx = request_composer_permission(&mut app);
        let blocked = paint_composer_fixture(&mut app, &mut terminal, area);
        assert!(!blocked.backend.cursor_visible());
        assert_eq!(app.input.text(), draft);
        crate::app::config::open_tab(&mut app, crate::app::ConfigTab::Help).unwrap();
        crate::app::handle_terminal_event(&mut app, crossterm::event::Event::Key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Esc, crossterm::event::KeyModifiers::NONE)));
        let still_focused = paint_composer_fixture(&mut app, &mut terminal, area);
        assert!(!still_focused.backend.cursor_visible(), "view return must preserve real interaction focus");
        resolve_composer_permission(&mut app);
        let restored = paint_composer_fixture(&mut app, &mut terminal, area);
        assert_eq!(restored.backend.cursor_position(), position);
        restored.assert_caret_after("TAIL");
    }
    let clipped = draw_composer_fixture(&mut app, 18, 1);
    assert!(!clipped.backend.cursor_visible());
    assert_eq!(app.input.text(), draft);
    let restored = draw_composer_fixture(&mut app, 60, 18);
    restored.assert_caret_after("TAIL");
    assert!(restored.draft().contains("α界"));

    let mut input_owned = App::test_default();
    input_owned.input.set_text("round trip 界 TAIL");
    let mut pending = request_composer_permission(&mut input_owned);
    input_owned.release_focus_target(FocusTarget::Permission);
    crate::app::config::open_tab(&mut input_owned, crate::app::ConfigTab::Help).unwrap();
    crate::app::handle_terminal_event(&mut input_owned, crossterm::event::Event::Key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Esc, crossterm::event::KeyModifiers::NONE)));
    draw_composer_fixture(&mut input_owned, 80, 18).assert_caret_after("TAIL");
    crate::app::handle_terminal_event(&mut input_owned, crossterm::event::Event::Paste("!".into()));
    crate::app::finalize_pending_paste_event(&mut input_owned);
    assert_eq!(input_owned.input.text(), "round trip 界 TAIL!");
    draw_composer_fixture(&mut input_owned, 80, 18).assert_caret_after("TAIL!");
    crate::app::test_support::assert_pending(&mut pending);
}

#[path = "live_feedback_audit_tests.rs"]
mod live_feedback;
