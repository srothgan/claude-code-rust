// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::app::{App, ExtraUsage, UsageSnapshot, UsageSourceKind, UsageWindow};
use serde_json::json;
use std::time::SystemTime;

// Re-import submodule items needed by tests
use super::candidates::{
    argument_candidates, detect_slash_at_cursor, supported_command_candidates,
};

fn session_update(update: model::SessionUpdate) -> crate::agent::events::ClientEvent {
    crate::agent::events::ClientEvent::SessionUpdate { session_id: "sess-1".to_owned(), update }
}

#[test]
fn parse_non_slash_returns_none() {
    assert!(parse("hello world").is_none());
}

#[test]
fn parse_slash_name_and_args() {
    let parsed = parse("/mode plan").expect("slash command");
    assert_eq!(parsed.name, "/mode");
    assert_eq!(parsed.args, vec!["plan"]);
}

#[test]
fn unsupported_command_is_handled_locally() {
    let mut app = App::test_default();
    let consumed = try_handle_submit(&mut app, "/definitely-unknown");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
}

#[test]
fn advertised_command_is_forwarded() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/remote-command", "Remote command")];
    let consumed = try_handle_submit(&mut app, "/remote-command");
    assert!(!consumed);
}

#[test]
fn login_logout_appear_in_candidates_as_builtins() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/1m-context"), "missing /1m-context");
    assert!(names.iter().any(|n| n == "/agent"), "missing /agent");
    assert!(names.iter().any(|n| n == "/config"), "missing /config");
    assert!(names.iter().any(|n| n == "/limits"), "missing /limits");
    assert!(names.iter().any(|n| n == "/docs"), "missing /docs");
    assert!(names.iter().any(|n| n == "/login"), "missing /login");
    assert!(names.iter().any(|n| n == "/logout"), "missing /logout");
    assert!(names.iter().any(|n| n == "/mcp"), "missing /mcp");
    assert!(names.iter().any(|n| n == "/opus-version"), "missing /opus-version");
    assert!(names.iter().any(|n| n == "/plugins"), "missing /plugins");
    assert!(names.iter().any(|n| n == "/rewind"), "missing /rewind");
    assert!(names.iter().any(|n| n == "/usage"), "missing /usage");
}

#[test]
fn app_slash_catalog_roundtrips_command_names() {
    for spec in APP_SLASH_COMMANDS {
        assert_eq!(AppSlashCommand::from_name(spec.name), Some(spec.command));
        assert_eq!(spec.command.name(), spec.name);
    }
}

/// Collect the first column of the App-Owned Commands table in the manual.
///
/// The header and separator rows carry no backticked cell, so they are skipped.
fn documented_app_slash_commands(markdown: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_table_section = false;

    for line in markdown.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            in_table_section = heading == "App-Owned Commands";
            continue;
        }
        if !in_table_section || !line.starts_with('|') {
            continue;
        }
        let Some(cell) = line.split('|').nth(1) else {
            continue;
        };
        if let Some(name) = cell.trim().strip_prefix('`').and_then(|inner| inner.strip_suffix('`'))
        {
            names.push(name.to_owned());
        }
    }

    names
}

/// The manual is read from disk rather than with `include_str!` so docs are not
/// embedded in the shipped binary. Only names and order are enforced; prose in
/// the other columns is free to diverge.
#[test]
fn docs_app_owned_command_table_matches_catalog() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/commands.md");
    let markdown = std::fs::read_to_string(path).expect("read docs/src/commands.md");

    let documented = documented_app_slash_commands(&markdown);
    let documented: Vec<&str> = documented.iter().map(String::as_str).collect();
    let expected: Vec<&str> = APP_SLASH_COMMANDS.iter().map(|spec| spec.name).collect();

    for name in &expected {
        assert!(
            documented.contains(name),
            "{name} is in APP_SLASH_COMMANDS but missing from the App-Owned Commands table in docs/src/commands.md"
        );
    }
    for name in &documented {
        assert!(
            expected.contains(name),
            "{name} is listed in the App-Owned Commands table in docs/src/commands.md but is not in APP_SLASH_COMMANDS"
        );
    }
    assert_eq!(
        documented, expected,
        "the App-Owned Commands table in docs/src/commands.md must list commands in APP_SLASH_COMMANDS order"
    );
}

#[test]
fn config_without_args_opens_settings_view() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/config");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
}

#[test]
fn app_config_shadows_advertised_config_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/config", "SDK config command").input_hint("<setting>")];

    let consumed = try_handle_submit(&mut app, "/config");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
}

#[test]
fn app_config_candidate_ignores_advertised_config_metadata() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/config", "SDK config command").input_hint("<setting>")];
    app.input.set_text("/config");
    let _ = app.input.set_cursor(0, "/config".chars().count());

    let slash = super::candidates::build_slash_state(&app).expect("slash state");
    let config_candidates: Vec<_> =
        slash.candidates.iter().filter(|candidate| candidate.primary == "/config").collect();

    assert_eq!(config_candidates.len(), 1);
    assert_eq!(config_candidates[0].secondary.as_deref(), Some("Open settings"));
}

#[test]
fn app_config_does_not_enter_advertised_argument_mode() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![model::AvailableCommand::new("/config", "SDK config command").input_hint("<setting>")];
    app.input.set_text("/config ");
    let _ = app.input.set_cursor(0, "/config ".chars().count());

    assert!(super::candidates::build_slash_state(&app).is_none());
}

#[test]
fn help_without_args_opens_help_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/help");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Help);
}

#[test]
fn config_with_extra_args_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/config extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /config");
}

#[test]
fn one_m_context_disable_persists_folder_local_override_and_hints_new_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/1m-context disable");

    assert!(consumed);
    let settings_path = dir.path().join(".claude").join("settings.local.json");
    let raw = std::fs::read_to_string(settings_path).expect("read settings.local.json");
    assert!(raw.contains("\"CLAUDE_CODE_DISABLE_1M_CONTEXT\": \"1\""));
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected success message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Disabled 1M context"));
    assert!(block.text.contains("/new-session"));
}

#[test]
fn one_m_context_enable_removes_folder_local_override_and_hints_new_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_settings = dir.path().join(".claude").join("settings.local.json");
    std::fs::create_dir_all(local_settings.parent().expect("settings parent")).expect("create dir");
    std::fs::write(
            &local_settings,
            "{\n  \"env\": {\n    \"CLAUDE_CODE_DISABLE_1M_CONTEXT\": \"1\",\n    \"KEEP_ME\": \"yes\"\n  }\n}\n",
        )
        .expect("write settings");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/1m-context enable");

    assert!(consumed);
    let raw = std::fs::read_to_string(local_settings).expect("read settings.local.json");
    assert!(!raw.contains("CLAUDE_CODE_DISABLE_1M_CONTEXT"));
    assert!(raw.contains("\"KEEP_ME\": \"yes\""));
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected success message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Enabled 1M context"));
    assert!(block.text.contains("/new-session"));
}

#[test]
fn one_m_context_status_reports_disabled_folder_local_override() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_settings = dir.path().join(".claude").join("settings.local.json");
    std::fs::create_dir_all(local_settings.parent().expect("settings parent")).expect("create dir");
    std::fs::write(
        &local_settings,
        "{\n  \"env\": {\n    \"CLAUDE_CODE_DISABLE_1M_CONTEXT\": \"1\"\n  }\n}\n",
    )
    .expect("write settings");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/1m-context status");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected status message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("1M context is disabled"));
    assert!(block.text.contains(".claude/settings.local.json"));
}

#[test]
fn opus_version_argument_candidates_are_static() {
    let app = App::test_default();
    let candidates = argument_candidates(&app, "/opus-version", 0);
    assert!(candidates.iter().any(|c| c.insert_value == "4.5"));
    assert!(candidates.iter().any(|c| {
        c.insert_value == "4.5"
            && c.primary == "4.5"
            && c.secondary.as_deref() == Some("Claude Opus 4.5")
    }));
    assert!(candidates.iter().any(|c| c.insert_value == "4.6"));
    assert!(candidates.iter().any(|c| c.insert_value == "4.7"));
    assert!(candidates.iter().any(|c| {
        c.insert_value == "4.8"
            && c.primary == "4.8"
            && c.secondary.as_deref() == Some("Claude Opus 4.8")
    }));
    assert!(candidates.iter().any(|c| {
        c.insert_value == "default"
            && c.primary == "default"
            && c.secondary.as_deref() == Some("Use Claude default Opus alias")
    }));
    assert!(candidates.iter().any(|c| {
        c.insert_value == "status"
            && c.primary == "status"
            && c.secondary.as_deref() == Some("Show current project-local Opus pin")
    }));
}

#[test]
fn opus_version_45_persists_folder_local_override_and_hints_new_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/opus-version 4.5");

    assert!(consumed);
    let settings_path = dir.path().join(".claude").join("settings.local.json");
    let raw = std::fs::read_to_string(settings_path).expect("read settings.local.json");
    assert!(raw.contains("\"ANTHROPIC_DEFAULT_OPUS_MODEL\": \"claude-opus-4-5-20251101\""));
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected success message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Pinned Opus to 4.5"));
    assert!(block.text.contains("/new-session"));
}

#[test]
fn opus_version_46_persists_folder_local_override() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/opus-version 4.6");

    assert!(consumed);
    let settings_path = dir.path().join(".claude").join("settings.local.json");
    let raw = std::fs::read_to_string(settings_path).expect("read settings.local.json");
    assert!(raw.contains("\"ANTHROPIC_DEFAULT_OPUS_MODEL\": \"claude-opus-4-6\""));
}

#[test]
fn opus_version_47_persists_folder_local_override() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/opus-version 4.7");

    assert!(consumed);
    let settings_path = dir.path().join(".claude").join("settings.local.json");
    let raw = std::fs::read_to_string(settings_path).expect("read settings.local.json");
    assert!(raw.contains("\"ANTHROPIC_DEFAULT_OPUS_MODEL\": \"claude-opus-4-7\""));
}

#[test]
fn opus_version_48_persists_folder_local_override() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/opus-version 4.8");

    assert!(consumed);
    let settings_path = dir.path().join(".claude").join("settings.local.json");
    let raw = std::fs::read_to_string(settings_path).expect("read settings.local.json");
    assert!(raw.contains("\"ANTHROPIC_DEFAULT_OPUS_MODEL\": \"claude-opus-4-8\""));
}

#[test]
fn opus_version_default_removes_folder_local_override_and_preserves_neighbor_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_settings = dir.path().join(".claude").join("settings.local.json");
    std::fs::create_dir_all(local_settings.parent().expect("settings parent")).expect("create dir");
    std::fs::write(
            &local_settings,
            "{\n  \"env\": {\n    \"ANTHROPIC_DEFAULT_OPUS_MODEL\": \"claude-opus-4-7\",\n    \"KEEP_ME\": \"yes\"\n  }\n}\n",
        )
        .expect("write settings");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/opus-version default");

    assert!(consumed);
    let raw = std::fs::read_to_string(local_settings).expect("read settings.local.json");
    assert!(!raw.contains("ANTHROPIC_DEFAULT_OPUS_MODEL"));
    assert!(raw.contains("\"KEEP_ME\": \"yes\""));
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected success message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Cleared the project-local Opus version pin"));
    assert!(block.text.contains("/new-session"));
}

#[test]
fn opus_version_status_reports_known_folder_local_override() {
    let dir = tempfile::tempdir().expect("tempdir");
    let local_settings = dir.path().join(".claude").join("settings.local.json");
    std::fs::create_dir_all(local_settings.parent().expect("settings parent")).expect("create dir");
    std::fs::write(
        &local_settings,
        "{\n  \"env\": {\n    \"ANTHROPIC_DEFAULT_OPUS_MODEL\": \"claude-opus-4-6\"\n  }\n}\n",
    )
    .expect("write settings");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/opus-version status");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected status message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Opus is pinned to 4.6"));
    assert!(block.text.contains(".claude/settings.local.json"));
}

#[test]
fn opus_version_status_reports_default_when_unset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());
    app.cwd_raw = dir.path().to_string_lossy().to_string();

    let consumed = try_handle_submit(&mut app, "/opus-version status");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected status message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Opus is using the default alias resolution"));
}

#[test]
fn opus_version_with_missing_arg_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/opus-version");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /opus-version <4.5|4.6|4.7|4.8|default|status>");
}

#[test]
fn opus_version_with_extra_args_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/opus-version 4.7 extra");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /opus-version <4.5|4.6|4.7|4.8|default|status>");
}

#[test]
fn opus_version_with_unknown_arg_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/opus-version 9.9");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /opus-version <4.5|4.6|4.7|4.8|default|status>");
}

#[test]
fn opus_version_requires_trusted_project_for_mutation() {
    let mut app = App::test_default();
    app.trust.status = crate::app::trust::TrustStatus::Untrusted;

    let consumed = try_handle_submit(&mut app, "/opus-version 4.7");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected error message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Project trust must be accepted"));
}

#[test]
fn plugins_without_args_opens_plugins_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/plugins");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Plugins);
}

#[test]
fn mcp_opens_config_at_mcp_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/mcp");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Mcp);
}

#[test]
fn mcp_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/mcp extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /mcp");
}

#[test]
fn plugins_with_extra_args_still_opens_plugins_tab() {
    let mut app = App::test_default();
    let dir = tempfile::tempdir().expect("tempdir");
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/plugins extra");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Plugins);
}

#[tokio::test(flavor = "current_thread")]
async fn login_is_handled_as_builtin_and_sets_command_pending() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let consumed = try_handle_submit(&mut app, "/login");
            assert!(consumed, "/login should be handled locally");
            // Status becomes CommandPending (or stays Ready if claude CLI is not in PATH)
            assert!(
                matches!(app.status, AppStatus::CommandPending | AppStatus::Ready),
                "expected CommandPending or Ready, got {:?}",
                app.status
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn logout_is_handled_as_builtin_and_sets_command_pending() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let consumed = try_handle_submit(&mut app, "/logout");
            assert!(consumed, "/logout should be handled locally");
            assert!(
                matches!(app.status, AppStatus::CommandPending | AppStatus::Ready),
                "expected CommandPending or Ready, got {:?}",
                app.status
            );
        })
        .await;
}

#[test]
fn login_rejects_extra_args() {
    let mut app = App::test_default();
    let consumed = try_handle_submit(&mut app, "/login somearg");
    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    assert!(matches!(last.role, MessageRole::System(_)));
}

#[test]
fn detect_slash_argument_context_after_first_space() {
    let lines = vec!["/mode pla".to_owned()];
    let detection =
        detect_slash_at_cursor(&lines, 0, "/mode pla".chars().count()).expect("slash detection");

    match detection.context {
        SlashContext::Argument { command, arg_index, token_range } => {
            assert_eq!(command, "/mode");
            assert_eq!(arg_index, 0);
            assert_eq!(token_range, (6, 9));
        }
        SlashContext::CommandName => panic!("expected argument context"),
    }
    assert_eq!(detection.query, "pla");
}

#[test]
fn mode_argument_candidates_are_dynamic() {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(super::super::ModeState {
        current_mode_id: "plan".to_owned(),
        current_mode_name: "Plan".to_owned(),
        available_modes: vec![
            super::super::ModeInfo { id: "plan".to_owned(), name: "Plan".to_owned() },
            super::super::ModeInfo { id: "code".to_owned(), name: "Code".to_owned() },
        ],
    });

    let candidates = argument_candidates(&app, "/mode", 0);
    assert!(candidates.iter().any(|c| c.insert_value == "plan"));
    assert!(candidates.iter().any(|c| c.insert_value == "code"));
    assert!(candidates.iter().any(|c| c.primary == "Plan"));
    assert!(candidates.iter().any(|c| c.secondary.as_deref() == Some("plan")));
}

#[test]
fn model_argument_candidates_are_dynamic() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("sonnet", "Claude Sonnet")
            .description("Balanced coding model"),
        crate::agent::model::AvailableModel::new("opus", "Claude Opus"),
    ];
    let candidates = argument_candidates(&app, "/model", 0);
    assert!(candidates.iter().any(|c| c.insert_value == "sonnet"));
    assert!(candidates.iter().any(|c| c.primary == "Claude Sonnet"));
    assert!(candidates.iter().any(|c| c.secondary.as_deref() == Some("Balanced coding model")));
    assert!(candidates.iter().any(|c| c.insert_value == "opus"));
}

#[test]
fn model_argument_candidates_include_sdk_default_option() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("default", "Default")
            .description("Default (recommended)"),
        crate::agent::model::AvailableModel::new("sonnet", "Claude Sonnet"),
        crate::agent::model::AvailableModel::new("opus", "Claude Opus"),
    ];

    let candidates = argument_candidates(&app, "/model", 0);

    assert!(candidates.iter().any(|c| c.insert_value == "default"));
    assert!(candidates.iter().any(|c| c.primary == "Default"));
    assert!(candidates.iter().any(|c| c.secondary.as_deref() == Some("Default (recommended)")));
    assert!(candidates.iter().any(|c| c.insert_value == "sonnet"));
    assert!(candidates.iter().any(|c| c.insert_value == "opus"));
}

#[test]
fn model_argument_candidates_rewrite_opus_secondary_from_project_pin() {
    let mut app = App::test_default();
    app.config.committed_local_settings_document = json!({
        "env": {
            "ANTHROPIC_DEFAULT_OPUS_MODEL": "claude-opus-4-5-20251101"
        }
    });
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("opus", "Opus")
            .description("Opus 4.7 · Most capable for complex work"),
    ];

    let candidates = argument_candidates(&app, "/model", 0);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].insert_value, "opus");
    assert_eq!(
        candidates[0].secondary.as_deref(),
        Some("Opus 4.5 · Most capable for complex work")
    );
}

#[test]
fn model_argument_candidates_keep_sdk_opus_description_when_unpinned() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("opus", "Opus")
            .description("Opus 4.7 · Most capable for complex work"),
    ];

    let candidates = argument_candidates(&app, "/model", 0);

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].insert_value, "opus");
    assert_eq!(
        candidates[0].secondary.as_deref(),
        Some("Opus 4.7 · Most capable for complex work")
    );
}

#[test]
fn agent_argument_candidates_include_reset_and_available_agents() {
    let mut app = App::test_default();
    app.sdk_inventory.available_agents = vec![
        crate::agent::model::AvailableAgent::new("reviewer", "Review code").model("claude-opus"),
        crate::agent::model::AvailableAgent::new("planner", "Plan work"),
    ];

    let candidates = argument_candidates(&app, "/agent", 0);

    assert!(candidates.iter().any(|candidate| {
        candidate.insert_value == "reset"
            && candidate.secondary.as_deref() == Some("Clear active agent")
    }));
    assert!(candidates.iter().any(|candidate| {
        candidate.insert_value == "reviewer"
            && candidate.primary == "reviewer"
            && candidate.secondary.as_deref() == Some("Review code - claude-opus")
    }));
    assert!(candidates.iter().any(|candidate| candidate.insert_value == "planner"));
}

#[test]
fn agent_argument_candidates_filter_by_query() {
    let mut app = App::test_default();
    app.sdk_inventory.available_agents = vec![
        crate::agent::model::AvailableAgent::new("reviewer", "Review code"),
        crate::agent::model::AvailableAgent::new("planner", "Plan work"),
    ];
    app.input.set_text("/agent rev");
    let _ = app.input.set_cursor(0, "/agent rev".chars().count());

    let slash = super::candidates::build_slash_state(&app).expect("slash state");

    assert!(matches!(slash.context, SlashContext::Argument { .. }));
    assert_eq!(
        slash
            .candidates
            .iter()
            .map(|candidate| candidate.insert_value.as_str())
            .collect::<Vec<_>>(),
        vec!["reviewer"]
    );
}

#[test]
fn rewind_argument_candidates_use_cached_targets() {
    let mut app = App::test_default();
    let session_id = model::SessionId::new("session-1");
    app.session_runtime.session_id = Some(session_id.clone());
    app.sdk_inventory.rewind_targets_session_id = Some(session_id);
    app.sdk_inventory.rewind_targets = vec![
        model::RewindTarget {
            uuid: "user-1".to_owned(),
            first_text: "first prompt".to_owned(),
            input_text: "first prompt".to_owned(),
            index: 0,
            previous_assistant_uuid: None,
        },
        model::RewindTarget {
            uuid: "user-2".to_owned(),
            first_text: "second prompt".to_owned(),
            input_text: "second prompt".to_owned(),
            index: 3,
            previous_assistant_uuid: Some("assistant-1".to_owned()),
        },
    ];
    app.input.set_text("/rewind second");
    let _ = app.input.set_cursor(0, "/rewind second".chars().count());

    let slash = super::candidates::build_slash_state(&app).expect("slash state");

    assert!(matches!(slash.context, SlashContext::Argument { .. }));
    assert_eq!(
        slash
            .candidates
            .iter()
            .map(|candidate| (
                candidate.insert_value.as_str(),
                candidate.primary.as_str(),
                candidate.secondary.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![("user-2", "second prompt", Some("user-2"))]
    );
}

#[test]
fn rewind_argument_candidates_hide_stale_targets() {
    let mut app = App::test_default();
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.sdk_inventory.rewind_targets_session_id = Some(model::SessionId::new("old-session"));
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
    }];

    let candidates = argument_candidates(&app, "/rewind", 0);

    assert!(candidates.is_empty());
}

#[test]
fn rewind_argument_context_requests_targets_when_cache_is_stale() {
    let mut app = App::test_default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn =
        Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.input.set_text("/rewind ");
    let _ = app.input.set_cursor(0, "/rewind ".chars().count());

    sync_with_cursor(&mut app);

    assert!(app.sdk_inventory.rewind_targets_in_flight);
    let envelope = rx.try_recv().expect("rewind target request");
    assert!(matches!(
        envelope.command,
        crate::agent::wire::BridgeCommand::GetRewindTargets { session_id }
            if session_id == "session-1"
    ));
}

#[test]
fn rewind_argument_context_shows_loading_while_request_is_in_flight() {
    let mut app = App::test_default();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn =
        Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.input.set_text("/rewind ");
    let _ = app.input.set_cursor(0, "/rewind ".chars().count());

    sync_with_cursor(&mut app);

    let slash = app.slash.as_ref().expect("slash state");
    assert!(slash.candidates.is_empty());
    assert_eq!(slash.placeholder.as_deref(), Some("Loading messages"));
}

#[test]
fn rewind_argument_context_shows_no_previous_messages_when_loaded_empty() {
    let mut app = App::test_default();
    let session_id = model::SessionId::new("session-1");
    app.session_runtime.session_id = Some(session_id.clone());
    app.sdk_inventory.rewind_targets_session_id = Some(session_id);
    app.input.set_text("/rewind ");
    let _ = app.input.set_cursor(0, "/rewind ".chars().count());

    let slash = super::candidates::build_slash_state(&app).expect("slash state");

    assert!(slash.candidates.is_empty());
    assert_eq!(slash.placeholder.as_deref(), Some("No previous user messages"));
}

#[test]
fn rewind_argument_context_shows_no_matching_messages_for_filtered_empty_result() {
    let mut app = App::test_default();
    let session_id = model::SessionId::new("session-1");
    app.session_runtime.session_id = Some(session_id.clone());
    app.sdk_inventory.rewind_targets_session_id = Some(session_id);
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
    }];
    app.input.set_text("/rewind missing");
    let _ = app.input.set_cursor(0, "/rewind missing".chars().count());

    let slash = super::candidates::build_slash_state(&app).expect("slash state");

    assert!(slash.candidates.is_empty());
    assert_eq!(slash.placeholder.as_deref(), Some("No matching messages"));
}

#[test]
fn effort_argument_candidates_include_session_only_max() {
    let mut app = App::test_default();
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
            .supports_effort(true)
            .supported_effort_levels(vec![
                crate::agent::model::EffortLevel::Low,
                crate::agent::model::EffortLevel::Medium,
                crate::agent::model::EffortLevel::High,
                crate::agent::model::EffortLevel::XHigh,
            ]),
    );

    let candidates = argument_candidates(&app, "/effort", 0);

    assert_eq!(
        candidates.iter().map(|candidate| candidate.insert_value.as_str()).collect::<Vec<_>>(),
        vec!["low", "medium", "high", "xhigh", "max"]
    );
    assert!(candidates.iter().any(|candidate| {
        candidate.insert_value == "max"
            && candidate.secondary.as_deref() == Some("Max - Maximum effort")
    }));
}

#[test]
fn effort_argument_candidates_filter_by_query() {
    let mut app = App::test_default();
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
            .supports_effort(true)
            .supported_effort_levels(crate::agent::model::EffortLevel::ALL.to_vec()),
    );
    app.input.set_text("/effort xh");
    let _ = app.input.set_cursor(0, "/effort xh".chars().count());

    let slash = super::candidates::build_slash_state(&app).expect("slash state");

    assert!(matches!(slash.context, SlashContext::Argument { .. }));
    assert_eq!(
        slash
            .candidates
            .iter()
            .map(|candidate| candidate.insert_value.as_str())
            .collect::<Vec<_>>(),
        vec!["xhigh"]
    );
}

#[test]
fn docs_argument_candidates_are_static_topics() {
    let app = App::test_default();
    let candidates = argument_candidates(&app, "/docs", 0);
    assert!(candidates.iter().any(|c| c.insert_value == "mode"));
    assert!(candidates.iter().any(|c| c.insert_value == "models"));
    assert!(candidates.iter().any(|c| c.insert_value == "shortcuts"));
    assert!(candidates.iter().any(|c| c.insert_value == "commands"));
    assert!(candidates.iter().any(|c| c.insert_value == "agents"));
}

#[test]
fn docs_without_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /docs <mode|models|shortcuts|commands|agents>");
}

#[test]
fn docs_models_show_advertised_effort_levels() {
    let mut app = App::test_default();
    app.sdk_inventory.available_models = vec![
        crate::agent::model::AvailableModel::new("sonnet", "Claude Sonnet")
            .description("Balanced model")
            .supports_effort(true)
            .supported_effort_levels(vec![
                crate::agent::model::EffortLevel::Low,
                crate::agent::model::EffortLevel::Medium,
                crate::agent::model::EffortLevel::High,
                crate::agent::model::EffortLevel::XHigh,
                crate::agent::model::EffortLevel::Max,
            ])
            .supports_fast_mode(Some(true)),
    ];

    let consumed = try_handle_submit(&mut app, "/docs models");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Docs: Models"));
    assert!(block.text.contains("Effort: Low, Medium, High, XHigh, Max"));
    assert!(block.text.contains("Fast mode"));
}

#[test]
fn docs_commands_reuse_help_rows() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands =
        vec![crate::agent::model::AvailableCommand::new("/help", "Open help")];

    let consumed = try_handle_submit(&mut app, "/docs commands");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| Command | Description |"));
    assert!(block.text.contains("/1m-context"));
    assert!(block.text.contains("project-local 1M context"));
    assert!(block.text.contains("/cancel"));
    assert!(block.text.contains("/compact"));
    assert!(block.text.contains("/config"));
    assert!(block.text.contains("/docs"));
    assert!(block.text.contains("/help"));
    assert!(block.text.contains("/mode"));
    assert!(block.text.contains("/model"));
    assert!(block.text.contains("/new-session"));
    assert!(block.text.contains("/resume"));
    assert!(block.text.contains("/rewind"));
}

#[test]
fn docs_commands_do_not_show_advertised_command_shadowed_by_app_command() {
    let mut app = App::test_default();
    app.sdk_inventory.available_commands = vec![
        crate::agent::model::AvailableCommand::new("/config", "SDK config command")
            .input_hint("<setting>"),
    ];

    let consumed = try_handle_submit(&mut app, "/docs commands");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| /config | Open the fullscreen settings tab. |"));
    assert!(!block.text.contains("SDK config command"));
}

#[test]
fn docs_shortcuts_use_live_help_state() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs shortcuts");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| Shortcut | Action |"));
    assert!(block.text.contains("Send message"));
    assert!(!block.text.contains("Toggle todo"));
}

#[test]
fn docs_with_unknown_topic_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs nope");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("Unknown docs topic: nope"));
    assert!(block.text.contains("Usage: /docs <mode|models|shortcuts|commands|agents>"));
}

#[test]
fn docs_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/docs commands extra");

    assert!(consumed);
    let last = app.transcript.messages.last().expect("expected system message");
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /docs <mode|models|shortcuts|commands|agents>");
}

#[test]
fn non_variable_command_argument_mode_is_disabled() {
    let mut app = App::test_default();
    app.input.set_text("/cancel now");
    let _ = app.input.set_cursor(0, "/cancel now".chars().count());
    sync_with_cursor(&mut app);
    assert!(app.slash.is_none());
}

#[test]
fn variable_command_argument_mode_stays_active_without_matches() {
    let mut app = App::test_default();
    app.session_runtime.mode = Some(super::super::ModeState {
        current_mode_id: "plan".to_owned(),
        current_mode_name: "Plan".to_owned(),
        available_modes: vec![super::super::ModeInfo {
            id: "plan".to_owned(),
            name: "Plan".to_owned(),
        }],
    });
    app.input.set_text("/mode xyz");
    let _ = app.input.set_cursor(0, "/mode xyz".chars().count());
    sync_with_cursor(&mut app);
    let slash = app.slash.as_ref().expect("slash state should stay active for empty result hint");
    assert!(slash.candidates.is_empty());
}

#[test]
fn confirm_selection_replaces_only_active_argument_token() {
    let mut app = App::test_default();
    app.input.set_text("/resume old-id trailing");
    let _ = app.input.set_cursor(0, "/resume old-id".chars().count());
    app.slash = Some(SlashState {
        trigger_row: 0,
        trigger_col: 8,
        query: "old-id".to_owned(),
        context: SlashContext::Argument {
            command: "/resume".to_owned(),
            arg_index: 0,
            token_range: (8, 14),
        },
        candidates: vec![SlashCandidate {
            insert_value: "new-id".to_owned(),
            primary: "New".to_owned(),
            secondary: None,
        }],
        placeholder: None,
        dialog: DialogState::default(),
    });

    confirm_selection(&mut app);

    assert_eq!(app.input.text(), "/resume new-id trailing");
}

#[tokio::test(flavor = "current_thread")]
async fn login_is_handled_as_builtin_even_when_advertised() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            app.sdk_inventory.available_commands =
                vec![model::AvailableCommand::new("/login", "Login")];

            let consumed = try_handle_submit(&mut app, "/login");
            assert!(consumed, "/login should be handled locally even when SDK advertises it");
        })
        .await;
}

#[test]
fn new_session_command_is_rendered_as_user_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/new-session");
    assert!(consumed);
    assert!(app.transcript.messages.len() >= 2);

    let Some(first) = app.transcript.messages.first() else {
        panic!("expected first message");
    };
    assert!(matches!(first.role, MessageRole::User));
    let Some(MessageBlock::Text(block)) = first.blocks.first() else {
        panic!("expected user text block");
    };
    assert_eq!(block.text, "/new-session");
}

#[test]
fn resume_with_missing_id_returns_usage() {
    let mut app = App::test_default();
    let consumed = try_handle_submit(&mut app, "/resume");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /resume <session_id>");
}

#[test]
fn resume_with_extra_args_returns_usage() {
    let mut app = App::test_default();
    let consumed = try_handle_submit(&mut app, "/resume abc-123 extra");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /resume <session_id>");
}

#[test]
fn rewind_with_missing_target_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/rewind");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /rewind <user_message_uuid> <both|conversation|code>");
}

#[test]
fn rewind_with_cached_target_requires_connection() {
    let mut app = App::test_default();
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
    }];

    let consumed = try_handle_submit(&mut app, "/rewind user-1 conversation");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected selection message");
    };
    assert!(matches!(last.role, MessageRole::System(Some(SystemSeverity::Error))));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Cannot rewind: not connected yet.");
}

#[test]
fn rewind_with_cached_target_sends_bridge_command() {
    let mut app = App::test_default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn =
        Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));
    app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
        uuid: "user-1".to_owned(),
        first_text: "first prompt".to_owned(),
        input_text: "first prompt".to_owned(),
        index: 0,
        previous_assistant_uuid: None,
    }];

    let consumed = try_handle_submit(&mut app, "/rewind user-1 conversation");

    assert!(consumed);
    assert_eq!(app.turn.pending_command_label.as_deref(), Some("Rewinding conversation..."));
    let envelope = rx.try_recv().expect("rewind command");
    let crate::agent::wire::BridgeCommand::Rewind {
        session_id,
        target_user_message_id,
        restore_mode,
        ..
    } = envelope.command
    else {
        panic!("expected rewind command");
    };
    assert_eq!(session_id, "session-1");
    assert_eq!(target_user_message_id, "user-1");
    assert_eq!(restore_mode, crate::agent::types::RewindRestoreMode::Conversation);
}

#[test]
fn resume_command_is_rendered_as_user_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/resume abc-123");
    assert!(consumed);
    assert!(app.transcript.messages.len() >= 2);

    let Some(first) = app.transcript.messages.first() else {
        panic!("expected user message");
    };
    assert!(matches!(first.role, MessageRole::User));
    let Some(MessageBlock::Text(block)) = first.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "/resume abc-123");
}

#[tokio::test(flavor = "current_thread")]
async fn resume_sets_command_pending_when_connected() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));

            let consumed = try_handle_submit(&mut app, "/resume abc-123");
            assert!(consumed);
            assert!(matches!(app.status, AppStatus::CommandPending));
            assert_eq!(app.resuming_session_id.as_deref(), Some("abc-123"));

            tokio::task::yield_now().await;
            assert!(rx.try_recv().is_ok());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn mode_sets_command_pending_and_mode_update_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.mode = Some(super::super::ModeState {
                current_mode_id: "code".to_owned(),
                current_mode_name: "Code".to_owned(),
                available_modes: vec![
                    super::super::ModeInfo { id: "plan".to_owned(), name: "Plan".to_owned() },
                    super::super::ModeInfo { id: "code".to_owned(), name: "Code".to_owned() },
                ],
            });

            let consumed = try_handle_submit(&mut app, "/mode plan");
            assert!(consumed);
            assert!(
                matches!(app.status, AppStatus::CommandPending),
                "expected CommandPending, got {:?}",
                app.status
            );
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching mode..."));

            // Simulate mode-update ack arriving from bridge.
            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::CurrentModeUpdate(
                    crate::agent::model::CurrentModeUpdate::new("plan"),
                )),
            );
            assert!(
                matches!(app.status, AppStatus::Ready),
                "expected Ready after CurrentModeUpdate ack, got {:?}",
                app.status
            );
            assert!(app.turn.pending_command_label.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn model_sets_command_pending_and_current_model_ack_updates_model_and_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("old-model", "old-model", "old-model")
                    .authoritative(true),
            );

            let consumed = try_handle_submit(&mut app, "/model sonnet");
            assert!(consumed);
            assert!(
                matches!(app.status, AppStatus::CommandPending),
                "expected CommandPending, got {:?}",
                app.status
            );
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching model..."));
            assert_eq!(
                app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
                Some("old-model")
            );

            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::CurrentModelUpdate(
                    crate::agent::model::CurrentModelUpdate::new(
                        crate::agent::model::CurrentModel::new("sonnet", "sonnet", "sonnet")
                            .authoritative(true),
                    ),
                )),
            );
            assert!(
                matches!(app.status, AppStatus::Ready),
                "expected Ready after current model ack, got {:?}",
                app.status
            );
            assert_eq!(
                app.session_runtime.current_model.as_ref().map(|model| model.resolved_id.as_str()),
                Some("sonnet")
            );
            assert!(app.turn.pending_command_label.is_none());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_sets_command_pending_and_config_option_ack_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
                    .supports_effort(true)
                    .supported_effort_levels(crate::agent::model::EffortLevel::ALL.to_vec()),
            );

            let consumed = try_handle_submit(&mut app, "/effort xhigh");
            assert!(consumed);
            assert!(matches!(app.status, AppStatus::CommandPending));
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching effort..."));
            assert!(matches!(
                app.turn.pending_command_ack.as_ref(),
                Some(super::super::PendingCommandAck::ConfigOption { option_id })
                    if option_id == "effortLevel"
            ));

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set effort command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetEffort {
                    session_id: "sess-1".to_owned(),
                    effort: "xhigh".to_owned(),
                }
            );

            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::ConfigOptionUpdate(
                    crate::agent::model::ConfigOptionUpdate {
                        option_id: "effortLevel".to_owned(),
                        value: serde_json::json!("xhigh"),
                    },
                )),
            );
            assert!(matches!(app.status, AppStatus::Ready));
            assert_eq!(
                app.session_runtime.config_options.get("effortLevel"),
                Some(&serde_json::json!("xhigh"))
            );
            assert_eq!(
                app.session_thinking_effort_effective(),
                crate::agent::model::EffortLevel::XHigh
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn effort_accepts_session_only_max() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
            app.session_runtime.session_id = Some("sess-1".into());
            app.session_runtime.current_model = Some(
                crate::agent::model::CurrentModel::new("opus", "Opus", "Opus")
                    .supports_effort(true)
                    .supported_effort_levels(vec![
                        crate::agent::model::EffortLevel::Low,
                        crate::agent::model::EffortLevel::Medium,
                        crate::agent::model::EffortLevel::High,
                    ]),
            );

            let consumed = try_handle_submit(&mut app, "/effort max");
            assert!(consumed);

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set effort command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetEffort {
                    session_id: "sess-1".to_owned(),
                    effort: "max".to_owned(),
                }
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_sets_command_pending_and_config_option_ack_restores_ready() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
            app.session_runtime.session_id = Some("sess-1".into());
            app.sdk_inventory.available_agents =
                vec![crate::agent::model::AvailableAgent::new("reviewer", "Review code")];

            let consumed = try_handle_submit(&mut app, "/agent reviewer");
            assert!(consumed);
            assert!(matches!(app.status, AppStatus::CommandPending));
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Switching agent..."));
            assert!(matches!(
                app.turn.pending_command_ack.as_ref(),
                Some(super::super::PendingCommandAck::ConfigOption { option_id })
                    if option_id == "agent"
            ));

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set agent command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetAgent {
                    session_id: "sess-1".to_owned(),
                    agent: Some("reviewer".to_owned()),
                }
            );

            super::super::events::handle_client_event(
                &mut app,
                session_update(crate::agent::model::SessionUpdate::ConfigOptionUpdate(
                    crate::agent::model::ConfigOptionUpdate {
                        option_id: "agent".to_owned(),
                        value: serde_json::json!("reviewer"),
                    },
                )),
            );
            assert!(matches!(app.status, AppStatus::Ready));
            assert_eq!(
                app.session_runtime.config_options.get("agent"),
                Some(&serde_json::json!("reviewer"))
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_reset_sends_null_agent() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
            app.session_runtime.session_id = Some("sess-1".into());

            let consumed = try_handle_submit(&mut app, "/agent reset");
            assert!(consumed);

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("reset agent command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetAgent {
                    session_id: "sess-1".to_owned(),
                    agent: None,
                }
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_allows_unadvertised_name_when_agent_catalog_is_empty() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
            app.session_runtime.session_id = Some("sess-1".into());

            let consumed = try_handle_submit(&mut app, "/agent custom-agent");
            assert!(consumed);

            tokio::task::yield_now().await;
            let envelope = rx.try_recv().expect("set agent command");
            assert_eq!(
                envelope.command,
                crate::agent::wire::BridgeCommand::SetAgent {
                    session_id: "sess-1".to_owned(),
                    agent: Some("custom-agent".to_owned()),
                }
            );
        })
        .await;
}

#[test]
fn agent_rejects_unknown_when_available_agents_are_populated() {
    let mut app = App::test_default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn =
        Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_runtime.session_id = Some("sess-1".into());
    app.sdk_inventory.available_agents =
        vec![crate::agent::model::AvailableAgent::new("reviewer", "Review code")];

    let consumed = try_handle_submit(&mut app, "/agent planner");

    assert!(consumed);
    assert!(rx.try_recv().is_err());
    assert!(!matches!(app.status, AppStatus::CommandPending));
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Unknown agent: planner");
}

#[test]
fn agent_invalid_arguments_return_usage() {
    for input in ["/agent", "/agent reviewer extra"] {
        let mut app = App::test_default();

        let consumed = try_handle_submit(&mut app, input);

        assert!(consumed);
        let Some(last) = app.transcript.messages.last() else {
            panic!("expected system usage message for {input}");
        };
        let Some(MessageBlock::Text(block)) = last.blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Usage: /agent <name|reset>");
        assert!(!matches!(app.status, AppStatus::CommandPending));
    }
}

#[test]
fn effort_invalid_arguments_return_usage() {
    for input in ["/effort", "/effort banana", "/effort high extra"] {
        let mut app = App::test_default();

        let consumed = try_handle_submit(&mut app, input);

        assert!(consumed);
        let Some(last) = app.transcript.messages.last() else {
            panic!("expected system usage message for {input}");
        };
        let Some(MessageBlock::Text(block)) = last.blocks.first() else {
            panic!("expected text block");
        };
        assert_eq!(block.text, "Usage: /effort <low|medium|high|xhigh|max>");
        assert!(!matches!(app.status, AppStatus::CommandPending));
    }
}

#[test]
fn effort_rejects_models_without_effort_support() {
    let mut app = App::test_default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn =
        Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_runtime.session_id = Some("sess-1".into());
    app.session_runtime.current_model = Some(
        crate::agent::model::CurrentModel::new("haiku", "Haiku", "Haiku").supports_effort(false),
    );

    let consumed = try_handle_submit(&mut app, "/effort high");

    assert!(consumed);
    assert!(rx.try_recv().is_err());
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Cannot switch effort: current model does not support effort.");
}

#[tokio::test(flavor = "current_thread")]
async fn new_session_sets_command_pending() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let mut app = App::test_default();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            app.session_runtime.conn =
                Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));

            let consumed = try_handle_submit(&mut app, "/new-session");
            assert!(consumed);
            assert!(
                matches!(app.status, AppStatus::CommandPending),
                "expected CommandPending, got {:?}",
                app.status
            );
            assert_eq!(app.turn.pending_command_label.as_deref(), Some("Starting new session..."));
        })
        .await;
}

#[test]
fn compact_without_connection_is_handled_locally() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/compact");
    assert!(consumed);
    assert!(!app.turn.pending_compact_clear);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Cannot compact: not connected yet.");
}

#[test]
fn compact_with_active_session_sets_compacting_without_success_pending() {
    let mut app = App::test_default();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.session_runtime.conn =
        Some(std::rc::Rc::new(crate::agent::client::AgentConnection::new(tx)));
    app.session_runtime.session_id = Some(model::SessionId::new("session-1"));

    let consumed = try_handle_submit(&mut app, "/compact");
    assert!(!consumed);
    assert!(!app.turn.pending_compact_clear);
    assert!(app.turn.is_compacting);
}

#[test]
fn compact_with_args_returns_usage_message() {
    let mut app = App::test_default();
    app.transcript.messages.push(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete("keep"))],
        None,
    ));

    let consumed = try_handle_submit(&mut app, "/compact now");
    assert!(consumed);
    assert!(app.transcript.messages.len() >= 2);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /compact");
}

#[test]
fn mode_with_extra_args_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/mode plan extra");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    assert!(matches!(last.role, MessageRole::System(_)));
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /mode <id>");
}

#[test]
fn model_with_missing_id_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/model");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /model <id>");
}

#[test]
fn model_with_extra_args_returns_usage_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/model sonnet extra");
    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected system usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /model <id>");
}

#[test]
fn confirm_selection_with_invalid_trigger_row_is_noop() {
    let mut app = App::test_default();
    app.input.set_text("/mode");
    app.slash = Some(SlashState {
        trigger_row: 99,
        trigger_col: 0,
        query: "m".into(),
        context: SlashContext::CommandName,
        candidates: vec![SlashCandidate {
            insert_value: "/mode".into(),
            primary: "/mode".into(),
            secondary: None,
        }],
        placeholder: None,
        dialog: DialogState::default(),
    });

    confirm_selection(&mut app);

    assert_eq!(app.input.text(), "/mode");
}

#[test]
fn docs_command_confirm_enters_argument_mode() {
    let mut app = App::test_default();
    app.input.set_text("/do");
    let _ = app.input.set_cursor(0, "/do".chars().count());
    app.slash = Some(SlashState {
        trigger_row: 0,
        trigger_col: 0,
        query: "do".into(),
        context: SlashContext::CommandName,
        candidates: vec![SlashCandidate {
            insert_value: "/docs".into(),
            primary: "/docs".into(),
            secondary: Some("Show in-chat help topics".into()),
        }],
        placeholder: None,
        dialog: DialogState::default(),
    });

    confirm_selection(&mut app);

    assert_eq!(app.input.text(), "/docs ");
    let slash = app.slash.as_ref().expect("topic autocomplete should activate");
    match &slash.context {
        SlashContext::Argument { command, arg_index, .. } => {
            assert_eq!(command, "/docs");
            assert_eq!(*arg_index, 0);
        }
        SlashContext::CommandName => panic!("expected argument autocomplete"),
    }
    assert!(slash.candidates.iter().any(|candidate| candidate.insert_value == "mode"));
}

#[test]
fn single_argument_builtin_selection_closes_autocomplete() {
    for (command, value) in [
        ("/docs", "commands"),
        ("/agent", "reviewer"),
        ("/effort", "xhigh"),
        ("/mode", "plan"),
        ("/model", "sonnet"),
        ("/opus-version", "4.7"),
        ("/resume", "session-1"),
    ] {
        let mut app = App::test_default();
        let input = format!("{command} ");
        app.input.set_text(&input);
        let _ = app.input.set_cursor(0, input.chars().count());
        app.slash = Some(SlashState {
            trigger_row: 0,
            trigger_col: input.chars().count(),
            query: String::new(),
            context: SlashContext::Argument {
                command: command.to_owned(),
                arg_index: 0,
                token_range: (input.chars().count(), input.chars().count()),
            },
            candidates: vec![SlashCandidate {
                insert_value: value.to_owned(),
                primary: value.to_owned(),
                secondary: None,
            }],
            placeholder: None,
            dialog: DialogState::default(),
        });

        confirm_selection(&mut app);

        assert_eq!(app.input.text(), format!("{command} {value} "));
        assert!(app.slash.is_none(), "{command} should close after first argument");
    }
}

#[test]
fn status_opens_config_at_status_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/status");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Status);
}

#[test]
fn usage_opens_config_at_usage_tab() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::test_default();
    app.settings_home_override = Some(dir.path().to_path_buf());

    let consumed = try_handle_submit(&mut app, "/usage");

    assert!(consumed);
    assert_eq!(
        app.surface_mode,
        super::super::SurfaceMode::Fullscreen(super::super::FullscreenView::Config)
    );
    assert_eq!(app.config.active_tab, super::super::ConfigTab::Usage);
}

#[test]
fn limits_with_fresh_snapshot_prints_markdown_table_in_chat() {
    let mut app = App::test_default();
    app.usage.snapshot = Some(UsageSnapshot {
        source: UsageSourceKind::Oauth,
        fetched_at: SystemTime::now(),
        five_hour: Some(UsageWindow {
            label: "5-hour",
            utilization: 47.0,
            resets_at: None,
            reset_description: Some("resets in 2h 14m".to_owned()),
        }),
        seven_day: Some(UsageWindow {
            label: "7-day",
            utilization: 62.0,
            resets_at: None,
            reset_description: Some("resets in 4d 11h".to_owned()),
        }),
        seven_day_opus: None,
        seven_day_sonnet: None,
        extra_usage: Some(ExtraUsage {
            monthly_limit: Some(20.0),
            used_credits: Some(12.4),
            utilization: Some(62.0),
            currency: Some("USD".to_owned()),
        }),
    });

    let consumed = try_handle_submit(&mut app, "/limits");

    assert!(consumed);
    assert_eq!(app.surface_mode, super::super::SurfaceMode::Chat);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected limits summary");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert!(block.text.contains("| 5-hour | 47% | resets in 2h 14m |"));
    assert!(block.text.contains("| 7-day | 62% | resets in 4d 11h |"));
    assert!(block.text.contains("| USD | 12.40 / 20.00 |"));
}

#[test]
fn limits_without_fresh_snapshot_prints_loading_message() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/limits");

    assert!(consumed);
    assert!(app.usage.pending_limits_response);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected loading message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Getting recent usage info.");
}

#[test]
fn status_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/status extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /status");
}

#[test]
fn usage_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/usage extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /usage");
}

#[test]
fn limits_with_extra_args_returns_usage() {
    let mut app = App::test_default();

    let consumed = try_handle_submit(&mut app, "/limits extra");

    assert!(consumed);
    let Some(last) = app.transcript.messages.last() else {
        panic!("expected usage message");
    };
    let Some(MessageBlock::Text(block)) = last.blocks.first() else {
        panic!("expected text block");
    };
    assert_eq!(block.text, "Usage: /limits");
}

#[test]
fn status_appears_in_candidates() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/status"), "missing /status");
}

#[test]
fn usage_appears_in_candidates() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/usage"), "missing /usage");
}

#[test]
fn limits_appears_in_candidates() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/limits"), "missing /limits");
}

#[test]
fn mcp_appears_in_candidates() {
    let app = App::test_default();
    let names: Vec<String> =
        supported_command_candidates(&app).into_iter().map(|c| c.primary).collect();
    assert!(names.iter().any(|n| n == "/mcp"), "missing /mcp");
}
