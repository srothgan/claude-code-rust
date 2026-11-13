// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::test_support::{output_region, welcome_art, welcome_lifecycle_app};

fn rows_at(app: &mut App, width: u16, now: Instant) -> SerializedLiveRows {
    serialize_live_rows_at(app, width, &BTreeSet::new(), now, &LayoutControl::uninterrupted())
}

fn assert_metadata(region: &[String]) {
    for (label, value) in [
        ("Version:", "8.6.4"),
        ("Subscription:", "Orchid"),
        ("Cwd:", "/garden"),
        ("Session ID:", "sid-47"),
    ] {
        let matching: Vec<_> = region.iter().filter(|row| row.contains(label)).collect();
        assert_eq!(matching.len(), 1, "one associated welcome field: {region:?}");
        assert!(matching[0].contains(value), "{label} must show {value}: {region:?}");
    }
}

fn metadata_positions(region: &[String]) -> Vec<(usize, usize)> {
    ["8.6.4", "Orchid", "/garden", "sid-47"]
        .iter()
        .map(|value| {
            let (index, row) = region
                .iter()
                .enumerate()
                .find(|(_, row)| row.contains(value))
                .expect("metadata value");
            let offset = row.find(value).unwrap();
            (index, unicode_width::UnicodeWidthStr::width(&row[..offset]))
        })
        .collect()
}

#[test]
fn lifecycle_welcome_values_release_ordered_history_across_widths() {
    let now = Instant::now();
    let (mut app, id) = welcome_lifecycle_app(now, false, Some(true));
    let notice = ChatMessage::new(
        MessageRole::System(Some(SystemSeverity::Info)),
        vec![MessageBlock::Notice(NoticeBlock::from_complete(
            SystemSeverity::Info,
            "NOTICE_AFTER_WELCOME",
        ))],
        None,
    );
    let notice_id = HistoryOutputId::Message(notice.id);
    app.push_message_tracked(notice);
    for width in [36, 120, 36] {
        let rows = rows_at(&mut app, width, now);
        let (welcome, text) = output_region(&rows, &id);
        assert!(!welcome.commit_ready);
        assert_eq!(rows.stable_row_count(), 0, "the notice cannot overtake loading metadata");
        for label in ["Subscription:", "Session ID:"] {
            let row = text.iter().find(|row| row.contains(label)).expect("unresolved field");
            assert!(!row.split_once(label).unwrap().1.trim().is_empty());
        }
        assert!(!text.iter().any(|row| row.contains("Orchid") || row.contains("sid-47")));
        let (later, notice) = output_region(&rows, &notice_id);
        assert!(later.commit_ready);
        assert!(later.start_row >= welcome.end_row);
        assert!(notice.iter().any(|row| row.contains("NOTICE_AFTER_WELCOME")));
    }
    app.status = AppStatus::Ready;
    let MessageBlock::Welcome(welcome) = &mut app.transcript.messages[0].blocks[0] else {
        panic!("welcome");
    };
    welcome.subscription = "Orchid".into();
    welcome.session_id = "sid-47".into();
    for width in [36, 120, 36] {
        let rows = rows_at(&mut app, width, now);
        let (welcome, text) = output_region(&rows, &id);
        assert_metadata(&text);
        assert_eq!(rows.segments().iter().filter(|segment| segment.ids.contains(&id)).count(), 1);
        assert!(welcome.commit_ready);
        assert_eq!(rows.stable_row_count(), rows.rows().len());
        let (notice, _) = output_region(&rows, &notice_id);
        assert!(notice.start_row >= welcome.end_row);
        let excluded = serialize_live_rows_at(
            &mut app,
            width,
            &BTreeSet::from([id.clone()]),
            now,
            &LayoutControl::uninterrupted(),
        );
        assert!(!excluded.segments().iter().any(|segment| segment.ids.contains(&id)));
        let text: Vec<_> = excluded.rows().iter().map(ToString::to_string).collect();
        assert!(!text.iter().any(|row| row.contains("8.6.4") || row.contains("sid-47")));
        assert!(
            output_region(&excluded, &notice_id)
                .1
                .iter()
                .any(|row| row.contains("NOTICE_AFTER_WELCOME"))
        );
    }
}

#[test]
fn lifecycle_welcome_motion_settles_without_shifting_metadata() {
    let now = Instant::now();
    let (mut connecting, id) = welcome_lifecycle_app(now, false, Some(false));
    let mut art = Vec::new();
    for frame in [0, 9, 18] {
        connecting.spinner_frame = frame;
        let rows = rows_at(&mut connecting, 120, now);
        let (segment, text) = output_region(&rows, &id);
        assert!(!segment.commit_ready);
        assert!(text.iter().any(|row| row.contains("Version:") && row.contains("8.6.4")));
        art.push(welcome_art(&text));
    }
    assert!(art.iter().all(|frame| !frame.is_empty()));
    assert!(art.windows(2).any(|frames| frames[0] != frames[1]));
    let (mut ready, id) = welcome_lifecycle_app(now, true, Some(false));
    let mut positions = None;
    let mut moving = Vec::new();
    for millis in [100, 400, 850] {
        let rows = rows_at(&mut ready, 120, now + std::time::Duration::from_millis(millis));
        let (segment, text) = output_region(&rows, &id);
        assert!(!segment.commit_ready);
        assert_metadata(&text);
        let current = metadata_positions(&text);
        if let Some(expected) = &positions {
            assert_eq!(&current, expected);
        } else {
            positions = Some(current);
        }
        moving.push(welcome_art(&text));
    }
    assert!(moving.windows(2).any(|frames| frames[0] != frames[1]));
    let mut final_region = None;
    for seconds in [2, 60] {
        let rows = rows_at(&mut ready, 120, now + std::time::Duration::from_secs(seconds));
        let (segment, text) = output_region(&rows, &id);
        assert!(segment.commit_ready);
        assert!(rows.stable_row_count() >= segment.end_row);
        assert_metadata(&text);
        assert_eq!(Some(metadata_positions(&text)), positions);
        if let Some(expected) = &final_region {
            assert_eq!(&text, expected);
        } else {
            final_region = Some(text);
        }
    }
}

#[test]
fn lifecycle_welcome_preferences_release_only_finalized_values() {
    let now = Instant::now();
    for ready in [false, true] {
        let (mut app, id) = welcome_lifecycle_app(now, ready, Some(true));
        let mut art = None;
        for (millis, frame) in [(0, 0), (400, 9), (900, 18), (2_000, 27)] {
            app.spinner_frame = frame;
            let rows = rows_at(&mut app, 120, now + std::time::Duration::from_millis(millis));
            let (segment, text) = output_region(&rows, &id);
            assert_eq!(segment.commit_ready, ready);
            let current = welcome_art(&text);
            assert!(!current.is_empty());
            if let Some(expected) = &art {
                assert_eq!(&current, expected);
            } else {
                art = Some(current);
            }
            if ready {
                assert_metadata(&text);
            }
        }
    }
    for arrival in [None, Some(false), Some(true)] {
        let (mut app, id) = welcome_lifecycle_app(now, true, None);
        let before = rows_at(&mut app, 120, now + std::time::Duration::from_millis(100));
        let (segment, text) = output_region(&before, &id);
        assert!(!segment.commit_ready);
        assert_metadata(&text);
        let resting = welcome_art(&text);
        app.config.snapshot = arrival.map(|value| {
            crate::agent::settings::SettingsSnapshot::test_value(
                "prefersReducedMotion",
                serde_json::json!(value),
            )
        });
        let after = rows_at(&mut app, 120, now + std::time::Duration::from_millis(700));
        let (segment, text) = output_region(&after, &id);
        assert_eq!(segment.commit_ready, arrival == Some(true));
        assert_metadata(&text);
        if arrival == Some(false) {
            assert_ne!(welcome_art(&text), resting);
        } else {
            assert_eq!(welcome_art(&text), resting);
        }
        let expired = rows_at(&mut app, 120, now + std::time::Duration::from_secs(2));
        let (segment, text) = output_region(&expired, &id);
        assert!(segment.commit_ready, "unknown preferences cannot hold history indefinitely");
        assert_eq!(welcome_art(&text), resting);
        assert_metadata(&text);
    }
}

#[test]
fn lifecycle_narrow_layout_and_content_arrival_release_the_welcome_region() {
    let now = Instant::now();
    let (mut settled, settled_id) = welcome_lifecycle_app(now, true, Some(false));
    let rows = rows_at(&mut settled, 120, now + std::time::Duration::from_secs(2));
    let resting = welcome_art(&output_region(&rows, &settled_id).1);
    for width in [30, 36] {
        let (mut app, id) = welcome_lifecycle_app(now, true, Some(false));
        let rows = rows_at(&mut app, width, now + std::time::Duration::from_millis(100));
        let (segment, text) = output_region(&rows, &id);
        assert!(segment.commit_ready);
        assert!(welcome_art(&text).is_empty());
        assert_metadata(&text);
    }
    let (mut app, id) = welcome_lifecycle_app(now, true, Some(false));
    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete("USER_INTERRUPTION"))],
        None,
    ));
    let mut assistant = ChatMessage::new(MessageRole::Assistant, Vec::new(), None);
    let block = TextBlock::from_complete("UNFINISHED_NEIGHBOR");
    let pending_id = HistoryOutputId::Block(block.id);
    assistant.blocks.push(MessageBlock::Text(block));
    app.push_message_tracked(assistant);
    app.bind_active_turn_assistant(app.transcript.messages.len() - 1);
    app.status = AppStatus::Running;
    let rows = rows_at(&mut app, 120, now + std::time::Duration::from_millis(100));
    let (segment, text) = output_region(&rows, &id);
    assert!(segment.commit_ready);
    assert_eq!(welcome_art(&text), resting);
    assert_metadata(&text);
    let (neighbor, text) = output_region(&rows, &pending_id);
    assert!(!neighbor.commit_ready, "the welcome settles independently of live neighbors");
    assert!(text.iter().any(|row| row.contains("UNFINISHED_NEIGHBOR")));
}
