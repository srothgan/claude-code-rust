// SPDX-License-Identifier: Apache-2.0

use super::super::{
    App, MessageBlock, TextBlock, TextBlockSpacing, TextSplitDecision, TextSplitKind,
    TranscriptUpdateSource, default_cache_split_policy, find_text_split,
};
use crate::agent::model;

/// A live SDK response can start after a background notification without a user prompt.
/// Give it a fresh mutable owner instead of extending already committed text.
pub(super) fn begin_live_assistant_response(app: &mut App) {
    let starts_response = match app.status {
        super::AppStatus::Ready | super::AppStatus::Error => true,
        super::AppStatus::Running | super::AppStatus::Thinking => {
            app.active_turn_assistant_idx().is_none()
        }
        super::AppStatus::Connecting | super::AppStatus::CommandPending => false,
    };
    if !starts_response {
        return;
    }
    app.start_assistant_response();
}

pub(super) fn handle_agent_message_chunk(
    app: &mut App,
    chunk: model::ContentChunk,
    source: TranscriptUpdateSource,
) {
    let model::ContentBlock::Text(text) = chunk.content else {
        return;
    };

    if text.text.is_empty() {
        return;
    }
    let owner_idx = app.ensure_assistant_output_owner(source);
    append_agent_stream_text(
        &mut app.transcript.messages[owner_idx].blocks,
        &text.text,
        chunk.source_message_uuid.as_deref(),
    );
    app.sync_after_message_blocks_changed(owner_idx);
}

pub(super) fn append_agent_stream_text(
    blocks: &mut Vec<MessageBlock>,
    chunk: &str,
    source_message_uuid: Option<&str>,
) {
    if chunk.is_empty() {
        return;
    }
    if let Some(MessageBlock::Text(block)) = blocks.last_mut() {
        block.text.push_str(chunk);
        block.add_source_message_uuid(source_message_uuid);
        block.cache.invalidate();
    } else {
        blocks.push(new_text_block(chunk.to_owned(), source_message_uuid));
    }

    split_tail_text_block(blocks);
}

fn new_text_block(text: String, source_message_uuid: Option<&str>) -> MessageBlock {
    MessageBlock::Text(TextBlock::new(text).with_source_message_uuid(source_message_uuid))
}

fn split_tail_text_block(blocks: &mut Vec<MessageBlock>) {
    loop {
        let Some(tail_idx) = blocks.len().checked_sub(1) else {
            break;
        };
        let Some(split) = blocks.get(tail_idx).and_then(|block| {
            if let MessageBlock::Text(block) = block {
                find_text_block_split(block.text.as_str())
            } else {
                None
            }
        }) else {
            break;
        };

        let (tail_id, source_message_uuids, completed, remainder) = match blocks.get(tail_idx) {
            Some(MessageBlock::Text(block)) => (
                block.id,
                block.source_message_uuids.clone(),
                block.text[..split.split_at].to_owned(),
                block.text[split.split_at..].to_owned(),
            ),
            _ => break,
        };

        if completed.is_empty() || remainder.is_empty() {
            break;
        }

        blocks[tail_idx] = MessageBlock::Text(
            TextBlock::new_with_id(tail_id, remainder)
                .with_source_message_uuids(source_message_uuids.clone()),
        );
        blocks.insert(tail_idx, completed_text_block(completed, split, source_message_uuids));
    }
}

fn completed_text_block(
    text: String,
    split: TextSplitDecision,
    source_message_uuids: Vec<String>,
) -> MessageBlock {
    let trailing_spacing = match split.kind {
        TextSplitKind::Generic => TextBlockSpacing::Continuation,
        TextSplitKind::ParagraphBoundary => TextBlockSpacing::ParagraphBreak,
    };
    MessageBlock::Text(
        TextBlock::new(text)
            .with_source_message_uuids(source_message_uuids)
            .with_trailing_spacing(trailing_spacing),
    )
}

pub(super) fn find_text_block_split(text: &str) -> Option<TextSplitDecision> {
    find_text_split(text, *default_cache_split_policy())
}

#[cfg(test)]
mod tests {
    use super::{TranscriptUpdateSource, handle_agent_message_chunk};
    use crate::agent::model;
    use crate::app::{App, ChatMessage, MessageBlock, MessageRole, TextBlockSpacing};

    #[test]
    fn streaming_text_chunk_appends_to_canonical_active_assistant_message() {
        let mut app = App::test_default();
        app.status = crate::app::AppStatus::Thinking;
        app.transcript.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);

        handle_agent_message_chunk(
            &mut app,
            model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                "first\n\nsecond",
            ))),
            TranscriptUpdateSource::Live,
        );

        assert_eq!(app.active_turn_assistant_idx(), Some(0));
        assert_eq!(app.transcript.messages[0].blocks.len(), 2);
        let Some(MessageBlock::Text(first)) = app.transcript.messages[0].blocks.first() else {
            panic!("expected first text block");
        };
        let Some(MessageBlock::Text(second)) = app.transcript.messages[0].blocks.get(1) else {
            panic!("expected second text block");
        };
        assert_eq!(first.text, "first\n\n");
        assert_eq!(first.trailing_spacing, TextBlockSpacing::ParagraphBreak);
        assert_eq!(second.text, "second");
        assert_eq!(second.trailing_spacing, TextBlockSpacing::None);
    }

    #[test]
    fn streaming_text_keeps_contiguous_chunks_in_one_block_when_source_uuid_changes() {
        let mut app = App::test_default();
        app.status = crate::app::AppStatus::Thinking;
        app.transcript.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);

        for (text, source_message_uuid) in
            [("H", "assistant-part-1"), ("ello!", "assistant-part-2")]
        {
            handle_agent_message_chunk(
                &mut app,
                model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(text)))
                    .source_message_uuid(Some(source_message_uuid.to_owned())),
                TranscriptUpdateSource::Live,
            );
        }

        assert_eq!(app.transcript.messages[0].blocks.len(), 1);
        let Some(MessageBlock::Text(block)) = app.transcript.messages[0].blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Hello!");
        assert!(block.has_source_message_uuid("assistant-part-1"));
        assert!(block.has_source_message_uuid("assistant-part-2"));
    }

    fn assert_default_split(prefix: &str, suffix: &str, kind: super::TextSplitKind) {
        let source = format!("{prefix}{suffix}");
        let split = super::find_text_block_split(&source).expect("eligible source boundary");
        assert_eq!(split.kind, kind);
        assert_eq!(split.split_at, prefix.len());
        assert!(source.is_char_boundary(split.split_at));
        assert!(!prefix.is_empty() && !suffix.is_empty());
        assert_eq!(&source[..split.split_at], prefix);
        assert_eq!(&source[split.split_at..], suffix);
    }

    #[test]
    fn split_prefers_double_newline() {
        assert_default_split("first\n\n", "second", super::TextSplitKind::ParagraphBoundary);
        assert_eq!(super::find_text_block_split("first\n\n"), None);
    }

    #[test]
    fn split_soft_limit_prefers_newline() {
        let policy = super::default_cache_split_policy();
        let exact = format!("{}\n", "a".repeat(policy.soft_limit_bytes - 1));
        assert_default_split(&exact, &"b".repeat(32), super::TextSplitKind::Generic);
        // T072/9's candidate precedes the threshold; growth, not candidate position, enables it.
        let early = "ordinary paragraph line\n";
        let growth = "b".repeat(policy.soft_limit_bytes - early.len());
        assert_default_split(early, &growth, super::TextSplitKind::Generic);
    }

    #[test]
    fn split_hard_limit_uses_sentence_when_needed() {
        let prefix =
            format!("{}.", "a".repeat(super::default_cache_split_policy().hard_limit_bytes + 32));
        assert_default_split(&prefix, " tail", super::TextSplitKind::Generic);
    }

    #[test]
    fn split_ignores_double_newline_inside_code_fence() {
        for source in ["```\nline1\n\nline2\n```", "```rust\nfirst\n\nsecond\n```"] {
            assert_eq!(super::find_text_block_split(source), None);
        }
    }

    fn stream(app: &mut App, value: &str, source_uuid: Option<&str>) {
        handle_agent_message_chunk(
            app,
            model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(value)))
                .source_message_uuid(source_uuid.map(str::to_owned)),
            TranscriptUpdateSource::Live,
        );
    }

    fn owned_text(app: &App, owner: usize) -> Vec<(&str, TextBlockSpacing)> {
        app.transcript.messages[owner]
            .blocks
            .iter()
            .map(|block| {
                let MessageBlock::Text(block) = block else { panic!("streamed text block") };
                (block.text.as_str(), block.trailing_spacing)
            })
            .collect()
    }

    fn assert_owned_source_uuid(app: &App, owner: usize, source_uuid: &str) {
        for block in &app.transcript.messages[owner].blocks {
            let MessageBlock::Text(block) = block else { panic!("streamed text block") };
            let actual = block.source_message_uuids.iter().map(String::as_str).collect::<Vec<_>>();
            assert_eq!(
                actual,
                [source_uuid],
                "source UUID follows the {:?} block",
                block.trailing_spacing,
            );
        }
    }

    #[test]
    fn streaming_one_chunk_releases_each_paragraph_before_extending_its_tail() {
        let mut app = App::test_default();
        stream(&mut app, "p1\n\np2\n\np3", None);
        let owner = app.active_turn_assistant_idx().expect("live owner");
        assert_eq!(
            owned_text(&app, owner),
            [
                ("p1\n\n", TextBlockSpacing::ParagraphBreak),
                ("p2\n\n", TextBlockSpacing::ParagraphBreak),
                ("p3", TextBlockSpacing::None),
            ]
        );
        stream(&mut app, " extended", None);
        assert_eq!(
            owned_text(&app, owner),
            [
                ("p1\n\n", TextBlockSpacing::ParagraphBreak),
                ("p2\n\n", TextBlockSpacing::ParagraphBreak),
                ("p3 extended", TextBlockSpacing::None),
            ]
        );
    }

    #[test]
    fn unbroken_unicode_stream_preserves_source_until_a_legal_boundary_arrives() {
        let policy = super::default_cache_split_policy();
        let source_uuid = "a79-source";
        let unicode = "\u{754c}".repeat(policy.hard_limit_bytes / "\u{754c}".len() + 1);
        assert!(
            !unicode.is_char_boundary(policy.hard_limit_bytes),
            "a forced byte cut bisects a scalar"
        );
        let ascii = "a".repeat(policy.hard_limit_bytes + 1);
        for source in [&unicode, &ascii] {
            assert!(
                source.len() > policy.soft_limit_bytes && source.len() > policy.hard_limit_bytes
            );
            for (separator, spacing) in
                [("\n", TextBlockSpacing::Continuation), ("\n\n", TextBlockSpacing::ParagraphBreak)]
            {
                let mut app = App::test_default();
                let mut received = String::new();
                let chars = source.chars().collect::<Vec<_>>();
                for part in chars.chunks(policy.soft_limit_bytes / 3) {
                    let chunk = part.iter().collect::<String>();
                    received.push_str(&chunk);
                    stream(&mut app, &chunk, Some(source_uuid));
                    let owner = app.active_turn_assistant_idx().expect("live owner");
                    let blocks = owned_text(&app, owner);
                    assert_eq!(blocks.iter().map(|(text, _)| *text).collect::<String>(), received);
                    assert!(
                        blocks.iter().all(|(_, spacing)| *spacing == TextBlockSpacing::None),
                        "no completed boundary exists in unbroken source"
                    );
                    assert_owned_source_uuid(&app, owner, source_uuid);
                }
                let owner = app.active_turn_assistant_idx().unwrap();
                let prefix = format!("{source}{separator}");
                stream(&mut app, &format!("{separator}distinct tail"), Some(source_uuid));
                assert_eq!(
                    owned_text(&app, owner),
                    [(prefix.as_str(), spacing), ("distinct tail", TextBlockSpacing::None)]
                );
                assert_owned_source_uuid(&app, owner, source_uuid);
                stream(&mut app, " extended", Some(source_uuid));
                assert_eq!(
                    owned_text(&app, owner),
                    [
                        (prefix.as_str(), spacing),
                        ("distinct tail extended", TextBlockSpacing::None)
                    ]
                );
                assert_eq!(
                    owned_text(&app, owner).iter().map(|(text, _)| *text).collect::<String>(),
                    format!("{prefix}distinct tail extended")
                );
                assert_owned_source_uuid(&app, owner, source_uuid);
            }
        }
    }
}
