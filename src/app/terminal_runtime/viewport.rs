// SPDX-License-Identifier: Apache-2.0

//! One viewport policy for live following, explicit reading, and reflow.

use crate::app::HistoryOutputId;
use crate::ui::inline_chat_rows::SerializedLiveRows;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ChatViewport {
    reading: bool,
    anchor: Option<RowAnchor>,
    pending_scroll: isize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowAnchor {
    id: HistoryOutputId,
    text_offset: usize,
    empty_rows: usize,
}

impl ChatViewport {
    pub(crate) const fn is_reading(&self) -> bool {
        self.reading
    }

    pub(crate) fn pause(&mut self) {
        self.reading = true;
    }

    pub(crate) fn resume(&mut self) {
        self.reading = false;
        self.anchor = None;
        self.pending_scroll = 0;
    }

    pub(crate) fn scroll(&mut self, rows: isize) {
        self.pause();
        self.pending_scroll = self.pending_scroll.saturating_add(rows);
    }

    pub(crate) fn start(&mut self, rows: &SerializedLiveRows, height: usize) -> usize {
        let last_start = rows.rows().len().saturating_sub(height);
        let start = self
            .anchor
            .as_ref()
            .and_then(|anchor| {
                rows.segments()
                    .iter()
                    .find(|segment| segment.ids.contains(&anchor.id))
                    .map(|segment| anchor.row(rows.rows(), segment.start_row, segment.end_row))
            })
            .unwrap_or_else(|| if self.anchor.is_some() { 0 } else { last_start });
        let start = start.saturating_add_signed(self.pending_scroll).min(last_start);
        self.pending_scroll = 0;
        start
    }

    pub(crate) fn record(&mut self, rows: &SerializedLiveRows, start: usize) {
        self.anchor =
            rows.segments().iter().find(|segment| segment.end_row > start).and_then(|segment| {
                segment.ids.first().map(|id| {
                    let preceding = &rows.rows()[segment.start_row..start.max(segment.start_row)];
                    RowAnchor {
                        id: id.clone(),
                        text_offset: preceding.iter().map(content_len).sum(),
                        empty_rows: preceding
                            .iter()
                            .rev()
                            .take_while(|row| content_len(row) == 0)
                            .count(),
                    }
                })
            });
    }
}

impl RowAnchor {
    fn row(&self, rows: &[ratatui::text::Line<'static>], start: usize, end: usize) -> usize {
        let mut offset = 0;
        for (index, row) in rows[start..end].iter().enumerate() {
            if offset == self.text_offset {
                return (start + index + self.empty_rows).min(end.saturating_sub(1));
            }
            offset += content_len(row);
            if offset > self.text_offset {
                return start + index;
            }
        }
        end.saturating_sub(1)
    }
}

// Soft wrapping changes whitespace/indentation but keeps the rendered content
// stream. Anchor within that stream rather than within its physical rows.
fn content_len(row: &ratatui::text::Line<'_>) -> usize {
    row.spans.iter().flat_map(|span| span.content.chars()).filter(|ch| !ch.is_whitespace()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, ChatMessage, MessageBlock, MessageRole, TextBlock};

    fn rows(app: &mut App, width: u16) -> SerializedLiveRows {
        crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            app,
            width,
            &std::collections::BTreeSet::default(),
        )
    }

    #[test]
    fn reflow_keeps_the_same_text_inside_a_long_response_segment() {
        let mut app = App::test_default();
        let paragraph = (0..200).map(|index| format!("word{index}")).collect::<Vec<_>>().join(" ");
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete(&paragraph))],
            None,
        ));
        let wide = rows(&mut app, 80);
        let start = 12;
        let old_text =
            wide.rows()[start].spans.iter().map(|span| span.content.as_ref()).collect::<String>();
        let word = old_text.split_whitespace().next().expect("anchor word");
        let mut viewport = ChatViewport::default();
        viewport.record(&wide, start);
        viewport.pause();
        let narrow = rows(&mut app, 24);
        let reflowed = viewport.start(&narrow, 6);
        let text = narrow.rows()[reflowed]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains(word), "expected {word} in {text}");
    }

    #[test]
    fn remapped_live_action_and_wrapped_hint_use_the_same_key_catalog() {
        use crate::app::keymap::{
            AppAction, KeyAction, KeyBinding, KeyBindingSource, KeyContext, ResolvedKeymap,
        };
        let mut app = App::test_default();
        app.chat_render.viewport.pause();
        app.keymap = ResolvedKeymap::from_bindings([KeyBinding::new(
            KeyContext::Global,
            "f7".parse().expect("key"),
            KeyAction::App(AppAction::FollowChat),
            KeyBindingSource::Config,
        )])
        .expect("keymap");
        let hint = crate::ui::input_rows::reading_hint_rows(&app, 12);
        assert!(hint.len() > 1);
        let text = hint
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains("F7 live"), "{text}");
        crate::app::events::handle_terminal_event(
            &mut app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::F(7),
                crossterm::event::KeyModifiers::NONE,
            )),
        );
        assert!(!app.chat_render.viewport.is_reading());
    }

    #[test]
    fn reading_survives_new_output_and_reflow_and_end_explicitly_returns_live() {
        let mut app = App::test_default();
        for number in 0..30 {
            app.transcript.messages.push(ChatMessage::new(
                MessageRole::User,
                vec![MessageBlock::Text(TextBlock::from_complete(&format!(
                    "Message {number} carries long text that reflows during resize"
                )))],
                None,
            ));
        }
        let original = rows(&mut app, 80);
        let mut viewport = ChatViewport::default();
        viewport.record(&original, 10);
        viewport.pause();
        let pinned_id = viewport.anchor.as_ref().expect("anchor").id.clone();
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("New output"))],
            None,
        ));
        let expanded = rows(&mut app, 80);
        assert_eq!(viewport.start(&expanded, 8), 10);
        let narrow = rows(&mut app, 24);
        let start = viewport.start(&narrow, 8);
        assert!(narrow.segments().iter().any(|segment| segment.ids.contains(&pinned_id)
            && segment.start_row <= start
            && segment.end_row > start));
        viewport.record(&narrow, start);
        viewport.scroll(-6);
        assert_eq!(viewport.start(&narrow, 8), start.saturating_sub(6));
        app.chat_render.viewport = viewport;
        crate::app::events::handle_terminal_event(
            &mut app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::End,
                crossterm::event::KeyModifiers::CONTROL,
            )),
        );
        assert!(!app.chat_render.viewport.is_reading());
        assert_eq!(app.chat_render.viewport.start(&narrow, 8), narrow.rows().len() - 8);
    }
}
