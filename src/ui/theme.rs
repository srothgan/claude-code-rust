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

use ratatui::style::Color;

// Accent
pub const RUST_ORANGE: Color = Color::Rgb(244, 118, 0);

// UI chrome
pub const DIM: Color = Color::DarkGray;
pub const PROMPT_CHAR: &str = "\u{276f}";
pub const SEPARATOR_CHAR: &str = "\u{2500}";

// Role header colors
pub const ROLE_ASSISTANT: Color = RUST_ORANGE;

// User message background
pub const USER_MSG_BG: Color = Color::Rgb(40, 44, 52);

// Tool status icons
pub const ICON_COMPLETED: &str = "\u{2713}";
pub const ICON_FAILED: &str = "\u{2717}";

// Status colors
pub const STATUS_ERROR: Color = Color::Red;
pub const SLASH_COMMAND: Color = Color::LightMagenta;

/// Tool kind icon + label pair. Monochrome Unicode symbols.
/// If `claude_tool_name` is provided, override icon/label for specific tools.
pub fn tool_kind_label(
    kind: crate::agent::protocol::ToolKind,
    claude_tool_name: Option<&str>,
) -> (&'static str, &'static str) {
    use crate::agent::protocol::ToolKind;

    // Override for specific Claude Code tool names.
    // TODO(ui): Evaluate removing claude_tool_name label overrides and using ToolKind labels only.
    if let Some(name) = claude_tool_name {
        match name {
            "Task" => return ("\u{25c7}", "Agent"),
            "WebSearch" => return ("\u{2295}", "WebSearch"),
            "WebFetch" => return ("\u{2295}", "WebFetch"),
            _ => {}
        }
    }

    match kind {
        ToolKind::Read => ("\u{2b1a}", "Read"),
        ToolKind::Edit => ("\u{25a3}", "Edit"),
        ToolKind::Delete => ("\u{25a3}", "Delete"),
        ToolKind::Move => ("\u{21c4}", "Move"),
        ToolKind::Search => ("\u{2315}", "Find"),
        ToolKind::Execute => ("\u{27e9}", "Bash"),
        ToolKind::Think => ("\u{2756}", "Think"),
        ToolKind::Fetch => ("\u{2295}", "Fetch"),
        ToolKind::SwitchMode => ("\u{2299}", "Mode"),
        ToolKind::Other => ("\u{25cb}", "Tool"),
    }
}
