// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::{ChatMessageId, NoticeDedupKey};
use crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding;
use pretty_assertions::assert_eq;

fn retry(app: &mut App, attempt: u64) {
    handle_client_event(
        app,
        session_update(model::SessionUpdate::ApiRetryUpdate {
            attempt,
            max_retries: 7,
            retry_delay_ms: 1500.0,
            error_status: Some(529),
            error: model::ApiRetryError::Overloaded,
        }),
    );
}

fn chunk(app: &mut App, text: &str) {
    handle_client_event(
        app,
        session_update(model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
            model::ContentBlock::Text(model::TextContent::new(text)),
        ))),
    );
}

fn owner(app: &App, id: ChatMessageId) -> &ChatMessage {
    app.transcript.messages.iter().find(|message| message.id == id).expect("same message owner")
}

fn notice<'a>(
    app: &'a App,
    owner_id: ChatMessageId,
    key: &NoticeDedupKey,
) -> &'a crate::app::NoticeBlock {
    let mut notices = owner(app, owner_id).blocks.iter().filter_map(|block| match block {
        MessageBlock::Notice(notice) if notice.dedup_key.as_ref() == Some(key) => Some(notice),
        _ => None,
    });
    let notice = notices.next().expect("notice on expected owner");
    assert!(notices.next().is_none(), "one notice per incident");
    assert_eq!(notice.severity, SystemSeverity::Warning);
    notice
}

fn rendered_notice(app: &mut App, owner_id: ChatMessageId, key: &NoticeDedupKey) -> String {
    let mi = app.transcript.messages.iter().position(|m| m.id == owner_id).unwrap();
    let bi = app.transcript.messages[mi]
        .blocks
        .iter()
        .position(|b| matches!(b, MessageBlock::Notice(n) if n.dedup_key.as_ref() == Some(key)))
        .unwrap();
    let boundary = if matches!(app.transcript.messages[mi].role, MessageRole::System(_)) {
        assert!(matches!(app.transcript.messages[mi].blocks.as_slice(), [MessageBlock::Notice(_)]));
        None
    } else {
        Some(bi)
    };
    let rendered = serialize_live_rows_with_boundaries_excluding(
        app,
        120,
        &std::collections::BTreeSet::new(),
        None,
    );
    let segment = rendered
        .segments()
        .iter()
        .find(|s| s.msg_idx == mi && s.block_idx == boundary)
        .expect("notice region");
    rendered
        .segment_rows(segment)
        .unwrap()
        .iter()
        .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn warning(app: &mut App, window: &str, scope: &str, reset: u64, utilization: f64) {
    let update = serde_json::from_value(serde_json::json!({"status":"allowed_warning","rate_limit_type":window,"limit_scope":scope,"resets_at":reset,"utilization":utilization})).unwrap();
    handle_client_event(app, session_update(model::SessionUpdate::RateLimitUpdate(update)));
}

fn rate_key(window: &str, scope: &str, reset: u64) -> NoticeDedupKey {
    NoticeDedupKey::RateLimit(crate::app::RateLimitIncidentKey {
        rate_limit_type: Some(window.into()),
        limit_scope: Some(scope.into()),
        resets_at_bucket: Some(reset),
    })
}

#[test]
fn retry_migrates_to_the_stream_owner_and_remains_updatable() {
    let mut app = make_test_app();
    retry(&mut app, 1);
    let standalone = app.transcript.messages.last().unwrap().id;
    assert!(rendered_notice(&mut app, standalone, &NoticeDedupKey::ApiRetry).contains("1/7"));
    handle_client_event(
        &mut app,
        session_update(model::SessionUpdate::SystemNoticeUpdate {
            severity: model::SystemNoticeSeverity::Info,
            message: "unrelated notice sentinel".into(),
        }),
    );
    let neighbor = app.transcript.messages.last().unwrap().id;
    handle_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
    chunk(&mut app, "Active response source");
    let active = app.transcript.messages[app.active_turn_assistant_idx().unwrap()].id;
    for attempt in [2, 3] {
        retry(&mut app, attempt);
        assert!(!app.transcript.messages.iter().any(|m| m.id == standalone));
        assert_eq!(app.transcript.messages[app.active_turn_assistant_idx().unwrap()].id, active);
        assert!(
            notice(&app, active, &NoticeDedupKey::ApiRetry)
                .text
                .text
                .contains(&format!("{attempt}/7"))
        );
        let rendered = rendered_notice(&mut app, active, &NoticeDedupKey::ApiRetry);
        assert!(rendered.contains(&format!("{attempt}/7")));
        assert!(!rendered.contains(&format!("{}/7", attempt - 1)));
        assert_eq!(first_block_text(owner(&app, neighbor)), "unrelated notice sentinel");
    }
    retry(&mut app, 4);
    chunk(&mut app, " continues");
    assert!(notice(&app, active, &NoticeDedupKey::ApiRetry).text.text.contains("4/7"));
    let source: String = owner(&app, active)
        .blocks
        .iter()
        .filter_map(|b| if let MessageBlock::Text(t) = b { Some(t.text.as_str()) } else { None })
        .collect();
    assert_eq!(source, "Active response source continues");
}

#[test]
fn rate_limit_incidents_remain_distinct_within_one_assistant_turn() {
    let mut app = make_test_app();
    handle_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
    chunk(&mut app, "source survives incident updates");
    let active = app.transcript.messages[app.active_turn_assistant_idx().unwrap()].id;
    let incidents = [
        ("five_hour", "personal", 1_700_000_100),
        ("seven_day", "personal", 1_700_000_100),
        ("five_hour", "group_pool", 1_700_000_100),
        ("five_hour", "personal", 1_700_000_900),
    ];
    for (window, scope, reset) in incidents {
        warning(&mut app, window, scope, reset, 0.91);
    }
    let before: Vec<_> = incidents
        .iter()
        .map(|(w, s, r)| {
            let n = notice(&app, active, &rate_key(w, s, *r));
            (n.id, n.text.text.clone())
        })
        .collect();
    for _ in 0..2 {
        warning(&mut app, "five_hour", "personal", 1_700_000_100, 0.97);
    }
    for (index, (window, scope, reset)) in incidents.iter().enumerate() {
        let n = notice(&app, active, &rate_key(window, scope, *reset));
        if index == 0 {
            assert!(n.text.text.contains("97%"));
            assert!(!n.text.text.contains("91%"));
        } else {
            assert_eq!(n.id, before[index].0);
            assert_eq!(n.text.text, before[index].1);
        }
    }
    assert_eq!(app.transcript.messages[app.active_turn_assistant_idx().unwrap()].id, active);
    assert_eq!(first_block_text(owner(&app, active)), "source survives incident updates");
}

#[test]
fn retry_updates_the_retained_owner_after_real_history_pruning() {
    let mut app = make_test_app();
    app.push_message_tracked(user_msg(&"old user sentinel ".repeat(1000)));
    let old_user = app.transcript.messages.last().unwrap().id;
    app.push_message_tracked(assistant_msg(vec![MessageBlock::Text(TextBlock::from_complete(
        &"old assistant sentinel ".repeat(1000),
    ))]));
    let old_assistant = app.transcript.messages.last().unwrap().id;
    handle_client_event(&mut app, session_update(model::SessionUpdate::AgentResponseStarted));
    chunk(&mut app, "retained active source");
    let active = app.transcript.messages[app.active_turn_assistant_idx().unwrap()].id;
    retry(&mut app, 1);
    warning(&mut app, "five_hour", "personal", 1_700_000_100, 0.92);
    let allowance_key = rate_key("five_hour", "personal", 1_700_000_100);
    let allowance = (
        notice(&app, active, &allowance_key).id,
        notice(&app, active, &allowance_key).text.text.clone(),
    );
    assert!(rendered_notice(&mut app, active, &NoticeDedupKey::ApiRetry).contains("1/7"));
    app.history_retention.max_bytes = 1;
    app.enforce_history_retention_tracked();
    assert!(!app.transcript.messages.iter().any(|m| m.id == old_user || m.id == old_assistant));
    assert!(
        app.transcript.messages.iter().any(
            |m| first_block_text(m).starts_with("Older messages hidden to keep memory bounded")
        )
    );
    assert_eq!(app.transcript.messages[app.active_turn_assistant_idx().unwrap()].id, active);
    retry(&mut app, 2);
    assert!(notice(&app, active, &NoticeDedupKey::ApiRetry).text.text.contains("2/7"));
    assert_eq!(
        (
            notice(&app, active, &allowance_key).id,
            notice(&app, active, &allowance_key).text.text.clone()
        ),
        allowance
    );
    assert_eq!(first_block_text(owner(&app, active)), "retained active source");
    let rendered = rendered_notice(&mut app, active, &NoticeDedupKey::ApiRetry);
    assert!(rendered.contains("2/7"));
    assert!(!rendered.contains("1/7"));
}
