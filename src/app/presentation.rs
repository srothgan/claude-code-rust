// SPDX-License-Identifier: Apache-2.0

//! Transcript clocks: SDK wall time, observed wall time, and elapsed time have distinct provenance.

use crate::app::{App, ChatMessage, MessageBlock, MessageRole};
use chrono::{DateTime, Local, Utc};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageTimestamp {
    pub time: DateTime<Utc>,
    pub observed: bool,
}

impl MessageTimestamp {
    pub(crate) fn observed() -> Self {
        Self { time: Utc::now(), observed: true }
    }

    pub(crate) fn apply_sdk(slot: &mut Option<Self>, timestamp: &str) {
        if slot.is_some_and(|value| !value.observed) {
            return;
        }
        if let Ok(time) = DateTime::parse_from_rfc3339(timestamp) {
            *slot = Some(Self { time: time.with_timezone(&Utc), observed: false });
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MessageTiming {
    pub timestamp: Option<MessageTimestamp>,
    pub duration: Option<TurnDuration>,
    started: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnDuration {
    pub elapsed: Duration,
    pub api: Option<Duration>,
    pub observed: bool,
    pub completed_at: DateTime<Utc>,
}

impl MessageTiming {
    pub fn observed(role: &MessageRole) -> Self {
        if !matches!(role, MessageRole::User | MessageRole::Assistant) {
            return Self::default();
        }
        Self {
            timestamp: Some(MessageTimestamp::observed()),
            started: matches!(role, MessageRole::Assistant).then(Instant::now),
            ..Self::default()
        }
    }

    pub fn set_sdk_timestamp(&mut self, timestamp: &str) {
        MessageTimestamp::apply_sdk(&mut self.timestamp, timestamp);
    }

    pub fn set_sdk_duration(&mut self, elapsed_ms: f64, api_ms: Option<f64>) {
        let Some(elapsed) = milliseconds(elapsed_ms) else { return };
        self.duration = Some(TurnDuration {
            elapsed,
            api: api_ms.and_then(milliseconds),
            observed: false,
            completed_at: Utc::now(),
        });
        self.started = None;
    }

    pub fn is_finished(&self) -> bool {
        self.started.is_none()
    }

    pub fn finish(&mut self) {
        if let Some(started) = self.started.take() {
            self.duration = Some(TurnDuration {
                elapsed: started.elapsed(),
                api: None,
                observed: true,
                completed_at: Utc::now(),
            });
        }
    }

    pub fn discard_replay_observations(&mut self) {
        self.started = None;
        if self.timestamp.is_some_and(|time| time.observed) {
            self.timestamp = None;
        }
    }
}

fn milliseconds(value: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(value / 1000.0).ok()
}

pub(crate) fn apply_message_metadata(
    app: &mut App,
    role: &str,
    uuid: Option<&str>,
    timestamp: &str,
) {
    let target = app
        .transcript
        .messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            let matches_role = matches!(
                (&message.role, role),
                (MessageRole::User, "user") | (MessageRole::Assistant, "assistant")
            );
            let matches_uuid = uuid.is_none_or(|uuid| {
                message.blocks.iter().any(|block| match block {
                    MessageBlock::Text(text) => {
                        text.source_message_uuids.iter().any(|id| id == uuid)
                    }
                    MessageBlock::ToolCall(tool) => tool.has_source_message_uuid(uuid),
                    _ => false,
                })
            });
            (matches_role && matches_uuid).then_some(index)
        })
        .or_else(|| (role == "assistant").then(|| app.active_turn_assistant_idx()).flatten());
    if let Some(index) = target {
        app.transcript.messages[index].timing.set_sdk_timestamp(timestamp);
        app.invalidate_layout(crate::app::InvalidationLevel::MessageChanged(index));
    } else if role == "user"
        && let Some(uuid) = uuid
    {
        app.pending_user_messages.set_sdk_timestamp(uuid, timestamp);
    }
}

pub(crate) fn timestamp_label(app: &App, message: &ChatMessage) -> Option<String> {
    if !app.config.show_message_timestamps_effective() {
        return None;
    }
    message.timing.timestamp.map(|timestamp| {
        format!(
            "{}{}",
            clock(app, timestamp.time),
            if timestamp.observed { " (observed)" } else { "" }
        )
    })
}

pub(crate) fn duration_label(app: &App, message: &ChatMessage) -> Option<String> {
    if !app.config.show_turn_duration_effective() {
        return None;
    }
    message.timing.duration.map(|duration| {
        let mut label = format!(
            "{} {}",
            if duration.observed { "Observed" } else { "Elapsed" },
            elapsed(duration.elapsed)
        );
        if app.config.show_message_timestamps_effective() {
            let _ = write!(label, " · Done {}", clock(app, duration.completed_at));
        }
        label
    })
}

pub(crate) fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!("{}h {}m {}s", seconds / 3600, seconds % 3600 / 60, seconds % 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

pub(crate) fn clock(app: &App, timestamp: DateTime<Utc>) -> String {
    let format = app.config.time_format();
    let locale = sys_locale::get_locale()
        .and_then(|locale| locale.replace('-', "_").parse::<chrono::Locale>().ok())
        .unwrap_or(chrono::Locale::POSIX);
    // Explicit 12-hour clocks require an AM/PM marker even in locales whose
    // ordinary clocks use 24-hour time and leave %p empty.
    let locale = if format == "12-hour" { chrono::Locale::POSIX } else { locale };
    let pattern = match format {
        "12-hour" => "%-I:%M %p",
        "24-hour" => "%H:%M",
        "24-hour-utc" => "%H:%MZ",
        custom
            if custom.contains('%')
                && !chrono::format::StrftimeItems::new(custom)
                    .any(|item| matches!(item, chrono::format::Item::Error)) =>
        {
            custom
        }
        _ => "%X",
    };
    let zone = app.config.time_zone().and_then(|zone| zone.parse::<chrono_tz::Tz>().ok());
    let formatted = if format == "24-hour-utc" {
        timestamp.format_localized(pattern, locale).to_string()
    } else if let Some(zone) = zone {
        timestamp.with_timezone(&zone).format_localized(pattern, locale).to_string()
    } else {
        timestamp.with_timezone(&Local).format_localized(pattern, locale).to_string()
    };
    // Custom patterns must not inject terminal controls or change label row structure.
    formatted.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{events::ClientEvent, model};
    use crate::app::{AppStatus, TextBlock};
    use serde_json::json;

    fn update(app: &mut App, update: model::SessionUpdate) {
        app.session_runtime.activate_session("test-session".into());
        crate::app::events::handle_client_event(
            app,
            ClientEvent::SessionUpdate { session_id: "test-session".into(), update },
        );
    }

    fn show_times(app: &mut App) {
        let mut snapshot = crate::agent::settings::SettingsSnapshot::test_value(
            "showMessageTimestamps",
            json!(true),
        );
        snapshot.values.extend(
            crate::agent::settings::SettingsSnapshot::test_value(
                "timeFormat",
                json!("24-hour-utc"),
            )
            .values,
        );
        snapshot.values.extend(
            crate::agent::settings::SettingsSnapshot::test_value("showTurnDuration", json!(true))
                .values,
        );
        app.config.snapshot = Some(snapshot);
    }

    #[test]
    fn live_sdk_clocks_render_once_and_survive_handoff_of_the_response_body() {
        let mut app = App::test_default();
        update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(
                model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(
                    "The answer",
                )))
                .source_message_uuid(Some("answer-1".into())),
            ),
        );
        update(
            &mut app,
            model::SessionUpdate::MessageMetadata {
                role: "assistant".into(),
                timestamp: "2026-10-03T15:02:01Z".into(),
                source_message_uuid: Some("answer-1".into()),
            },
        );
        update(
            &mut app,
            model::SessionUpdate::TurnTiming { duration_ms: 2000.0, api_duration_ms: Some(1250.0) },
        );
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: "test-session".into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
        let message = &app.transcript.messages[0];
        assert!(!message.timing.timestamp.expect("timestamp").observed);
        assert_eq!(message.timing.duration.expect("duration").elapsed, Duration::from_secs(2));
        assert_eq!(
            message.timing.duration.expect("duration").api,
            Some(Duration::from_millis(1250))
        );
        let message_id = message.id;
        let text_id = match &message.blocks[0] {
            MessageBlock::Text(text) => text.id,
            _ => panic!("text"),
        };
        let hidden = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            &mut app,
            100,
            &std::collections::BTreeSet::default(),
        );
        let hidden_text: String = hidden
            .rows()
            .iter()
            .flat_map(|row| &row.spans)
            .map(|span| span.content.as_ref())
            .collect();
        assert!(!hidden_text.contains("Elapsed 2.0s"));
        assert!(!hidden_text.contains("15:02"));
        show_times(&mut app);
        let excluded = std::collections::BTreeSet::from([
            crate::app::HistoryOutputId::AssistantLabel(message_id),
            crate::app::HistoryOutputId::Block(text_id),
        ]);
        let rows = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            &mut app, 100, &excluded,
        );
        let text: String = rows
            .rows()
            .iter()
            .flat_map(|row| &row.spans)
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("Elapsed 2.0s"));
        assert!(text.contains("Done "));
        assert_eq!(
            rows.segments()
                .iter()
                .filter(|segment| segment
                    .ids
                    .contains(&crate::app::HistoryOutputId::AssistantDuration(message_id)))
                .count(),
            1
        );
        let all = crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            &mut app,
            100,
            &std::collections::BTreeSet::default(),
        );
        assert!(
            all.rows()
                .iter()
                .any(|row| row.spans.iter().any(|span| span.content.contains("15:02Z")))
        );
    }

    #[test]
    fn durations_can_be_enabled_without_enabling_completion_timestamps() {
        let mut app = App::test_default();
        update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("Answer")),
            )),
        );
        update(
            &mut app,
            model::SessionUpdate::TurnTiming { duration_ms: 2700.0, api_duration_ms: Some(1800.0) },
        );
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "showTurnDuration",
            json!(true),
        ));
        assert_eq!(
            duration_label(&app, &app.transcript.messages[0]).as_deref(),
            Some("Elapsed 2.7s")
        );
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "showTurnDuration",
            json!(false),
        ));
        assert!(duration_label(&app, &app.transcript.messages[0]).is_none());
    }

    #[test]
    fn queued_native_timestamp_moves_to_the_started_message_and_failure_finishes_observed_time() {
        let mut app = App::test_default();
        app.pending_user_messages
            .try_push_sending(crate::app::PendingUserMessage::sending(
                "queued-clock".into(),
                "queued question".into(),
                Vec::new(),
            ))
            .expect("queue");
        update(
            &mut app,
            model::SessionUpdate::MessageMetadata {
                role: "user".into(),
                timestamp: "2025-01-01T10:00:00Z".into(),
                source_message_uuid: Some("queued-clock".into()),
            },
        );
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::UserMessageStarted {
                session_id: "test-session".into(),
                message_uuid: "queued-clock".into(),
                source: crate::agent::types::UserMessageStartSource::Assistant,
            },
        );
        assert_eq!(
            app.transcript.messages[0].timing.timestamp.expect("native clock").time.to_rfc3339(),
            "2025-01-01T10:00:00+00:00"
        );
        assert!(!app.transcript.messages[0].timing.timestamp.expect("native").observed);
        update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("Partial response")),
            )),
        );
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnError {
                session_id: "test-session".into(),
                message: "service failed".into(),
                queued_turn_count: None,
                api_error_status: None,
                terminal_reason: None,
            },
        );
        assert_eq!(app.status, AppStatus::Error);
        assert!(app.transcript.messages[1].timing.duration.expect("observed duration").observed);
    }

    #[test]
    fn clock_presets_custom_patterns_and_timezone_share_one_formatter() {
        let mut app = App::test_default();
        let time = DateTime::parse_from_rfc3339("2026-10-03T15:02:01Z")
            .expect("clock")
            .with_timezone(&Utc);
        for (format, expected) in [
            ("12-hour", "5:02 PM"),
            ("24-hour", "17:02"),
            ("24-hour-utc", "15:02Z"),
            ("%Y-%m-%d %H:%M", "2026-10-03 17:02"),
            ("%H%n%M", "17 02"),
        ] {
            let mut snapshot =
                crate::agent::settings::SettingsSnapshot::test_value("timeFormat", json!(format));
            snapshot.time_zone = Some("Europe/Berlin".into());
            app.config.snapshot = Some(snapshot);
            assert_eq!(clock(&app, time), expected);
        }
        let mut message = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("historic"))],
            None,
        );
        message.timing.discard_replay_observations();
        assert!(message.timing.timestamp.is_none());
        assert!(message.timing.duration.is_none());
        assert!(message.timing.is_finished());
    }
}
