// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use crate::agent::notifications::SdkNotification;
use crate::app::config::ConfigState;
use crate::logging::targets::APP_NOTIFY;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyEvent {
    PermissionRequired,
    QuestionRequired,
    TurnComplete,
    ModelDirected,
}

/// Focus, category policy, transport selection and SDK delivery identity have one owner.
/// Local callers notify only after a real waiting/completion transition is accepted.
#[derive(Debug)]
pub struct NotificationManager {
    terminal_focused: bool,
    capabilities: TerminalCapabilities,
    recent: VecDeque<(String, NotificationIdentity)>,
    deliver: fn(NotificationDelivery),
}

#[derive(Debug, PartialEq, Eq)]
enum NotificationIdentity {
    Notice(String),
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotificationDelivery {
    pub(crate) ring_bell: bool,
    pub(crate) send_desktop: bool,
    pub(crate) escape_sequence: Option<String>,
    pub(crate) body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TerminalCapabilities {
    #[default]
    Other,
    Iterm2,
    Ghostty,
    Kitty,
}

impl Default for NotificationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationManager {
    #[must_use]
    pub fn new() -> Self {
        let manager = Self {
            terminal_focused: true,
            capabilities: terminal_capabilities_from_env(std::env::vars_os().filter_map(
                |(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)),
            )),
            recent: VecDeque::new(),
            deliver: deliver_notification,
        };
        tracing::debug!(target: APP_NOTIFY, event_name = "notification_manager_initialized",
            terminal_focused = manager.terminal_focused, terminal_capabilities = ?manager.capabilities);
        manager
    }

    pub fn on_focus_gained(&mut self) {
        self.observe_focus(true);
    }
    pub fn on_focus_lost(&mut self) {
        self.observe_focus(false);
    }

    fn observe_focus(&mut self, focused: bool) {
        let previous = self.terminal_focused;
        self.terminal_focused = focused;
        tracing::debug!(target: APP_NOTIFY, event_name = "notification_focus_observed",
            terminal_focused = focused, previously_focused = previous);
    }

    pub fn notify(
        &self,
        config: &ConfigState,
        event: NotifyEvent,
        session_id: &str,
        interaction_id: Option<&str>,
    ) {
        let _span = tracing::debug_span!(target: APP_NOTIFY, "local_notification",
            session_id, interaction_id = interaction_id.unwrap_or_default(), origin = "local_transition").entered();
        self.dispatch(config, event, notification_text(event));
    }

    /// Replay seeds identities without sending alerts. Suppressed live events are also
    /// consumed: switching focus/settings later must not turn an old event into an alert.
    /// Return whether a native notice is new for transcript presentation.
    pub(crate) fn observe_sdk(
        &mut self,
        config: &ConfigState,
        notification: &SdkNotification,
        replay: bool,
    ) -> bool {
        match notification {
            SdkNotification::SdkNotice { session_id, uuid, priority, .. } => {
                let new = self.remember(session_id, NotificationIdentity::Notice(uuid.clone()));
                let visible = new || replay;
                tracing::debug!(target: APP_NOTIFY, event_name = "notification_sdk_handled",
                    origin = "sdk_notice", session_id, notification_uuid = uuid, priority, replay,
                    outcome = if visible { "transcript" } else { "suppressed" },
                    reason = if visible { "native_ui_notice" } else { "duplicate" });
                visible
            }
            SdkNotification::ModelTool {
                session_id,
                tool_use_id,
                text,
                local_sent,
                push_sent,
                disabled_reason,
                ..
            } => {
                let _span = tracing::debug_span!(target: APP_NOTIFY, "sdk_notification",
                    session_id, tool_call_id = tool_use_id, origin = "model_tool", replay)
                .entered();
                let new =
                    self.remember(session_id, NotificationIdentity::Tool(tool_use_id.clone()));
                let reason = if !new {
                    Some("duplicate")
                } else if replay {
                    Some("replay")
                } else if *local_sent == Some(true) {
                    Some("upstream_local_delivery")
                } else if local_sent.is_none() {
                    Some("missing_local_report")
                } else if !matches!(disabled_reason.as_deref(), None | Some("no_transport")) {
                    Some("sdk_suppressed")
                } else {
                    None
                };
                tracing::debug!(target: APP_NOTIFY, event_name = "notification_sdk_handled",
                    outcome = if reason.is_some() { "suppressed" } else { "local_candidate" },
                    reason = reason.unwrap_or("eligible"),
                    local_report_present = local_sent.is_some(), local_sent = local_sent.unwrap_or(false),
                    push_report_present = push_sent.is_some(), push_sent = push_sent.unwrap_or(false),
                    disabled_reason = disabled_reason.as_deref().unwrap_or_default());
                if reason.is_none() {
                    self.dispatch(config, NotifyEvent::ModelDirected, text);
                }
                false // The canonical tool call already shows the input and delivery report.
            }
        }
    }

    /// Completed historical tools can be replayed later on the live stream. Their IDs
    /// establish a replay boundary even when old SDK history omits delivery metadata.
    pub(crate) fn observe_history_tool(&mut self, session_id: &str, tool_id: &str) {
        self.remember(session_id, NotificationIdentity::Tool(tool_id.to_owned()));
    }

    fn remember(&mut self, session_id: &str, identity: NotificationIdentity) -> bool {
        const MAX_IDENTITIES: usize = 512;
        if self.recent.iter().any(|(session, seen)| session == session_id && seen == &identity) {
            return false;
        }
        if self.recent.len() == MAX_IDENTITIES {
            self.recent.pop_front();
        }
        self.recent.push_back((session_id.to_owned(), identity));
        true
    }

    fn dispatch(&self, config: &ConfigState, event: NotifyEvent, body: &str) {
        let settings_available = config.snapshot.is_some();
        let category_enabled = config.notification_enabled(event);
        let channel = config.preferred_notification_channel_effective();
        let suppression = if !settings_available {
            Some("settings_unavailable")
        } else if self.terminal_focused {
            Some("terminal_focused")
        } else if !category_enabled {
            Some("category_disabled")
        } else {
            None
        };
        let delivery =
            suppression.is_none().then(|| notification_plan(channel, self.capabilities, body));
        let ring_bell = delivery.as_ref().is_some_and(|plan| plan.ring_bell);
        let send_desktop = delivery.as_ref().is_some_and(|plan| plan.send_desktop);
        let terminal_protocol =
            delivery.as_ref().is_some_and(|plan| plan.escape_sequence.is_some());
        let will_deliver = ring_bell || send_desktop || terminal_protocol;
        tracing::debug!(target: APP_NOTIFY, event_name = "notification_delivery_decided",
            category = ?event, settings_available, terminal_focused = self.terminal_focused, category_enabled,
            notification_method = ?channel, terminal_capabilities = ?self.capabilities,
            ring_bell, send_desktop, terminal_protocol,
            outcome = if will_deliver { "dispatch" } else { "suppressed" },
            reason = suppression.unwrap_or(if will_deliver { "transport_selected" } else { "notifications_disabled" }));
        if will_deliver && let Some(delivery) = delivery {
            (self.deliver)(delivery);
        }
    }

    #[cfg(test)]
    pub(crate) fn with_delivery(
        deliver: fn(NotificationDelivery),
        capabilities: TerminalCapabilities,
    ) -> Self {
        Self { capabilities, deliver, ..Self::new() }
    }
}

fn deliver_notification(delivery: NotificationDelivery) {
    let mut stdout = std::io::stdout().lock();
    deliver_terminal_notification(&delivery, &mut stdout);
    drop(stdout);
    if delivery.send_desktop {
        // OS delivery may block on COM/D-Bus. Keep it outside the TUI loop.
        let span = tracing::Span::current();
        std::thread::spawn(move || {
            let _entered = span.enter();
            if let Err(error) =
                notify_rust::Notification::new().summary("Claude Code").body(&delivery.body).show()
            {
                tracing::debug!(target: APP_NOTIFY, event_name = "notification_transport_result",
                    transport = "desktop", outcome = "failure", error_kind = "desktop_backend", %error);
            } else {
                tracing::debug!(target: APP_NOTIFY, event_name = "notification_transport_result",
                    transport = "desktop", outcome = "success", reason = "backend_accepted");
            }
        });
    }
}

fn deliver_terminal_notification(
    delivery: &NotificationDelivery,
    writer: &mut impl std::io::Write,
) {
    if let Some(sequence) = &delivery.escape_sequence {
        log_terminal_result("terminal_protocol", writer.write_all(sequence.as_bytes()));
    }
    if delivery.ring_bell {
        log_terminal_result("bell", writer.write_all(b"\x07"));
    }
    log_terminal_result("terminal_flush", writer.flush());
}

fn log_terminal_result(transport: &str, result: std::io::Result<()>) {
    match result {
        Ok(()) => tracing::debug!(target: APP_NOTIFY, event_name = "notification_transport_result",
            transport, outcome = "success", reason = "output_written"),
        Err(error) => {
            tracing::debug!(target: APP_NOTIFY, event_name = "notification_transport_result",
            transport, outcome = "failure", error_kind = ?error.kind(), %error);
        }
    }
}

fn notification_plan(
    channel: PreferredNotifChannel,
    capabilities: TerminalCapabilities,
    body: &str,
) -> NotificationDelivery {
    use PreferredNotifChannel as Channel;
    let protocol = match (channel, capabilities) {
        (Channel::Auto, terminal) => terminal,
        (Channel::Iterm2 | Channel::Iterm2WithBell, TerminalCapabilities::Iterm2) => {
            TerminalCapabilities::Iterm2
        }
        (Channel::Ghostty, TerminalCapabilities::Ghostty) => TerminalCapabilities::Ghostty,
        (Channel::Kitty, TerminalCapabilities::Kitty) => TerminalCapabilities::Kitty,
        _ => TerminalCapabilities::Other,
    };
    let escape_sequence = match protocol {
        TerminalCapabilities::Kitty => {
            Some(format!("\x1b]99;;Claude Code: {}\x1b\\", sanitize_message(body)))
        }
        TerminalCapabilities::Iterm2 | TerminalCapabilities::Ghostty => {
            Some(format!("\x1b]9;Claude Code: {}\x1b\\", sanitize_message(body)))
        }
        TerminalCapabilities::Other => None,
    };
    let disabled = channel == Channel::NotificationsDisabled;
    let bell_only = channel == Channel::TerminalBell;
    NotificationDelivery {
        ring_bell: !disabled
            && (bell_only
                || channel == Channel::Iterm2WithBell
                || (channel == Channel::Auto && escape_sequence.is_none())),
        send_desktop: !disabled && !bell_only && escape_sequence.is_none(),
        escape_sequence: if disabled || bell_only { None } else { escape_sequence },
        body: sanitize_message(body),
    }
}

fn terminal_capabilities_from_env<I: IntoIterator<Item = (String, String)>>(
    vars: I,
) -> TerminalCapabilities {
    let mut program = None;
    let mut iterm = false;
    let mut kitty = false;
    for (key, value) in vars {
        match key.as_str() {
            "TERM_PROGRAM" => program = Some(value),
            "ITERM_SESSION_ID" if !value.is_empty() => iterm = true,
            "KITTY_WINDOW_ID" if !value.is_empty() => kitty = true,
            "TERM" if value == "xterm-kitty" => kitty = true,
            _ => {}
        }
    }
    match program.as_deref() {
        Some("iTerm.app") => TerminalCapabilities::Iterm2,
        Some("ghostty") => TerminalCapabilities::Ghostty,
        Some("kitty") => TerminalCapabilities::Kitty,
        _ if iterm => TerminalCapabilities::Iterm2,
        _ if kitty => TerminalCapabilities::Kitty,
        _ => TerminalCapabilities::Other,
    }
}

const fn notification_text(event: NotifyEvent) -> &'static str {
    match event {
        NotifyEvent::PermissionRequired => "Permission required -- waiting for your approval",
        NotifyEvent::QuestionRequired => "Question required -- waiting for your input",
        NotifyEvent::TurnComplete => "Turn complete",
        NotifyEvent::ModelDirected => "Claude requests your attention",
    }
}

fn sanitize_message(message: &str) -> String {
    message
        .chars()
        .filter_map(|ch| match ch {
            '\r' | '\n' | '\t' => Some(' '),
            _ if ch.is_control() => None,
            _ => Some(ch),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreferredNotifChannel {
    #[default]
    Auto,
    Iterm2,
    Iterm2WithBell,
    TerminalBell,
    NotificationsDisabled,
    Kitty,
    Ghostty,
}

impl PreferredNotifChannel {
    #[must_use]
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "iterm2" => Some(Self::Iterm2),
            "iterm2_with_bell" => Some(Self::Iterm2WithBell),
            "terminal_bell" => Some(Self::TerminalBell),
            "notifications_disabled" => Some(Self::NotificationsDisabled),
            "kitty" => Some(Self::Kitty),
            "ghostty" => Some(Self::Ghostty),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_plans_match_real_terminal_protocols_and_fallbacks() {
        for (channel, terminal, prefix) in [
            (PreferredNotifChannel::Iterm2, TerminalCapabilities::Iterm2, "\x1b]9;"),
            (PreferredNotifChannel::Ghostty, TerminalCapabilities::Ghostty, "\x1b]9;"),
            (PreferredNotifChannel::Kitty, TerminalCapabilities::Kitty, "\x1b]99;;"),
        ] {
            let plan = notification_plan(channel, terminal, "hello\n\x1bworld\x07\u{9c}");
            assert_eq!(
                plan.escape_sequence,
                Some(format!("{prefix}Claude Code: hello world\x1b\\"))
            );
            assert!(!plan.ring_bell && !plan.send_desktop);
            assert!(notification_plan(channel, TerminalCapabilities::Other, "hello").send_desktop);
            assert_eq!(
                notification_plan(PreferredNotifChannel::Auto, terminal, "hello"),
                notification_plan(channel, terminal, "hello")
            );
        }
        let auto =
            notification_plan(PreferredNotifChannel::Auto, TerminalCapabilities::Other, "hello");
        assert!(auto.ring_bell && auto.send_desktop);
        let bell = notification_plan(
            PreferredNotifChannel::TerminalBell,
            TerminalCapabilities::Kitty,
            "hello",
        );
        assert!(bell.ring_bell && !bell.send_desktop && bell.escape_sequence.is_none());
        let both = notification_plan(
            PreferredNotifChannel::Iterm2WithBell,
            TerminalCapabilities::Iterm2,
            "hello",
        );
        assert!(both.ring_bell && both.escape_sequence.is_some() && !both.send_desktop);
        let disabled = notification_plan(
            PreferredNotifChannel::NotificationsDisabled,
            TerminalCapabilities::Kitty,
            "hello",
        );
        assert!(
            !disabled.ring_bell && !disabled.send_desktop && disabled.escape_sequence.is_none()
        );
    }

    #[test]
    fn detects_terminal_notification_capabilities_without_confusing_platforms() {
        for (key, value, expected) in [
            ("TERM_PROGRAM", "iTerm.app", TerminalCapabilities::Iterm2),
            ("ITERM_SESSION_ID", "w0t1", TerminalCapabilities::Iterm2),
            ("TERM_PROGRAM", "ghostty", TerminalCapabilities::Ghostty),
            ("KITTY_WINDOW_ID", "1", TerminalCapabilities::Kitty),
            ("TERM", "xterm-kitty", TerminalCapabilities::Kitty),
            ("TERM_PROGRAM", "Windows_Terminal", TerminalCapabilities::Other),
        ] {
            assert_eq!(
                terminal_capabilities_from_env([(key.to_owned(), value.to_owned())]),
                expected
            );
        }
    }
}
