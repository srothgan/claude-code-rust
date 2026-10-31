// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::{events::ClientEvent, model::SessionUpdate, wire::BridgeCommand};
use crate::app::{App, FullscreenView, SurfaceMode};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

async fn child(name: &str) -> Option<PathBuf> {
    if std::env::var("CLAUDE_RS_AUDIT_CASE").ok().as_deref() == Some(name) {
        return Some(std::env::var_os("CLAUDE_RS_AUDIT_ROOT").unwrap().into());
    }
    let root = tempfile::tempdir().unwrap();
    let mut command = crate::diagnostics_test_support::command(
        &format!("app::config::status_usage_tests::{name}"),
        root.path(),
        name,
    );
    command.current_dir(root.path());
    let output = crate::diagnostics_test_support::run(command).await;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    None
}

fn fixture(root: &Path) -> (App, crate::agent::client::CommandReceiver) {
    let (mut app, mut commands) = crate::app::test_support::app_with_connection();
    app.settings_home_override = Some(root.join("home"));
    // The SDK's fallback folder for this nonexistent relative cwd is literally project-a.
    // A fresh child cwd prevents another test or checkout from making it canonicalizable.
    assert!(!Path::new("project-a").exists());
    app.cwd_raw = "project-a".into();
    app.session_runtime.session_title = Some("Live title".into());
    app.recent_sessions = vec![crate::app::RecentSessionInfo {
        session_id: "session-1".into(),
        summary: "Listed summary".into(),
        last_modified_ms: 1,
        file_size_bytes: 2,
        cwd: Some("project-a".into()),
        git_branch: None,
        custom_title: Some("Listed title".into()),
        first_prompt: Some("First request".into()),
    }];
    open_tab(&mut app, ConfigTab::Status).unwrap();
    assert_eq!(
        commands.try_recv().unwrap().command,
        BridgeCommand::GetStatusSnapshot { session_id: "session-1".into() }
    );
    assert!(commands.try_recv().is_err());
    (app, commands)
}

fn save(root: &Path, project: &str) -> PathBuf {
    let file = root.join("home/.claude/projects").join(project).join("session-1.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{}\n").unwrap();
    file
}

fn key(app: &mut App, code: KeyCode) {
    crate::app::handle_terminal_event(app, Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn draft(app: &mut App, value: &str) {
    key(app, KeyCode::Home);
    let count = app.config.session_rename_overlay().unwrap().draft.chars().count();
    for _ in 0..count {
        key(app, KeyCode::Delete);
    }
    crate::app::handle_terminal_event(app, Event::Paste(value.into()));
}

fn rendered(app: &mut App) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
    terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..40)
        .map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect::<String>().trim().to_owned())
        .collect()
}

fn name(app: &mut App) -> String {
    rendered(app).iter().find_map(|row| row.strip_prefix("Session name:")).unwrap().trim().into()
}

fn acknowledge(app: &mut App, title: Option<&str>) {
    crate::app::handle_client_event(
        app,
        ClientEvent::SessionUpdate {
            session_id: "session-1".into(),
            update: SessionUpdate::SessionTitleUpdate(title.map(Into::into)),
        },
    );
    crate::app::handle_client_event(
        app,
        ClientEvent::SessionsListed {
            sessions: vec![crate::agent::types::SessionListEntry {
                session_id: "session-1".into(),
                summary: "Listed summary".into(),
                last_modified_ms: 1,
                file_size_bytes: 2,
                cwd: Some("project-a".into()),
                git_branch: None,
                custom_title: title.map(Into::into),
                first_prompt: Some("First request".into()),
            }],
        },
    );
}

fn rename(title: &str) -> BridgeCommand {
    BridgeCommand::RenameSession { session_id: "session-1".into(), title: title.into() }
}

fn generate(description: &str) -> BridgeCommand {
    BridgeCommand::GenerateSessionTitle {
        session_id: "session-1".into(),
        description: description.into(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn rename_trims_named_or_empty_drafts_and_waits_for_acknowledgement() {
    let Some(root) =
        child("rename_trims_named_or_empty_drafts_and_waits_for_acknowledgement").await
    else {
        return;
    };
    save(&root, "project-a");
    for (input, expected) in [("  Edited title  ", Some("Edited title")), ("   ", None)] {
        let (mut app, mut commands) = fixture(&root);
        key(&mut app, KeyCode::Char('r'));
        draft(&mut app, input);
        key(&mut app, KeyCode::Enter);
        assert_eq!(commands.try_recv().unwrap().command, rename(expected.unwrap_or("")));
        assert_eq!(app.config.session_rename_overlay().unwrap().draft, input);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('x'));
        crate::app::handle_terminal_event(&mut app, Event::Paste("ignored while pending".into()));
        assert!(commands.try_recv().is_err());
        assert_eq!(app.config.session_rename_overlay().unwrap().draft, input);
        assert_eq!(app.active_session_custom_title(), Some("Live title"));
        acknowledge(&mut app, expected);
        assert!(app.config.overlay.is_none());
        assert!(app.config.pending_session_title_change.is_none());
        assert_eq!(app.session_runtime.session_title.as_deref(), expected);
        assert_eq!(app.recent_sessions[0].custom_title.as_deref(), expected);
        assert_eq!(name(&mut app), expected.unwrap_or("(unnamed session)"));
        assert!(app.transcript.messages.is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn listed_unicode_draft_is_editable_inside_the_title_and_cancel_is_reusable() {
    let Some(root) =
        child("listed_unicode_draft_is_editable_inside_the_title_and_cancel_is_reusable").await
    else {
        return;
    };
    save(&root, "project-a");
    let (mut app, mut commands) = fixture(&root);
    app.session_runtime.session_title = None;
    app.recent_sessions[0].custom_title = Some("🦀中é".into());
    key(&mut app, KeyCode::Char('r'));
    assert!(app.config.session_rename_confirmable());
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Char('!'));
    assert_eq!(app.config.session_rename_overlay().unwrap().draft, "🦀中!é");
    key(&mut app, KeyCode::Esc);
    assert!(app.config.overlay.is_none());
    assert!(commands.try_recv().is_err());
    assert_eq!(app.active_session_custom_title(), Some("🦀中é"));
    key(&mut app, KeyCode::Char('r'));
    assert_eq!(app.config.session_rename_overlay().unwrap().draft, "🦀中é");
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Char('!'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(commands.try_recv().unwrap().command, rename("🦀中!é"));
    assert!(commands.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn transcript_appearance_is_rechecked_on_dialog_open_and_finished_turn() {
    let Some(root) =
        child("transcript_appearance_is_rechecked_on_dialog_open_and_finished_turn").await
    else {
        return;
    };
    for on_turn in [false, true] {
        let file = root.join("home/.claude/projects/project-a/session-1.jsonl");
        if file.exists() {
            std::fs::remove_file(&file).unwrap();
        }
        let (mut app, mut commands) = fixture(&root);
        assert_eq!(app.config.session_transcript, Some(SessionTranscriptState::NotSaved));
        if on_turn {
            key(&mut app, KeyCode::Char('r'));
            draft(&mut app, "Waiting draft");
            key(&mut app, KeyCode::Enter);
            assert!(commands.try_recv().is_err());
            assert!(!app.config.session_rename_confirmable());
            assert_eq!(app.config.session_rename_overlay().unwrap().draft, "Waiting draft");
        }
        save(&root, "project-a");
        assert_eq!(app.config.session_transcript, Some(SessionTranscriptState::NotSaved));
        if on_turn {
            crate::app::handle_client_event(
                &mut app,
                ClientEvent::TurnComplete {
                    session_id: "session-1".into(),
                    queued_turn_count: None,
                    terminal_reason: None,
                },
            );
            assert_eq!(
                commands.try_recv().unwrap().command,
                BridgeCommand::GetContextUsage { session_id: "session-1".into() }
            );
        } else {
            key(&mut app, KeyCode::Char('r'));
        }
        assert!(app.config.session_rename_confirmable());
        assert!(commands.try_recv().is_err());
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            commands.try_recv().unwrap().command,
            rename(if on_turn { "Waiting draft" } else { "Live title" })
        );
        assert!(commands.try_recv().is_err());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resumed_sibling_transcript_authorizes_the_exact_active_session_rename() {
    let Some(root) =
        child("resumed_sibling_transcript_authorizes_the_exact_active_session_rename").await
    else {
        return;
    };
    let (mut app, mut commands) = fixture(&root);
    assert!(!Path::new("project-b").exists());
    assert!(!root.join("home/.claude/projects/project-a/session-1.jsonl").exists());
    save(&root, "project-b");
    app.recent_sessions[0].cwd = Some("project-b".into());
    key(&mut app, KeyCode::Char('r'));
    draft(&mut app, "Sibling title");
    key(&mut app, KeyCode::Enter);
    assert_eq!(commands.try_recv().unwrap().command, rename("Sibling title"));
    assert!(commands.try_recv().is_err());
    assert!(!root.join("home/.claude/projects/project-a/session-1.jsonl").exists());
}

#[tokio::test(flavor = "current_thread")]
async fn title_operations_exclude_each_other_until_completion_then_allow_retry() {
    let Some(root) =
        child("title_operations_exclude_each_other_until_completion_then_allow_retry").await
    else {
        return;
    };
    save(&root, "project-a");
    let (mut app, mut commands) = fixture(&root);
    key(&mut app, KeyCode::Char('g'));
    assert_eq!(commands.try_recv().unwrap().command, generate("Live title"));
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Enter);
    assert!(commands.try_recv().is_err());
    acknowledge(&mut app, Some("Generated title"));
    key(&mut app, KeyCode::Esc);
    // Esc may close the retained rename dialog; return to Status through its entry point.
    open_tab(&mut app, ConfigTab::Status).unwrap();
    assert!(matches!(
        commands.try_recv().unwrap().command,
        BridgeCommand::GetStatusSnapshot { .. }
    ));
    key(&mut app, KeyCode::Char('r'));
    draft(&mut app, "After generation");
    key(&mut app, KeyCode::Enter);
    assert_eq!(commands.try_recv().unwrap().command, rename("After generation"));
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('g'));
    assert!(commands.try_recv().is_err());
    assert_eq!(app.active_session_custom_title(), Some("Generated title"));
    acknowledge(&mut app, Some("After generation"));
    key(&mut app, KeyCode::Char('g'));
    assert_eq!(commands.try_recv().unwrap().command, generate("After generation"));
    assert!(commands.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn generation_uses_live_listed_summary_then_prompt_and_recovers_from_empty_metadata() {
    let Some(root) =
        child("generation_uses_live_listed_summary_then_prompt_and_recovers_from_empty_metadata")
            .await
    else {
        return;
    };
    for (live, listed, summary, prompt, expected) in [
        (
            Some("  Live choice  "),
            Some("Listed choice"),
            "Summary choice",
            Some("Prompt choice"),
            Some("Live choice"),
        ),
        (
            None,
            Some(" Listed choice "),
            "Summary choice",
            Some("Prompt choice"),
            Some("Listed choice"),
        ),
        (None, None, " Summary choice ", Some("Prompt choice"), Some("Summary choice")),
        (None, None, " ", Some(" Prompt choice "), Some("Prompt choice")),
        (None, None, " ", None, None),
    ] {
        let (mut app, mut commands) = fixture(&root);
        app.session_runtime.session_title = live.map(Into::into);
        app.recent_sessions[0].custom_title = listed.map(Into::into);
        app.recent_sessions[0].summary = summary.into();
        app.recent_sessions[0].first_prompt = prompt.map(Into::into);
        key(&mut app, KeyCode::Char('g'));
        assert_eq!(app.session_runtime.session_title.as_deref(), live);
        assert_eq!(app.recent_sessions[0].custom_title.as_deref(), listed);
        if let Some(expected) = expected {
            assert_eq!(commands.try_recv().unwrap().command, generate(expected));
        } else {
            assert!(commands.try_recv().is_err());
            assert!(app.config.pending_session_title_change.is_none());
            assert!(rendered(&mut app).iter().any(|row| row.contains("No session summary")));
            app.recent_sessions[0].first_prompt = Some("New request".into());
            key(&mut app, KeyCode::Char('g'));
            assert_eq!(commands.try_recv().unwrap().command, generate("New request"));
            assert!(app.config.last_error.is_none());
        }
        assert!(commands.try_recv().is_err());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn status_name_follows_live_precedence_and_explicit_clearing() {
    let Some(root) = child("status_name_follows_live_precedence_and_explicit_clearing").await
    else {
        return;
    };
    let (mut app, _commands) = fixture(&root);
    assert_eq!(name(&mut app), "Live title", "live title must beat the distinct listed title");
    app.session_runtime.session_title = None;
    assert_eq!(name(&mut app), "Listed title");
    crate::app::handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "session-1".into(),
            update: SessionUpdate::SessionTitleUpdate(Some("Updated live title".into())),
        },
    );
    assert_eq!(name(&mut app), "Updated live title");
    crate::app::handle_client_event(
        &mut app,
        ClientEvent::SessionUpdate {
            session_id: "session-1".into(),
            update: SessionUpdate::SessionTitleUpdate(None),
        },
    );
    assert_eq!(name(&mut app), "(unnamed session)");
    assert!(app.recent_sessions[0].custom_title.is_none());
    assert_eq!(app.recent_sessions[0].summary, "Listed summary");
}

#[tokio::test(flavor = "current_thread")]
async fn usage_tab_activation_manual_refresh_and_sdk_result_share_one_lifecycle() {
    let Some(root) =
        child("usage_tab_activation_manual_refresh_and_sdk_result_share_one_lifecycle").await
    else {
        return;
    };
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut app, mut commands) = fixture(&root);
            key(&mut app, KeyCode::Tab);
            assert_eq!(app.config.active_tab, ConfigTab::Usage);
            assert!(app.usage.in_flight);
            assert_eq!(
                commands.try_recv().unwrap().command,
                BridgeCommand::GetUsage { session_id: "session-1".into() }
            );
            key(&mut app, KeyCode::Char('r'));
            assert!(commands.try_recv().is_err());
            let receive = |app: &mut App, percent| {
                crate::app::handle_client_event(
                    app,
                    ClientEvent::StructuredUsageReceived {
                        session_id: "session-1".into(),
                        error: None,
                        snapshot: Some(
                            serde_json::from_value(
                                serde_json::json!({"five_hour": {"utilization": percent}}),
                            )
                            .unwrap(),
                        ),
                    },
                );
            };
            receive(&mut app, 27.0);
            assert!(!app.usage.in_flight);
            assert!(
                rendered(&mut app)
                    .iter()
                    .any(|row| row.starts_with("5-hour") && row.contains("27% used"))
            );
            key(&mut app, KeyCode::BackTab);
            assert!(matches!(
                commands.try_recv().unwrap().command,
                BridgeCommand::GetStatusSnapshot { .. }
            ));
            key(&mut app, KeyCode::Tab);
            assert!(commands.try_recv().is_err(), "fresh activation must reuse the snapshot");
            key(&mut app, KeyCode::Char('r'));
            assert_eq!(
                commands.try_recv().unwrap().command,
                BridgeCommand::GetUsage { session_id: "session-1".into() }
            );
            assert!(
                rendered(&mut app)
                    .iter()
                    .any(|row| row.starts_with("5-hour") && row.contains("27% used"))
            );
            key(&mut app, KeyCode::Char('r'));
            assert!(commands.try_recv().is_err());
            receive(&mut app, 63.0);
            assert!(!app.usage.in_flight);
            let rows = rendered(&mut app);
            let quota = rows.iter().find(|row| row.starts_with("5-hour")).unwrap();
            assert!(quota.contains("63% used") && !quota.contains("27%"));
            assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Config));
            assert!(commands.try_recv().is_err());
        })
        .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn denied_transcript_traversal_disables_confirmation_and_displays_the_cause() {
    use std::os::unix::fs::PermissionsExt;
    struct Restore(PathBuf, std::fs::Permissions);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }
    let Some(root) =
        child("denied_transcript_traversal_disables_confirmation_and_displays_the_cause").await
    else {
        return;
    };
    let file = save(&root, "project-a");
    let folder = file.parent().unwrap();
    let guard = Restore(folder.into(), std::fs::metadata(folder).unwrap().permissions());
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o000)).unwrap();
    match std::fs::metadata(&file) {
        Ok(_) => {
            assert!(
                std::env::var_os("CLAUDE_RS_REQUIRE_PERMISSION_DENIAL").is_none(),
                "the required permission-denial fixture was not exercised"
            );
            eprintln!("permission restriction unavailable for this user; run as non-root");
            return;
        }
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
    }
    let (mut app, mut commands) = fixture(&root);
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Enter);
    assert!(commands.try_recv().is_err());
    assert!(!app.config.session_rename_confirmable());
    let Some(SessionTranscriptState::CheckFailed(cause)) = app.config.session_transcript.as_ref()
    else {
        panic!("missing filesystem failure")
    };
    let cause = cause.clone();
    assert!(cause.contains("Permission denied"));
    let rows = rendered(&mut app);
    let start = rows.iter().position(|row| row.contains("Rename session")).unwrap();
    let end = rows.iter().position(|row| row.contains("Leave the field empty")).unwrap();
    assert!(rows[start..end].join(" ").contains(&cause));
    assert!(!rows[start..].iter().any(|row| row.contains("Enter confirm")));
    drop(guard);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('r'));
    key(&mut app, KeyCode::Enter);
    assert_eq!(commands.try_recv().unwrap().command, rename("Live title"));
}
