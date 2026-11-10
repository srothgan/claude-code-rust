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
fn write_osc2_title(title: &str, writer: &mut impl Write) {
    let mut buf = Vec::with_capacity(title.len() + 6);
    buf.extend_from_slice(b"\x1b]2;");
    buf.extend_from_slice(crate::ui::terminal_output::sanitize_osc_payload(title).as_bytes());
    buf.extend_from_slice(b"\x07");
    let _ = writer.write_all(&buf);
    let _ = writer.flush();
}

fn update_to(app: &super::App, writer: &mut impl Write) {
    write_osc2_title(&title(app), writer);
}

fn restore_to(cwd: &str, writer: &mut impl Write) {
    write_osc2_title(folder_name(cwd), writer);
}

/// Update the terminal tab title to reflect the current app status.
///
/// Called every frame tick during animating states, and on state transitions
/// for static states (Ready, Error).
pub fn update_tab_title(app: &super::App) {
    update_to(app, &mut std::io::stdout().lock());
}

fn title(app: &super::App) -> String {
    let name = folder_name(&app.cwd_raw);
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
    restore_to(cwd, &mut std::io::stdout().lock());
}

#[cfg(test)]
#[path = "tab_title_audit_tests.rs"]
mod audit_tests;
