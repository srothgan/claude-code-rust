// SPDX-License-Identifier: Apache-2.0

//! Transcript clocks: SDK wall time, observed wall time, and elapsed time have distinct provenance.

use crate::app::{App, ChatMessage, MessageBlock, MessageRole};
use chrono::{DateTime, Local, Utc};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

#[cfg(test)]
thread_local! {
    static OBSERVATION_TIME: std::cell::Cell<Option<(Instant, DateTime<Utc>)>> = const { std::cell::Cell::new(None) };
}

fn observation_time() -> (Instant, DateTime<Utc>) {
    #[cfg(test)]
    if let Some(time) = OBSERVATION_TIME.with(std::cell::Cell::get) {
        return time;
    }
    (Instant::now(), Utc::now())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageTimestamp {
    pub time: DateTime<Utc>,
    pub observed: bool,
}

impl MessageTimestamp {
    pub(crate) fn observed() -> Self {
        Self { time: observation_time().1, observed: true }
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
            started: matches!(role, MessageRole::Assistant).then(|| observation_time().0),
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
            completed_at: observation_time().1,
        });
        self.started = None;
    }

    pub fn is_finished(&self) -> bool {
        self.started.is_none()
    }

    pub fn finish(&mut self) {
        if let Some(started) = self.started.take() {
            let (now, completed_at) = observation_time();
            self.duration = Some(TurnDuration {
                elapsed: now.saturating_duration_since(started),
                api: None,
                observed: true,
                completed_at,
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

#[derive(Debug, Clone, Copy, Default)]
pub struct ClockFormat<'a> {
    pub format: &'a str,
    pub zone: Option<&'a str>,
}

pub(crate) fn clock(app: &App, timestamp: DateTime<Utc>) -> String {
    ClockFormat { format: app.config.time_format(), zone: app.config.time_zone() }
        .format(timestamp, false)
}

impl ClockFormat<'_> {
    pub(crate) fn timestamp(self, value: &str) -> Option<String> {
        DateTime::parse_from_rfc3339(value).ok().map(|time| self.format(time.to_utc(), true))
    }

    fn format(self, timestamp: DateTime<Utc>, with_date: bool) -> String {
        let format = self.format;
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
        // Presets add a calendar date for tool results; explicit custom patterns stay verbatim.
        let pattern = if with_date && pattern != format {
            format!("%Y-%m-%d {pattern}")
        } else {
            pattern.to_owned()
        };
        let zone = self.zone.and_then(|zone| zone.parse::<chrono_tz::Tz>().ok());
        let formatted = if format == "24-hour-utc" {
            timestamp.format_localized(&pattern, locale).to_string()
        } else if let Some(zone) = zone {
            timestamp.with_timezone(&zone).format_localized(&pattern, locale).to_string()
        } else {
            timestamp.with_timezone(&Local).format_localized(&pattern, locale).to_string()
        };
        // Custom patterns must not inject terminal controls or change label row structure.
        formatted.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect()
    }
}

pub(crate) fn invalidate_tool_clocks(app: &mut App) {
    for message in &mut app.transcript.messages {
        for block in &mut message.blocks {
            if let MessageBlock::ToolCall(tool) = block
                && tool
                    .output_metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.presentation.is_some())
            {
                tool.cache.invalidate();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{events::ClientEvent, model};
    use serde_json::json;

    struct TestClock {
        previous: Option<(Instant, DateTime<Utc>)>,
        start: Instant,
        wall: DateTime<Utc>,
    }

    impl TestClock {
        fn new() -> Self {
            let start = Instant::now();
            let wall = DateTime::parse_from_rfc3339("2026-10-03T15:05:00Z").unwrap().to_utc();
            let previous = OBSERVATION_TIME.with(|clock| clock.replace(Some((start, wall))));
            Self { previous, start, wall }
        }
        fn advance(&self, elapsed: Duration) {
            OBSERVATION_TIME.with(|clock| {
                clock.set(Some((
                    self.start + elapsed,
                    self.wall + chrono::Duration::from_std(elapsed).unwrap(),
                )));
            });
        }
    }

    impl Drop for TestClock {
        fn drop(&mut self) {
            OBSERVATION_TIME.with(|clock| clock.set(self.previous));
        }
    }

    fn preferences(app: &mut App, timestamps: bool, durations: bool, format: &str) {
        let mut snapshot = crate::agent::settings::SettingsSnapshot::test_value(
            "showMessageTimestamps",
            json!(timestamps),
        );
        snapshot.values.extend(
            crate::agent::settings::SettingsSnapshot::test_value(
                "showTurnDuration",
                json!(durations),
            )
            .values,
        );
        snapshot.values.extend(
            crate::agent::settings::SettingsSnapshot::test_value("timeFormat", json!(format))
                .values,
        );
        snapshot.time_zone = Some("UTC".into());
        app.config.snapshot = Some(snapshot);
    }

    fn send(app: &mut App, rx: &mut crate::agent::client::CommandReceiver, source: &str) -> String {
        use crate::agent::wire::BridgeCommand;
        app.input.set_text(source);
        crate::app::input_submit::submit_input(app);
        let mut uuid = None;
        while let Ok(envelope) = rx.try_recv() {
            match envelope.command {
                BridgeCommand::Prompt { session_id, message_uuid, chunks, inline_pastes } => {
                    assert!(uuid.is_none());
                    assert_eq!(session_id, "session-1");
                    assert!(inline_pastes.is_empty());
                    assert_eq!(chunks.len(), 1);
                    assert_eq!(chunks[0].kind, "text");
                    assert_eq!(chunks[0].value, json!(source));
                    uuid = Some(message_uuid);
                }
                BridgeCommand::GetContextUsage { session_id } => {
                    assert_eq!(session_id, "session-1");
                }
                other => panic!("unexpected command {other:?}"),
            }
        }
        uuid.expect("prompt")
    }

    fn chunk(source: &str, uuid: &str, user: bool) -> model::SessionUpdate {
        let chunk =
            model::ContentChunk::new(model::ContentBlock::Text(model::TextContent::new(source)))
                .source_message_uuid(Some(uuid.into()));
        if user {
            model::SessionUpdate::UserMessageChunk(chunk)
        } else {
            model::SessionUpdate::AgentMessageChunk(chunk)
        }
    }

    fn metadata(role: &str, uuid: &str, timestamp: &str) -> model::SessionUpdate {
        model::SessionUpdate::MessageMetadata {
            role: role.into(),
            timestamp: timestamp.into(),
            source_message_uuid: Some(uuid.into()),
        }
    }

    fn event_update(app: &mut App, event: model::SessionUpdate) {
        crate::app::test_support::deliver_update(app, event);
    }

    fn finish(app: &mut App) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::TurnComplete {
                session_id: "session-1".into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
    }

    fn render(
        app: &mut App,
        excluded: &std::collections::BTreeSet<crate::app::HistoryOutputId>,
    ) -> crate::ui::inline_chat_rows::SerializedLiveRows {
        crate::ui::inline_chat_rows::serialize_live_rows_with_boundaries_excluding(
            app, 100, excluded, None,
        )
    }

    fn region(
        rows: &crate::ui::inline_chat_rows::SerializedLiveRows,
        owner: usize,
        kind: Option<crate::ui::inline_chat_rows::LiveRowBoundaryKind>,
    ) -> String {
        rows.segments()
            .iter()
            .filter(|s| s.msg_idx == owner && kind.is_none_or(|kind| s.kind == kind))
            .flat_map(|s| rows.segment_rows(s).unwrap())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn source_owner(app: &App, uuid: &str) -> usize {
        app.transcript
            .messages
            .iter()
            .position(|m| {
                m.blocks
                    .iter()
                    .any(|b| matches!(b,MessageBlock::Text(t) if t.has_source_message_uuid(uuid)))
            })
            .unwrap()
    }

    #[test]
    fn live_sdk_clocks_render_once_and_survive_handoff_of_the_response_body() {
        use crate::app::HistoryOutputId;
        use crate::ui::inline_chat_rows::LiveRowBoundaryKind;
        use std::collections::BTreeSet;
        for (timestamps, durations) in [(false, false), (true, false), (false, true), (true, true)]
        {
            let clock = TestClock::new();
            let (mut app, mut rx) = crate::app::test_support::app_with_connection();
            preferences(&mut app, timestamps, durations, "24-hour-utc");
            send(&mut app, &mut rx, "Clock question");
            event_update(&mut app, chunk("Owned clock answer", "answer-1", false));
            let owner = source_owner(&app, "answer-1");
            let id = app.transcript.messages[owner].id;
            event_update(&mut app, metadata("assistant", "answer-1", "2026-10-03T15:02:01Z"));
            event_update(
                &mut app,
                model::SessionUpdate::TurnTiming {
                    duration_ms: 2700.0,
                    api_duration_ms: Some(1800.0),
                },
            );
            clock.advance(Duration::from_secs(900));
            finish(&mut app);
            let timing = &app.transcript.messages[owner].timing;
            assert!(!timing.timestamp.unwrap().observed);
            let duration = timing.duration.unwrap();
            assert_eq!(duration.elapsed, Duration::from_millis(2700));
            assert_eq!(duration.api, Some(Duration::from_millis(1800)));
            assert!(!duration.observed);
            assert_eq!(duration.completed_at, clock.wall);
            let rows = render(&mut app, &BTreeSet::new());
            let body = region(&rows, owner, Some(LiveRowBoundaryKind::AssistantText));
            let elapsed = region(&rows, owner, Some(LiveRowBoundaryKind::AssistantDuration));
            assert!(body.contains("Owned clock answer"));
            assert_eq!(body.contains("15:02Z"), timestamps);
            assert!(!body.contains("(observed)"));
            assert_eq!(elapsed.contains("Elapsed 2.7s"), durations);
            assert_eq!(elapsed.contains("15:05Z"), timestamps && durations);
            assert!(!body.contains("Elapsed"));
            let mut excluded: BTreeSet<_> = app.transcript.messages[owner]
                .blocks
                .iter()
                .map(|b| match b {
                    MessageBlock::Text(t) => HistoryOutputId::Block(t.id),
                    _ => panic!("text source"),
                })
                .collect();
            excluded.insert(HistoryOutputId::AssistantLabel(id));
            let remaining = render(&mut app, &excluded);
            assert!(region(&remaining, owner, Some(LiveRowBoundaryKind::AssistantText)).is_empty());
            assert_eq!(
                remaining
                    .segments()
                    .iter()
                    .filter(|s| s.msg_idx == owner
                        && s.ids.contains(&HistoryOutputId::AssistantDuration(id)))
                    .count(),
                usize::from(durations)
            );
            excluded.insert(HistoryOutputId::AssistantDuration(id));
            assert!(
                region(
                    &render(&mut app, &excluded),
                    owner,
                    Some(LiveRowBoundaryKind::AssistantDuration)
                )
                .is_empty()
            );
            send(&mut app, &mut rx, "Independent next question");
            event_update(&mut app, chunk("Second clock answer", "answer-2", false));
            let next = source_owner(&app, "answer-2");
            assert!(app.transcript.messages[next].timing.timestamp.unwrap().observed);
            assert!(app.transcript.messages[next].timing.duration.is_none());
            let next_rows = region(&render(&mut app, &BTreeSet::new()), next, None);
            assert!(next_rows.contains("Second clock answer"));
            assert!(!next_rows.contains("15:02Z"));
            assert!(!next_rows.contains("Elapsed 2.7s"));
        }
    }

    #[test]
    fn queued_native_timestamp_follows_its_source_without_consuming_neighbors() {
        let _clock = TestClock::new();
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        preferences(&mut app, true, true, "24-hour-utc");
        send(&mut app, &mut rx, "Main question");
        event_update(&mut app, chunk("Main response", "main-answer", false));
        let first = send(&mut app, &mut rx, "Queued target source");
        let second = send(&mut app, &mut rx, "Queued neighbor source");
        event_update(&mut app, metadata("user", &first, "2025-01-01T10:11:00Z"));
        event_update(&mut app, metadata("user", &second, "2025-01-01T12:22:00Z"));
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::UserMessageStarted {
                session_id: "session-1".into(),
                message_uuid: first.clone(),
                source: crate::agent::types::UserMessageStartSource::StreamEvent,
            },
        );
        let owner = source_owner(&app, &first);
        let timestamp = app.transcript.messages[owner].timing.timestamp.unwrap();
        assert!(!timestamp.observed);
        assert_eq!(timestamp.time.to_rfc3339(), "2025-01-01T10:11:00+00:00");
        let shown = region(&render(&mut app, &std::collections::BTreeSet::new()), owner, None);
        assert!(shown.contains("Queued target source"));
        assert!(shown.contains("10:11Z"));
        assert!(!shown.contains("12:22Z"));
        assert!(!shown.contains("(observed)"));
        let pending = app.pending_user_messages.iter().next().unwrap();
        assert_eq!((&pending.uuid, pending.text.as_str()), (&second, "Queued neighbor source"));
        assert_eq!(pending.timestamp.unwrap().time.to_rfc3339(), "2025-01-01T12:22:00+00:00");
        assert!(
            !app.transcript
                .messages
                .iter()
                .flat_map(|m| &m.blocks)
                .any(|b| matches!(b,MessageBlock::Text(t) if t.has_source_message_uuid(&second)))
        );
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
    }

    #[test]
    fn failed_response_records_observed_elapsed_once_through_the_event_boundary() {
        use crate::ui::inline_chat_rows::LiveRowBoundaryKind;
        let clock = TestClock::new();
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        preferences(&mut app, true, true, "24-hour-utc");
        send(&mut app, &mut rx, "Failing question");
        event_update(&mut app, chunk("Owned partial response", "failed-answer", false));
        let owner = source_owner(&app, "failed-answer");
        clock.advance(Duration::from_millis(2700));
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnError {
                session_id: "session-1".into(),
                message: "clock fixture failure".into(),
                queued_turn_count: None,
                api_error_status: None,
                terminal_reason: None,
            },
        );
        let duration = app.transcript.messages[owner].timing.duration.unwrap();
        assert!(duration.observed);
        assert_eq!(duration.elapsed, Duration::from_millis(2700));
        assert_eq!(duration.api, None);
        assert_eq!(duration.completed_at.to_rfc3339(), "2026-10-03T15:05:02.700+00:00");
        let rows = render(&mut app, &std::collections::BTreeSet::new());
        assert!(
            region(&rows, owner, Some(LiveRowBoundaryKind::AssistantText))
                .contains("Owned partial response")
        );
        assert!(
            region(&rows, owner, Some(LiveRowBoundaryKind::AssistantNotice))
                .contains("clock fixture failure")
        );
        assert!(
            region(&rows, owner, Some(LiveRowBoundaryKind::AssistantDuration))
                .contains("Observed 2.7s")
        );
        clock.advance(Duration::from_secs(7200));
        finish(&mut app);
        assert_eq!(app.transcript.messages[owner].timing.duration, Some(duration));
        assert!(
            region(
                &render(&mut app, &std::collections::BTreeSet::new()),
                owner,
                Some(LiveRowBoundaryKind::AssistantDuration)
            )
            .contains("Observed 2.7s")
        );
    }

    #[test]
    fn native_metadata_keeps_first_valid_time_on_the_matching_source() {
        let _clock = TestClock::new();
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        preferences(&mut app, true, false, "24-hour-utc");
        let user_one = send(&mut app, &mut rx, "Earlier user source");
        event_update(&mut app, chunk("Earlier assistant source", "assistant-one", false));
        finish(&mut app);
        let user_two = send(&mut app, &mut rx, "Later user source");
        let active = app.active_turn_assistant_idx().unwrap();
        let observed = app.transcript.messages[active].timing.timestamp;
        // Producers can announce the active response's metadata before its first source-bearing chunk.
        event_update(&mut app, metadata("assistant", "assistant-two", "malformed"));
        assert_eq!(app.transcript.messages[active].timing.timestamp, observed);
        event_update(&mut app, metadata("assistant", "assistant-two", "2026-10-03T16:16:00Z"));
        event_update(&mut app, chunk("Later assistant source", "assistant-two", false));
        for (role, uuid, time) in [
            ("assistant", "assistant-one", "2026-10-03T11:11:00Z"),
            ("user", user_one.as_str(), "2026-10-03T10:10:00Z"),
            ("user", user_two.as_str(), "2026-10-03T14:14:00Z"),
        ] {
            let owner = source_owner(&app, uuid);
            let before = app.transcript.messages[owner].timing.timestamp;
            event_update(&mut app, metadata(role, uuid, "not a date"));
            assert_eq!(app.transcript.messages[owner].timing.timestamp, before);
            event_update(&mut app, metadata(role, uuid, time));
            event_update(&mut app, metadata(role, uuid, "2026-10-03T23:59:00Z"));
        }
        event_update(&mut app, metadata("assistant", "assistant-two", "2026-10-03T23:59:00Z"));
        let rows = render(&mut app, &std::collections::BTreeSet::new());
        let expected = [
            (user_one.as_str(), "Earlier user source", "10:10Z"),
            ("assistant-one", "Earlier assistant source", "11:11Z"),
            (user_two.as_str(), "Later user source", "14:14Z"),
            ("assistant-two", "Later assistant source", "16:16Z"),
        ];
        for (uuid, content, time) in expected {
            let owner = source_owner(&app, uuid);
            assert!(!app.transcript.messages[owner].timing.timestamp.unwrap().observed);
            let rendered = region(&rows, owner, None);
            assert!(rendered.contains(content));
            assert!(rendered.contains(time));
            assert!(!rendered.contains("(observed)"));
            assert!(!rendered.contains("23:59Z"));
            for (_, _, other) in expected {
                if other != time {
                    assert!(!rendered.contains(other));
                }
            }
        }
        assert_eq!(app.active_turn_assistant_idx(), Some(active));
    }

    #[test]
    fn elapsed_units_preserve_remainders_and_sdk_milliseconds() {
        for (millis, expected) in [
            (125, "0.1s"),
            (59_900, "59.9s"),
            (60_000, "1m 0s"),
            (3_599_000, "59m 59s"),
            (3_600_000, "1h 0m 0s"),
            (7_384_000, "2h 3m 4s"),
        ] {
            assert_eq!(elapsed(Duration::from_millis(millis)), expected);
        }
        let _clock = TestClock::new();
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        preferences(&mut app, false, true, "24-hour-utc");
        send(&mut app, &mut rx, "Long turn");
        event_update(&mut app, chunk("Long response", "long-answer", false));
        event_update(
            &mut app,
            model::SessionUpdate::TurnTiming { duration_ms: 7_384_000.0, api_duration_ms: None },
        );
        finish(&mut app);
        let owner = source_owner(&app, "long-answer");
        assert_eq!(
            app.transcript.messages[owner].timing.duration.unwrap().elapsed,
            Duration::from_secs(7384)
        );
        let shown = region(
            &render(&mut app, &std::collections::BTreeSet::new()),
            owner,
            Some(crate::ui::inline_chat_rows::LiveRowBoundaryKind::AssistantDuration),
        );
        assert!(shown.contains("Elapsed 2h 3m 4s"));
        assert!(!shown.contains("Observed"));
    }

    #[test]
    fn resumed_history_preserves_native_dates_without_inventing_observed_clocks() {
        let clock = TestClock::new();
        let (mut app, mut rx) = crate::app::test_support::app_with_connection();
        let dir = tempfile::tempdir().unwrap();
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::SessionReplaced {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().into_owned(),
                current_model: model::CurrentModel::new("model", "model", "model")
                    .authoritative(true),
                available_models: vec![],
                mode: None,
                fast_mode_state: model::FastModeState::Off,
                fast_mode_disabled_reason: None,
                ultracode: None,
                history_updates: vec![
                    chunk("Historical user", "history-user", true),
                    metadata("user", "history-user", "2024-01-02T10:11:00Z"),
                    chunk("Historical answer", "history-answer", false),
                    metadata("assistant", "history-answer", "2024-01-02T10:12:00Z"),
                    chunk("Untimed user", "untimed-user", true),
                    chunk("Untimed answer", "untimed-answer", false),
                ],
                restored_input: None,
            },
        );
        // Session replacement invalidates the previous settings snapshot.
        preferences(&mut app, true, true, "%Y-%m-%d %H:%M");
        let rows = render(&mut app, &std::collections::BTreeSet::new());
        for (uuid, source, time) in [
            ("history-user", "Historical user", Some("2024-01-02 10:11")),
            ("history-answer", "Historical answer", Some("2024-01-02 10:12")),
            ("untimed-user", "Untimed user", None),
            ("untimed-answer", "Untimed answer", None),
        ] {
            let owner = source_owner(&app, uuid);
            let message = &app.transcript.messages[owner];
            assert!(message.timing.is_finished());
            assert!(message.timing.duration.is_none());
            let shown = region(&rows, owner, None);
            assert!(shown.contains(source));
            assert!(!shown.contains("Observed"));
            assert!(!shown.contains("(observed)"));
            if let Some(time) = time {
                assert!(shown.contains(time), "historical message is missing its timestamp");
                assert!(!message.timing.timestamp.unwrap().observed);
            } else {
                assert!(message.timing.timestamp.is_none());
                assert!(!shown.contains("2026-10-03"));
            }
        }
        while let Ok(envelope) = rx.try_recv() {
            use crate::agent::wire::BridgeCommand;
            assert!(matches!(envelope.command,
                BridgeCommand::GetMcpSnapshot { session_id }
                | BridgeCommand::GetStatusSnapshot { session_id }
                | BridgeCommand::GetContextUsage { session_id }
                | BridgeCommand::InspectSettings { session_id, .. }
                if session_id == "session-1"));
        }
        send(&mut app, &mut rx, "Later live question");
        event_update(&mut app, chunk("Later live answer", "live-answer", false));
        clock.advance(Duration::from_secs(2));
        finish(&mut app);
        let owner = source_owner(&app, "live-answer");
        assert!(app.transcript.messages[owner].timing.timestamp.unwrap().observed);
        assert_eq!(
            app.transcript.messages[owner].timing.duration.unwrap().elapsed,
            Duration::from_secs(2)
        );
        let live = region(&render(&mut app, &std::collections::BTreeSet::new()), owner, None);
        assert!(live.contains("Later live answer"));
        assert!(live.contains("2026-10-03 15:05 (observed)"));
        assert!(live.contains("Observed 2.0s"));
    }
}
