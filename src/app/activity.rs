// SPDX-License-Identifier: Apache-2.0

use std::time::{Duration, Instant};

use crate::agent::model::RuntimeSessionState;
use crate::ui::{host_tips::HOST_TIPS, spinner_verbs::random_spinner_verb};

use super::{App, AppStatus};

const TIP_INTERVAL: Duration = Duration::from_secs(60);

/// Presentation choices and repaint deadline belong to the turn, not a response or tool.
pub(crate) struct TurnActivity {
    started_at: Instant,
    verb: &'static str,
    tips: [u8; HOST_TIPS.len()],
    next_tip_at: Instant,
}

impl TurnActivity {
    pub(crate) fn new(now: Instant) -> Self {
        let mut tips = [0; HOST_TIPS.len()];
        for (tip, index) in tips.iter_mut().zip(0_u8..) {
            *tip = index;
        }
        fastrand::shuffle(&mut tips);
        Self { started_at: now, verb: random_spinner_verb(), tips, next_tip_at: now + TIP_INTERVAL }
    }

    fn tip_slot(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.started_at).as_secs() / TIP_INTERVAL.as_secs()
    }

    fn tip(&self, now: Instant) -> Option<&'static str> {
        let slot = self.tip_slot(now);
        let index = usize::try_from(slot % HOST_TIPS.len() as u64).ok()?;
        Some(HOST_TIPS[usize::from(self.tips[index])])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityLabel {
    Working,
    Compacting,
    Cancelling,
}

/// The heading owner is derived alongside activity visibility, never stored on the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityHeading {
    Assistant(usize),
    Composer,
}

pub(crate) struct ActivityPresentation {
    pub label: ActivityLabel,
    pub heading: ActivityHeading,
    /// The SDK thinking phase is rendered separately in the active assistant output.
    pub thinking: bool,
    pub verb: &'static str,
    pub tip: Option<&'static str>,
}

impl ActivityPresentation {
    pub fn text(&self) -> String {
        let label = match self.label {
            ActivityLabel::Working => self.verb,
            ActivityLabel::Compacting => "Compacting",
            ActivityLabel::Cancelling => "Cancelling",
        };
        format!("{label}…")
    }
}

impl App {
    /// Called only on genuine work entry; folded-in prompts and liveness updates retain choices.
    pub(crate) fn begin_turn_activity(&mut self, now: Instant) {
        if self.turn.activity.is_none() {
            self.turn.activity = Some(TurnActivity::new(now));
            self.session_runtime.runtime_session_state = None;
        }
    }

    /// One display decision shared by rendering and repaint scheduling.
    pub(crate) fn activity_presentation(&self, now: Instant) -> Option<ActivityPresentation> {
        let label = if self.turn.cancel_requested {
            ActivityLabel::Cancelling
        } else {
            if !self.is_agent_turn_active()
                || matches!(
                    self.session_runtime.runtime_session_state,
                    Some(RuntimeSessionState::Idle | RuntimeSessionState::RequiresAction)
                )
                || !self.turn.pending_interaction_ids.is_empty()
                || self.mcp.pending_elicitation.is_some()
            {
                return None;
            }
            if self.turn.compaction.is_active() {
                ActivityLabel::Compacting
            } else {
                ActivityLabel::Working
            }
        };
        let activity = self.turn.activity.as_ref();
        Some(ActivityPresentation {
            label,
            heading: self
                .active_turn_assistant_idx()
                .map_or(ActivityHeading::Composer, ActivityHeading::Assistant),
            thinking: label == ActivityLabel::Working && self.status == AppStatus::Thinking,
            verb: activity.map_or("Working", |activity| activity.verb),
            tip: if label == ActivityLabel::Working && self.config.spinner_tips_enabled_effective()
            {
                activity.and_then(|activity| activity.tip(now))
            } else {
                None
            },
        })
    }

    pub(crate) fn tick_activity(&mut self, now: Instant) {
        let visible =
            self.activity_presentation(now).is_some_and(|presentation| presentation.tip.is_some());
        let Some(activity) = self.turn.activity.as_mut() else { return };
        if now < activity.next_tip_at {
            return;
        }
        let slot = activity.tip_slot(now);
        activity.next_tip_at =
            activity.started_at + TIP_INTERVAL * u32::try_from(slot + 1).unwrap_or(u32::MAX);
        if visible {
            self.request_chat_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{events::ClientEvent, model, settings::SettingsSnapshot};

    fn update(app: &mut App, update: model::SessionUpdate) {
        crate::app::events::handle_client_event(
            app,
            ClientEvent::SessionUpdate { session_id: "activity-session".into(), update },
        );
    }

    #[tokio::test]
    async fn failure_disconnect_and_conversation_replacement_clear_activity() {
        let events = [
            ClientEvent::TurnError {
                session_id: "activity-session".into(),
                message: "failed".into(),
                queued_turn_count: Some(0),
                api_error_status: None,
                terminal_reason: None,
            },
            ClientEvent::ConnectionFailed("closed".into()),
            ClientEvent::SessionUpdate {
                session_id: "activity-session".into(),
                update: model::SessionUpdate::ConversationReset {
                    new_conversation_id: "replacement".into(),
                    trigger: None,
                    timestamp: None,
                    user_message_uuid: None,
                },
            },
        ];
        for event in events {
            let mut app = App::test_default();
            app.session_runtime.session_id = Some("activity-session".into());
            let now = Instant::now();
            app.status = AppStatus::Thinking;
            app.begin_turn_activity(now);
            crate::app::events::handle_client_event(&mut app, event);
            assert!(app.turn.activity.is_none());
            assert!(app.activity_presentation(now).is_none());
        }
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn activity_survives_output_and_user_wait_without_restarting_tip_clock() {
        let mut app = App::test_default();
        app.session_runtime.session_id = Some("activity-session".into());
        let start = Instant::now();
        app.status = AppStatus::Running;
        app.begin_turn_activity(start);
        let initial = app.activity_presentation(start).expect("dispatch visible");
        assert_eq!(initial.label, ActivityLabel::Working);
        assert_ne!(initial.verb, "Thinking");
        let verb = initial.verb;
        assert!(!initial.thinking);
        let first = initial.tip;
        assert!(first.is_some());
        assert_eq!(
            app.activity_presentation(start + Duration::from_millis(59999)).expect("active").tip,
            first
        );
        let second =
            app.activity_presentation(start + Duration::from_secs(60)).expect("active").tip;
        assert_ne!(second, first);

        update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        );
        update(
            &mut app,
            model::SessionUpdate::RuntimeSessionStateUpdate(RuntimeSessionState::Running),
        );
        let thinking = app.activity_presentation(start).expect("thinking");
        assert_eq!(thinking.label, ActivityLabel::Working);
        assert!(thinking.thinking);
        assert_eq!(thinking.text(), initial.text());
        update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Working),
        );
        update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("Writing")),
            )),
        );
        update(
            &mut app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("tool", "Read").status(model::ToolCallStatus::InProgress),
            ),
        );
        update(
            &mut app,
            model::SessionUpdate::ToolCallUpdate(model::ToolCallUpdate::new(
                "tool",
                model::ToolCallUpdateFields::new().status(model::ToolCallStatus::Completed),
            )),
        );
        let working = app
            .activity_presentation(start + Duration::from_secs(60))
            .expect("tools do not end activity");
        assert_eq!(working.label, ActivityLabel::Working);
        assert!(!working.thinking);
        assert_eq!(working.verb, verb);
        assert_eq!(working.tip, second);

        app.turn.pending_interaction_ids.push("tool".into());
        assert!(
            app.activity_presentation(start).is_none(),
            "locally delivered interaction hides immediately"
        );
        update(
            &mut app,
            model::SessionUpdate::RuntimeSessionStateUpdate(RuntimeSessionState::RequiresAction),
        );
        app.turn.pending_interaction_ids.clear();
        assert!(app.activity_presentation(start).is_none(), "wait for SDK work to resume");
        update(
            &mut app,
            model::SessionUpdate::RuntimeSessionStateUpdate(RuntimeSessionState::Running),
        );
        let resumed = app.activity_presentation(start + Duration::from_secs(130)).expect("resumed");
        assert_eq!(resumed.verb, verb);
        assert_ne!(resumed.tip, second);
        assert_eq!(app.turn.activity.as_ref().expect("same turn").started_at, start);

        update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        );
        crate::app::events::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: "activity-session".into(),
                queued_turn_count: Some(0),
                terminal_reason: None,
            },
        );
        assert!(app.activity_presentation(start).is_none());
        assert!(app.turn.activity.is_none());
        update(
            &mut app,
            model::SessionUpdate::AgentMessageChunk(model::ContentChunk::new(
                model::ContentBlock::Text(model::TextContent::new("Late output")),
            )),
        );
        update(
            &mut app,
            model::SessionUpdate::AgentActivityUpdate(model::AgentActivityPhase::Thinking),
        );
        update(
            &mut app,
            model::SessionUpdate::ToolCall(
                model::ToolCall::new("detached", "Background")
                    .status(model::ToolCallStatus::InProgress),
            ),
        );
        assert!(
            app.activity_presentation(start).is_none(),
            "late output and detached tools cannot restart activity"
        );
        app.status = AppStatus::Running;
        app.begin_turn_activity(start + Duration::from_secs(200));
        assert!(
            app.activity_presentation(start + Duration::from_secs(200))
                .expect("new turn")
                .tip
                .is_some()
        );
    }

    #[test]
    fn disabled_tips_skip_tip_repaints_and_retain_turn_choices() {
        let mut app = App::test_default();
        let start = Instant::now();
        app.status = AppStatus::Running;
        app.begin_turn_activity(start);
        let initial = app.activity_presentation(start).expect("active");
        let next_tip = app.activity_presentation(start + TIP_INTERVAL).expect("active").tip;
        app.config.snapshot =
            Some(SettingsSnapshot::test_value("spinnerTipsEnabled", serde_json::json!(false)));
        app.surface_dirty.chat.repaint = false;
        app.tick_activity(start + TIP_INTERVAL);
        assert!(!app.surface_dirty.chat.repaint);
        assert_eq!(app.turn.activity.as_ref().expect("same turn").started_at, start);
        app.config.snapshot =
            Some(SettingsSnapshot::test_value("spinnerTipsEnabled", serde_json::json!(true)));
        let resumed = app.activity_presentation(start + TIP_INTERVAL).expect("active");
        assert!(resumed.tip.is_some());
        assert_eq!(resumed.verb, initial.verb);
        assert_eq!(resumed.tip, next_tip);
        app.tick_activity(start + TIP_INTERVAL * 2);
        assert!(app.surface_dirty.chat.repaint);
    }

    #[test]
    fn cancellation_compaction_and_reduced_motion_keep_one_presentation_owner() {
        let mut app = App::test_default();
        let start = Instant::now();
        app.begin_turn_activity(start);
        app.turn.compaction.begin_manual();
        assert_eq!(
            app.activity_presentation(start).expect("standalone compaction").label,
            ActivityLabel::Compacting
        );
        app.turn.cancel_requested = true;
        app.session_runtime.runtime_session_state = Some(RuntimeSessionState::RequiresAction);
        let cancel =
            app.activity_presentation(start + Duration::from_secs(10)).expect("cancel priority");
        assert_eq!(cancel.label, ActivityLabel::Cancelling);
        assert!(!cancel.thinking);
        assert!(cancel.tip.is_none());
        app.turn.reset_for_turn_exit();
        app.status = AppStatus::Running;
        app.begin_turn_activity(start);
        app.config.snapshot =
            Some(SettingsSnapshot::test_value("prefersReducedMotion", serde_json::json!(true)));
        app.surface_dirty.chat.repaint = false;
        app.tick_activity(start + Duration::from_millis(59999));
        assert!(!app.surface_dirty.chat.repaint);
        assert_eq!(
            app.turn.activity.as_ref().expect("active").next_tip_at,
            start + Duration::from_secs(60)
        );
        app.tick_activity(start + Duration::from_secs(60));
        assert!(app.surface_dirty.chat.repaint, "tip deadline repaints without animation");
        assert_eq!(
            app.turn.activity.as_ref().expect("active").next_tip_at,
            start + Duration::from_secs(120)
        );
        app.surface_dirty.chat.repaint = false;
        app.tick_activity(start + Duration::from_secs(61));
        assert!(!app.surface_dirty.chat.repaint, "tips do not continuously repaint");
        app.session_runtime.runtime_session_state = Some(RuntimeSessionState::RequiresAction);
        app.tick_activity(start + Duration::from_secs(120));
        assert!(!app.surface_dirty.chat.repaint, "hidden tips advance without painting");
        app.session_runtime.runtime_session_state = Some(RuntimeSessionState::Running);
        app.tick_activity(start + Duration::from_secs(135));
        assert_eq!(
            app.turn.activity.as_ref().expect("active").next_tip_at,
            start + Duration::from_secs(180)
        );
        let tips: Vec<_> = (0..HOST_TIPS.len())
            .map(|slot| {
                app.turn
                    .activity
                    .as_ref()
                    .expect("active")
                    .tip(start + TIP_INTERVAL * u32::try_from(slot).expect("slot"))
            })
            .collect();
        let mut unique = tips.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), HOST_TIPS.len());
        assert_ne!(tips.last().copied().flatten(), tips.first().copied().flatten());
    }
}
