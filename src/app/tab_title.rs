// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

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
pub fn update_tab_title(status: &AppStatus, spinner_frame: usize, cwd: &str) {
    let name = folder_name(cwd);
    let active = pulse_char(spinner_frame);

    let title = match status {
        AppStatus::Connecting
        | AppStatus::CommandPending
        | AppStatus::Thinking
        | AppStatus::Running => {
            format!("{active} {name}")
        }
        AppStatus::Ready | AppStatus::Error => format!("{IDLE_CHAR} {name}"),
    };

    write_osc2_title(&title);
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
    fn active_sequence_toggles_between_open_and_filled_diamond() {
        assert_eq!(ACTIVE_CHARS, &['\u{25C7}', '\u{25C6}']);
    }

    #[test]
    fn idle_indicator_is_open_circle() {
        assert_eq!(IDLE_CHAR, '\u{25CB}');
    }

    #[test]
    fn pulse_char_changes_more_slowly_than_spinner_frame() {
        assert_eq!(pulse_char(0), pulse_char(PULSE_FRAME_DIVISOR - 1));
        assert_ne!(pulse_char(0), pulse_char(PULSE_FRAME_DIVISOR));
    }

    #[test]
    fn title_prefix_uses_single_indicator_column() {
        let active = {
            let pulse = pulse_char(0);
            format!("{pulse} claude_rust")
        };
        let idle = format!("{IDLE_CHAR} claude_rust");

        assert_eq!(active.chars().nth(1), Some(' '));
        assert_eq!(idle.chars().nth(1), Some(' '));
    }
}
