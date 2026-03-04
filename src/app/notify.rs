// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

/// Events that can trigger a user notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyEvent {
    /// A tool call requires explicit user approval.
    PermissionRequired,
    /// The agent finished its turn.
    TurnComplete,
}

/// Central notification manager.
///
/// Tracks whether the terminal window is focused (via crossterm
/// `FocusGained`/`FocusLost` events backed by DECSET 1004) and dispatches
/// notifications only when the window is **not** focused.
///
/// Two notification layers fire in parallel:
/// 1. **Terminal bell** (`BEL \x07`) -- causes a taskbar flash / dock bounce
///    on virtually every terminal emulator.
/// 2. **Desktop notification** via `notify-rust` -- OS-native toast popup
///    (Windows Toast, macOS Notification Center, Linux freedesktop D-Bus).
///    Spawned on a background thread so it never blocks the TUI event loop.
///    Silently ignored when the notification backend is unavailable (e.g. SSH).
#[derive(Debug)]
pub struct NotificationManager {
    terminal_focused: bool,
}

impl NotificationManager {
    #[must_use]
    pub const fn new() -> Self {
        // Default to `true` (focused) so that terminals which do not support
        // DECSET 1004 never fire spurious notifications.
        Self {
            terminal_focused: true,
        }
    }

    /// Call when the terminal emits a `FocusGained` event.
    pub fn on_focus_gained(&mut self) {
        self.terminal_focused = true;
    }

    /// Call when the terminal emits a `FocusLost` event.
    pub fn on_focus_lost(&mut self) {
        self.terminal_focused = false;
    }

    /// Whether the terminal window currently has OS focus.
    #[must_use]
    pub const fn is_focused(&self) -> bool {
        self.terminal_focused
    }

    /// Send a notification if the terminal is not focused.
    ///
    /// This is the single entry-point that all event handlers should call.
    /// It is intentionally cheap when focused (just a bool check).
    pub fn notify(&self, event: NotifyEvent) {
        if self.terminal_focused {
            return;
        }
        ring_bell();
        send_desktop_notification(event);
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Write the ASCII BEL character to stdout, causing a taskbar flash / dock
/// bounce in most terminal emulators.
fn ring_bell() {
    use std::io::Write;
    let _ = std::io::stdout().write_all(b"\x07");
    let _ = std::io::stdout().flush();
}

/// Spawn a background thread that sends an OS-native desktop notification.
///
/// Runs on `std::thread::spawn` rather than tokio because `notify-rust`'s
/// `show()` may block on a D-Bus round-trip (Linux) or COM call (Windows).
/// Errors are silently discarded -- the bell is the reliable fallback.
fn send_desktop_notification(event: NotifyEvent) {
    let (summary, body) = match event {
        NotifyEvent::PermissionRequired => (
            "Claude Code",
            "Permission required -- waiting for your approval",
        ),
        NotifyEvent::TurnComplete => ("Claude Code", "Turn complete"),
    };
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .summary(summary)
            .body(body)
            .show();
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_focused() {
        let mgr = NotificationManager::new();
        assert!(
            mgr.is_focused(),
            "should default to focused to suppress spurious notifications"
        );
    }

    #[test]
    fn focus_lost_sets_unfocused() {
        let mut mgr = NotificationManager::new();
        mgr.on_focus_lost();
        assert!(!mgr.is_focused());
    }

    #[test]
    fn focus_gained_restores_focused() {
        let mut mgr = NotificationManager::new();
        mgr.on_focus_lost();
        mgr.on_focus_gained();
        assert!(mgr.is_focused());
    }
}
