// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Terminal tab/window title management.
//!
//! Uses OSC 2 escape sequences to update the terminal tab title with a simple
//! busy toggle during active agent turns and a static idle icon otherwise.

use super::state::AppStatus;
use std::io::Write;

const ACTIVE_CHARS: &[char] = &['\u{25C7}', '\u{25C6}'];
const IDLE_CHAR: char = '\u{25CB}';
const PULSE_FRAME_DIVISOR: usize = 10;

/// Extract the last path component from a cwd string for use as the tab label.
fn folder_name(cwd: &str) -> &str {
    cwd.rsplit(['/', '\\']).find(|segment| !segment.is_empty()).unwrap_or("claude_rust")
}

/// Write an OSC 2 (set window title) escape sequence to stdout.
fn write_osc2_title(title: &str) {
    let mut buf = Vec::with_capacity(title.len() + 6);
    buf.extend_from_slice(b"\x1b]2;");
    buf.extend_from_slice(title.as_bytes());
    buf.extend_from_slice(b"\x07");
    let _ = std::io::stdout().write_all(&buf);
    let _ = std::io::stdout().flush();
}

/// Update the terminal tab title to reflect the current app status.
///
/// Called every frame tick during animating states, and on state transitions
/// for static states (Ready, Error).
pub fn update_tab_title(app: &super::App) {
    write_osc2_title(&title(app));
}

fn title(app: &super::App) -> String {
    // Stock Claude Code names the tab after the session once it has a title.
    let name = app.session_runtime.session_title.as_deref().unwrap_or(folder_name(&app.cwd_raw));
    if !app.config.status_in_terminal_tab_effective() {
        return name.to_owned();
    }
    let active = if app.config.prefers_reduced_motion_effective() {
        '\u{25C6}'
    } else {
        pulse_char(app.spinner_frame)
    };

    match &app.status {
        AppStatus::Connecting
        | AppStatus::CommandPending
        | AppStatus::Thinking
        | AppStatus::Running => {
            format!("{active} {name}")
        }
        AppStatus::Ready | AppStatus::Error => format!("{IDLE_CHAR} {name}"),
    }
}

fn pulse_char(spinner_frame: usize) -> char {
    let pulse_frame = spinner_frame / PULSE_FRAME_DIVISOR;
    ACTIVE_CHARS[pulse_frame % ACTIVE_CHARS.len()]
}

/// Restore the terminal tab title to a clean folder name (no status icon).
///
/// Called on graceful shutdown.
pub fn restore_tab_title(cwd: &str) {
    write_osc2_title(folder_name(cwd));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acknowledged_preferences_control_activity_and_reduced_motion_in_the_title() {
        let mut app = super::super::App::test_default();
        app.status = AppStatus::Running;
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "presentation.showStatusInTerminalTab",
            serde_json::json!(false),
        ));
        assert_eq!(title(&app), "test");
        let mut snapshot = crate::agent::settings::SettingsSnapshot::test_value(
            "prefersReducedMotion",
            serde_json::json!(true),
        );
        snapshot.values.extend(
            crate::agent::settings::SettingsSnapshot::test_value(
                "presentation.showStatusInTerminalTab",
                serde_json::json!(true),
            )
            .values,
        );
        app.config.snapshot = Some(snapshot);
        for frame in [0, 10, 100] {
            app.spinner_frame = frame;
            assert_eq!(title(&app), "◆ test");
        }
        app.status = AppStatus::Ready;
        assert_eq!(title(&app), "○ test");
        app.session_runtime.session_title = Some("probe-e2e".to_owned());
        assert_eq!(title(&app), "○ probe-e2e");
    }

    #[test]
    fn folder_name_extracts_last_component() {
        assert_eq!(folder_name("/home/user/projects/claude_rust"), "claude_rust");
        assert_eq!(folder_name("C:\\Users\\Simon\\Desktop\\claude_rust"), "claude_rust");
        assert_eq!(folder_name("claude_rust"), "claude_rust");
    }

    #[test]
    fn folder_name_falls_back_for_empty_or_root() {
        // Path::file_name returns None for root paths or empty strings
        assert_eq!(folder_name(""), "claude_rust");
    }

    #[test]
    fn pulse_char_cycles_between_distinct_active_frames_and_idle_is_separate() {
        let first = pulse_char(0);
        let same_window = pulse_char(PULSE_FRAME_DIVISOR - 1);
        let next = pulse_char(PULSE_FRAME_DIVISOR);

        assert_eq!(first, same_window);
        assert_ne!(first, next);
        assert_ne!(IDLE_CHAR, first);
        assert_ne!(IDLE_CHAR, next);
    }
}
