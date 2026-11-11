// SPDX-License-Identifier: Apache-2.0

use ratatui::style::Color;

// Accent
pub const RUST_ORANGE: Color = Color::Rgb(244, 118, 0);
pub const BTW_ACCENT: Color = Color::LightCyan;
pub const ULTRACODE_ACCENT: Color = Color::Rgb(180, 140, 255);

// UI chrome
pub const DIM: Color = Color::DarkGray;
pub const PROMPT_CHAR: &str = "\u{276f}";
pub const ASSISTANT_MESSAGE_MARKER: &str = "\u{25cf}";
pub const SEPARATOR_CHAR: &str = "\u{2500}";

// Role header colors
pub const ROLE_ASSISTANT: Color = RUST_ORANGE;

// User message background
pub const USER_MSG_BG: Color = Color::Rgb(40, 44, 52);

// Tool status icons
pub const ICON_COMPLETED: &str = "\u{2713}";
pub const ICON_DETACHED: &str = "\u{2197}";
pub const ICON_FAILED: &str = "\u{2717}";

// Status colors
pub const STATUS_ERROR: Color = Color::Red;
pub const STATUS_WARNING: Color = Color::Yellow;
pub const SLASH_COMMAND: Color = Color::LightMagenta;
pub const SUBAGENT_TOKEN: Color = Color::LightBlue;

/// SDK tool icon + label pair. Monochrome Unicode symbols.
/// Unknown tool names fall back to a generic Tool label.
pub fn tool_name_label(sdk_tool_name: &str) -> (&'static str, &'static str) {
    match sdk_tool_name {
        "Read" => ("\u{2b1a}", "Read"),
        "Write" => ("\u{25a3}", "Write"),
        "Edit" => ("\u{25a3}", "Edit"),
        "MultiEdit" => ("\u{25a3}", "MultiEdit"),
        "NotebookEdit" => ("\u{25a3}", "NotebookEdit"),
        "Delete" => ("\u{25a3}", "Delete"),
        "Move" => ("\u{21c4}", "Move"),
        "Glob" => ("\u{2315}", "Glob"),
        "Grep" => ("\u{2315}", "Grep"),
        "LS" => ("\u{2315}", "LS"),
        "Bash" => ("$ \u{27e9}", "Bash"),
        "PowerShell" => ("PS \u{27e9}", "PowerShell"),
        name if crate::agent::model::is_agent_tool_name(name) => ("\u{25c7}", "Subagent"),
        "TaskCreate" | "TaskUpdate" | "TaskGet" | "TaskList" | "TaskOutput" | "TaskStop" => {
            ("\u{25a1}", "Task")
        }
        "WebFetch" => ("\u{2295}", "WebFetch"),
        "WebSearch" => ("\u{2295}", "WebSearch"),
        "EnterPlanMode" => ("\u{2299}", "EnterPlanMode"),
        "ExitPlanMode" => ("\u{2299}", "ExitPlanMode"),
        "Config" => ("\u{2299}", "Config"),
        "CronCreate" | "CronDelete" | "CronList" => ("\u{25f7}", "Cron"),
        "ScheduleWakeup" => ("\u{25f7}", "Wakeup"),
        "PushNotification" => ("!", "Notify"),
        "RemoteTrigger" => ("\u{21c4}", "Remote"),
        "REPL" => ("\u{03bb}", "REPL"),
        "Monitor" => ("\u{25c9}", "Monitor"),
        "Workflow" => ("\u{25c7}", "Workflow"),
        "Projects" => ("\u{25a6}", "Projects"),
        "Artifact" => ("\u{25c8}", "Artifact"),
        "ShowOnboardingRolePicker" => ("\u{2299}", "Role"),
        "Skill" => ("\u{2726}", "Skill"),
        "EnterWorktree" => ("\u{21c4}", "EnterWorktree"),
        "ExitWorktree" => ("\u{21c4}", "ExitWorktree"),
        _ => ("\u{25cb}", "Tool"),
    }
}
