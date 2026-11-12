// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::client::CommandReceiver;
use crate::agent::wire::BridgeCommand;
use crate::app::test_support::app_with_connection;
use crate::app::{ConfigTab, FullscreenView, SurfaceMode};

fn submit(app: &mut App, input: &str) {
    app.input.set_text(input);
    crate::app::input_submit::submit_input(app);
}

fn assert_no_command(rx: &mut CommandReceiver) {
    assert!(matches!(rx.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)));
}

fn error_text(app: &App) -> &str {
    let message = app.transcript.messages.last().expect("error feedback");
    assert_eq!(message.role, MessageRole::System(Some(SystemSeverity::Error)));
    let Some(MessageBlock::Text(text)) = message.blocks.first() else {
        panic!("expected text feedback")
    };
    &text.text
}

#[test]
fn invalid_submissions_preserve_history_and_reject_before_dispatch_or_navigation() {
    let cases = [
        ("/fast invalid", "/fast [on|off]"),
        ("/config extra", "/config"),
        ("/memory extra", "/memory"),
        ("/permissions extra", "/permissions"),
        ("/sandbox extra", "/sandbox"),
        ("/hooks extra", "/hooks"),
        ("/mcp extra", "/mcp"),
        ("/plugins extra", "/plugins"),
        ("/login somearg", "/login"),
        ("/docs", "/docs <mode|models|shortcuts|commands|agents>"),
        ("/docs nope", "/docs <mode|models|shortcuts|commands|agents>"),
        ("/docs commands extra", "/docs <mode|models|shortcuts|commands|agents>"),
        ("/resume abc-123", "/resume"),
        ("/resume abc-123 extra", "/resume"),
        ("/rewind", "/rewind <user_message_uuid> <both|conversation|code>"),
        ("/rewind user-1", "/rewind <user_message_uuid> <both|conversation|code>"),
        ("/rewind user-1 invalid", "/rewind <user_message_uuid> <both|conversation|code>"),
        ("/effort", "/effort <low|medium|high|xhigh|max|reset>"),
        ("/effort banana", "/effort <low|medium|high|xhigh|max|reset>"),
        ("/effort high extra", "/effort <low|medium|high|xhigh|max|reset>"),
        ("/compact now", "/compact"),
        ("/mode plan extra", "/mode <id>"),
        ("/model", "/model <id>"),
        ("/model sonnet extra", "/model <id>"),
        ("/status extra", "/status"),
        ("/usage extra", "/usage"),
        ("/reload now", "/reload"),
    ];
    for (input, syntax) in cases {
        for status in [AppStatus::Ready, AppStatus::Thinking] {
            let home = tempfile::tempdir().expect("settings home");
            let (mut app, mut rx) = app_with_connection();
            app.settings_home_override = Some(home.path().to_path_buf());
            app.status = status;
            app.config.active_tab = ConfigTab::Help;
            app.config.settings.category = "memory".into();
            app.config.settings.query = "keep search".into();
            let message = ChatMessage::new(
                MessageRole::User,
                vec![MessageBlock::Text(TextBlock::from_complete("keep"))],
                None,
            );
            let saved_id = message.id;
            app.push_message_tracked(message);

            submit(&mut app, input);

            let text = error_text(&app);
            assert!(text.contains("Usage:") && text.contains(syntax), "{input}: {text}");
            let saved = app
                .transcript
                .messages
                .iter()
                .find(|m| m.id == saved_id)
                .expect("existing message must survive rejection");
            assert_eq!(saved.role, MessageRole::User);
            assert!(matches!(saved.blocks.as_slice(), [MessageBlock::Text(t)] if t.text == "keep"));
            assert_eq!(app.surface_mode, SurfaceMode::Chat, "{input}");
            assert_eq!(app.config.active_tab, ConfigTab::Help, "{input}");
            assert_eq!(app.config.settings.category, "memory", "{input}");
            assert_eq!(app.config.settings.query, "keep search", "{input}");
            assert_eq!(app.status, status, "{input}");
            assert!(app.turn.pending_command_ack.is_none(), "{input}");
            assert!(!app.turn.compaction.is_active(), "{input}");
            assert!(app.pending_submit.is_none(), "{input}");
            assert_no_command(&mut rx);
        }
    }
}

#[test]
fn local_view_submissions_own_sdk_collisions_and_reach_the_requested_destination() {
    for (input, tab, category) in [
        ("/config", ConfigTab::Settings, Some("general")),
        ("/memory", ConfigTab::Settings, Some("memory")),
        ("/permissions", ConfigTab::Settings, Some("permissions")),
        ("/sandbox", ConfigTab::Settings, Some("sandbox")),
        ("/hooks", ConfigTab::Settings, Some("hooks")),
        ("/help", ConfigTab::Help, None),
        ("/plugins", ConfigTab::Plugins, None),
        ("/mcp", ConfigTab::Mcp, None),
        ("/status", ConfigTab::Status, None),
        ("/usage", ConfigTab::Usage, None),
    ] {
        for status in [AppStatus::Ready, AppStatus::Thinking] {
            let home = tempfile::tempdir().expect("isolated settings home");
            let (mut app, mut rx) = app_with_connection();
            app.settings_home_override = Some(home.path().to_path_buf());
            app.config.active_tab =
                if tab == ConfigTab::Help { ConfigTab::Status } else { ConfigTab::Help };
            app.config.settings.category =
                if category == Some("memory") { "general" } else { "memory" }.into();
            app.config.settings.query = "old query".into();
            app.config.selected_scope = crate::agent::settings::SettingsScope::Local;
            app.sdk_inventory.available_commands = vec![
                model::AvailableCommand::new(input, "SDK collision").input_hint("<sdk-action>"),
            ];
            app.input.set_text(input);
            let _ = app.input.set_cursor(0, input.chars().count());
            let suggestions = requested_slash_state(&app).expect("app completion");
            let own = suggestions
                .candidates
                .iter()
                .filter(|c| c.insert_value == input)
                .collect::<Vec<_>>();
            assert_eq!(own.len(), 1, "one app-owned completion for {input}");
            assert_ne!(own[0].secondary.as_deref(), Some("SDK collision"));
            app.status = status;

            submit(&mut app, input);

            assert_eq!(
                app.surface_mode,
                SurfaceMode::Fullscreen(FullscreenView::Config),
                "{input}"
            );
            assert_eq!(app.config.active_tab, tab, "{input}");
            if let Some(category) = category {
                assert_eq!(app.config.settings.category, category, "{input}");
                assert!(app.config.settings.query.is_empty(), "{input}");
                assert_eq!(app.config.settings.focus, crate::app::config::SettingsFocus::Content);
            }
            assert_eq!(app.config.selected_scope, crate::agent::settings::SettingsScope::Local);
            assert_eq!(app.status, status, "{input}");
            assert!(app.pending_submit.is_none());
            assert!(app.input.is_empty());
            while let Ok(envelope) = rx.try_recv() {
                // Read-only view refreshes are allowed; an SDK Prompt is not.
                assert!(
                    matches!(envelope.command,
                        BridgeCommand::InspectSettings { ref session_id, .. } if tab == ConfigTab::Settings && session_id == "session-1"
                    ) || matches!(envelope.command,
                        BridgeCommand::GetMcpSnapshot { ref session_id } if tab == ConfigTab::Mcp && session_id == "session-1"
                    ) || matches!(envelope.command,
                        BridgeCommand::GetStatusSnapshot { ref session_id } if tab == ConfigTab::Status && session_id == "session-1"
                    ),
                    "unexpected command for {input}: {:?}",
                    envelope.command
                );
            }
            assert_no_command(&mut rx);
        }
    }
}

#[test]
fn advertised_canonical_and_alias_submissions_deliver_exact_prompt_and_unknown_stays_local() {
    for input in ["/remote-command release  blue", "/remote release  blue", "/definitely-unknown"] {
        let (mut app, mut rx) = app_with_connection();
        app.session_runtime.session_id = Some(model::SessionId::new("routing-session"));
        app.sdk_inventory.available_commands = vec![
            model::AvailableCommand::new("/remote-command", "Remote command")
                .aliases(vec!["/remote".into()]),
        ];
        assert_eq!(
            find_advertised_command(&app, "/remote").map(|c| c.name.as_str()),
            Some("/remote-command")
        );
        assert!(supported_command_candidates(&app).iter().any(|c| c.insert_value == "/remote"));

        submit(&mut app, input);

        if input == "/definitely-unknown" {
            let text = error_text(&app);
            assert!(
                text.contains(input) && text.contains("not") && text.contains("supported"),
                "{text}"
            );
            assert_eq!(app.status, AppStatus::Ready);
        } else {
            let BridgeCommand::Prompt { session_id, chunks, inline_pastes, .. } =
                rx.try_recv().expect("forwarded prompt").command
            else {
                panic!("expected Prompt")
            };
            assert_eq!(session_id, "routing-session");
            assert_eq!(chunks.len(), 1, "one complete text payload");
            assert_eq!(chunks[0].kind, "text");
            assert_eq!(chunks[0].value, json!(input));
            assert!(inline_pastes.is_empty());
            while let Ok(envelope) = rx.try_recv() {
                assert!(
                    matches!(envelope.command, BridgeCommand::GetContextUsage { session_id } if session_id == "routing-session")
                );
            }
        }
        assert_no_command(&mut rx);
        assert!(app.input.is_empty());
    }
}

#[test]
fn session_commands_report_connection_causes_without_dispatch_or_pending_work() {
    for input in
        ["/ultracode on", "/ultracode off", "/compact", "/reload", "/rewind user-1 conversation"]
    {
        for connected in [false, true] {
            let mut app = App::test_default();
            let mut rx = connected.then(|| attach_test_connection(&mut app));
            app.sdk_inventory.rewind_targets = vec![model::RewindTarget {
                uuid: "user-1".into(),
                first_text: "prompt".into(),
                input_text: "prompt".into(),
                index: 0,
                previous_assistant_uuid: None,
                resume_anchor_uuid: None,
            }];

            submit(&mut app, input);

            let text = error_text(&app);
            let command = input.split_whitespace().next().expect("command").trim_start_matches('/');
            assert!(text.to_lowercase().contains(command), "{input}: {text}");
            assert!(
                text.contains(if connected { "no active session" } else { "not connected" }),
                "{text}"
            );
            assert_eq!(app.status, AppStatus::Ready);
            assert!(app.turn.pending_command_ack.is_none());
            assert!(app.pending_submit.is_none());
            assert!(!app.turn.compaction.is_active());
            if let Some(rx) = rx.as_mut() {
                assert_no_command(rx);
            }
        }
    }
}

#[test]
fn new_session_submission_appends_the_user_command_without_replacing_history() {
    let mut app = App::test_default();
    app.push_message_tracked(ChatMessage::new(
        MessageRole::User,
        vec![MessageBlock::Text(TextBlock::from_complete("earlier question"))],
        None,
    ));
    let existing = app.transcript.messages.iter().map(|m| m.id).collect::<Vec<_>>();

    submit(&mut app, "/new-session");

    assert!(app.transcript.messages.iter().filter(|m| !existing.contains(&m.id)).any(|m|
        m.role == MessageRole::User && matches!(m.blocks.as_slice(), [MessageBlock::Text(t)] if t.text == "/new-session")
    ));
    let saved =
        app.transcript.messages.iter().find(|m| m.id == existing[0]).expect("existing history");
    assert!(
        matches!(saved.blocks.as_slice(), [MessageBlock::Text(t)] if t.text == "earlier question")
    );
}

#[test]
fn resume_picker_selects_the_current_session_when_present_and_a_listed_session_otherwise() {
    for sessions in [
        vec!["other-session", "current-session"],
        vec!["current-session", "other-session"],
        vec!["other-session", "third-session"],
    ] {
        let (mut app, mut rx) = app_with_connection();
        app.session_runtime.session_id = Some(model::SessionId::new("current-session"));
        app.recent_sessions = sessions
            .iter()
            .map(|id| crate::app::RecentSessionInfo {
                session_id: (*id).into(),
                summary: (*id).into(),
                last_modified_ms: 1,
                file_size_bytes: 1,
                cwd: None,
                git_branch: None,
                custom_title: None,
                first_prompt: None,
            })
            .collect();

        submit(&mut app, "/resume");

        assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::SessionPicker));
        let selected =
            crate::app::session_picker::selected_session(&app).expect("listed selection");
        if sessions.contains(&"current-session") {
            assert_eq!(selected.session_id, "current-session");
        } else {
            assert!(sessions.contains(&selected.session_id.as_str()));
        }
        assert_no_command(&mut rx);
    }
}
