// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::{App, ChatMessage, MessageBlock, MessageRole, TextBlock};

#[test]
fn tracked_enforcement_reports_pruning_and_later_pressure_on_its_actual_cadence() {
    let mut app = App::test_default();
    app.history_retention = HistoryRetentionPolicy { max_bytes: 2048 };
    for label in ["OLD_ALPHA", "OLD_BETA"] {
        app.push_message_tracked(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete(&format!(
                "{label}{}",
                "x".repeat(64 * 1024)
            )))],
            None,
        ));
    }
    let mut protected = crate::app::test_support::tool_call_info(
        "PROTECTED_TOOL",
        crate::agent::model::ToolCallStatus::InProgress,
    );
    protected.title = "p".repeat(128 * 1024);
    app.push_message_tracked(ChatMessage::new(
        MessageRole::Assistant,
        vec![MessageBlock::ToolCall(Box::new(protected))],
        None,
    ));
    let capture = |app: &mut App| {
        crate::logging::test_capture::capture(|| app.enforce_history_retention_tracked())
            .into_iter()
            .filter(|entry| entry["event_name"] == "history_retention_metrics")
            .collect::<Vec<_>>()
    };
    let first = capture(&mut app);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["history_dropped_pass"], 2);
    assert_eq!(first[0]["history_dropped_total"], 2);
    assert!(first[0]["history_bytes"].as_u64().unwrap() > 128 * 1024);
    assert!(
        first[0]["history_peak"].as_u64().unwrap()
            > first[0]["history_bytes"].as_u64().unwrap() + 100 * 1024
    );
    assert!(first[0]["history_util_pct"].as_str().unwrap().parse::<f64>().unwrap() > 100.0);
    assert!(
        !app.transcript
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| matches!(block,MessageBlock::Text(text) if text.text.starts_with("OLD_")))
    );
    assert!(
        app.transcript
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| matches!(block,MessageBlock::ToolCall(tool) if tool.id=="PROTECTED_TOOL"))
    );
    for _ in 1..HISTORY_LOG_INTERVAL {
        assert!(capture(&mut app).is_empty());
    }
    let next = capture(&mut app);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0]["history_dropped_pass"], 0);
    assert_eq!(next[0]["history_dropped_total"], 2);
    assert_eq!(next[0]["history_peak"], first[0]["history_peak"]);
    assert_eq!(next[0]["history_dropped_bytes_total"], first[0]["history_dropped_bytes_total"]);
    assert!(capture(&mut app).is_empty());
}
