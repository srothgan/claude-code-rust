// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

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
        "Bash" => ("\u{27e9}", "Bash"),
        "Task" | "Agent" => ("\u{25c7}", "Subagent"),
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
        "EnterWorktree" => ("\u{21c4}", "EnterWorktree"),
        "ExitWorktree" => ("\u{21c4}", "ExitWorktree"),
        _ => ("\u{25cb}", "Tool"),
    }
}

#[cfg(test)]
mod tests {
    use super::tool_name_label;

    #[test]
    fn task_and_agent_share_subagent_label_and_icon() {
        assert_eq!(tool_name_label("Task"), ("\u{25c7}", "Subagent"));
        assert_eq!(tool_name_label("Agent"), ("\u{25c7}", "Subagent"));
    }

    #[test]
    fn sdk_task_state_tools_use_neutral_task_label() {
        for sdk_tool_name in
            ["TaskCreate", "TaskUpdate", "TaskGet", "TaskList", "TaskOutput", "TaskStop"]
        {
            assert_eq!(tool_name_label(sdk_tool_name), ("\u{25a1}", "Task"));
        }
    }

    #[test]
    fn worktree_tools_use_worktree_labels_and_icon() {
        assert_eq!(tool_name_label("EnterWorktree"), ("\u{21c4}", "EnterWorktree"));
        assert_eq!(tool_name_label("ExitWorktree"), ("\u{21c4}", "ExitWorktree"));
    }

    #[test]
    fn cron_tools_use_cron_labels_and_icon() {
        for sdk_tool_name in ["CronCreate", "CronDelete", "CronList"] {
            assert_eq!(tool_name_label(sdk_tool_name), ("\u{25f7}", "Cron"));
        }
    }

    #[test]
    fn schedule_wakeup_tool_uses_wakeup_label_and_icon() {
        assert_eq!(tool_name_label("ScheduleWakeup"), ("\u{25f7}", "Wakeup"));
    }

    #[test]
    fn push_notification_tool_uses_notify_label_and_icon() {
        assert_eq!(tool_name_label("PushNotification"), ("!", "Notify"));
    }

    #[test]
    fn remote_trigger_tool_uses_remote_label_and_icon() {
        assert_eq!(tool_name_label("RemoteTrigger"), ("\u{21c4}", "Remote"));
    }

    #[test]
    fn repl_tool_uses_lambda_label_and_icon() {
        assert_eq!(tool_name_label("REPL"), ("\u{03bb}", "REPL"));
    }

    #[test]
    fn monitor_and_workflow_tools_use_background_task_labels_and_icons() {
        assert_eq!(tool_name_label("Monitor"), ("\u{25c9}", "Monitor"));
        assert_eq!(tool_name_label("Workflow"), ("\u{25c7}", "Workflow"));
    }

    #[test]
    fn project_artifact_and_role_tools_use_specific_labels_and_icons() {
        assert_eq!(tool_name_label("Projects"), ("\u{25a6}", "Projects"));
        assert_eq!(tool_name_label("Artifact"), ("\u{25c8}", "Artifact"));
        assert_eq!(tool_name_label("ShowOnboardingRolePicker"), ("\u{2299}", "Role"));
    }

    #[test]
    fn plan_mode_tools_use_plan_mode_labels_and_icon() {
        assert_eq!(tool_name_label("EnterPlanMode"), ("\u{2299}", "EnterPlanMode"));
        assert_eq!(tool_name_label("ExitPlanMode"), ("\u{2299}", "ExitPlanMode"));
    }

    #[test]
    fn unknown_tools_use_generic_fallback() {
        assert_eq!(tool_name_label("UnknownFutureTool"), ("\u{25cb}", "Tool"));
    }
}
