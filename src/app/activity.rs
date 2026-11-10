// SPDX-License-Identifier: Apache-2.0

use std::time::{Duration, Instant};

use crate::agent::model::RuntimeSessionState;
use crate::ui::{host_tips::HOST_TIPS, spinner_verbs::random_spinner_verb};

use super::{App, AppStatus};

pub(crate) fn now() -> Instant {
    #[cfg(test)]
    if let Some(now) = testing::NOW.with(std::cell::Cell::get) {
        return now;
    }
    Instant::now()
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    thread_local! { pub(super) static NOW: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) }; }
    pub(crate) struct Clock(Option<Instant>);
    impl Clock {
        pub(crate) fn at(now: Instant) -> Self {
            Self(NOW.replace(Some(now)))
        }
        // Borrowing the guard keeps time changes within its restoration scope.
        #[allow(clippy::unused_self)]
        pub(crate) fn set(&self, now: Instant) {
            NOW.set(Some(now));
        }
    }
    impl Drop for Clock {
        fn drop(&mut self) {
            NOW.set(self.0);
        }
    }
    #[allow(clippy::expect_used)]
    pub(crate) fn choices(app: &mut App) -> &'static str {
        let verb = random_spinner_verb();
        let activity = app.turn.activity.as_mut().expect("admitted activity");
        activity.verb = verb;
        for (tip, index) in activity.tips.iter_mut().zip(0_u8..) {
            *tip = index;
        }
        verb
    }
}

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
                || !self.pending_interaction_ids.is_empty()
                || !self.mcp.elicitations.is_empty()
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
