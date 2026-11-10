// SPDX-License-Identifier: Apache-2.0
use super::overlay_support::*;
use crate::agent::model;

#[test]
fn rename_availability_keeps_draft_reason_and_permitted_controls_inside_the_modal() {
    let error =
        "CHECK_FAILED: independent transcript access failure; try the saved project. ERROR_END";
    for (state, generating) in [
        (SessionTranscriptState::Saved, false),
        (SessionTranscriptState::NotSaved, false),
        (SessionTranscriptState::CheckFailed(error.into()), false),
        (SessionTranscriptState::Saved, true),
    ] {
        let mut app = app(ConfigTab::Status);
        app.config.session_transcript = Some(state.clone());
        app.config.overlay = Some(ConfigOverlayState::SessionRename(SessionRenameOverlayState {
            draft: "Independent draft".into(),
            cursor: 17,
        }));
        if generating {
            app.config.pending_session_title_change = Some(PendingSessionTitleChangeState {
                session_id: "overlay-session".into(),
                kind: PendingSessionTitleChangeKind::Generate,
            });
        }
        for (w, h) in [(100, 32), (60, 14)] {
            let b = draw(&mut app, w, h);
            let lines = rows(&b, boxed(&b, "Rename session"));
            let input = find(&lines, "Independent draft");
            let hints = help(&lines);
            assert!(hints.contains("Esc cancel"));
            assert_eq!(
                hints.contains("Enter confirm"),
                matches!(state, SessionTranscriptState::Saved) && !generating
            );
            match &state {
                SessionTranscriptState::NotSaved => {
                    let reason = find(&lines, "Renaming becomes");
                    assert!(reason > input);
                    let message = lines[reason..].join(" ");
                    assert!(message.contains("saved") && message.contains("send a message"));
                }
                SessionTranscriptState::CheckFailed(_) => {
                    let start = find(&lines, "CHECK_FAILED");
                    let end = lines
                        .iter()
                        .position(|r| r.contains("Leave the field"))
                        .unwrap_or_else(|| find(&lines, "Esc cancel"));
                    assert!(start > input);
                    wrapped(&lines[start..end], error);
                }
                SessionTranscriptState::Saved => {
                    assert!(!lines.iter().any(|r| r.contains("after this session is saved")));
                }
            }
            if generating {
                assert!(find(&lines, "being generated") > input);
            }
        }
    }
}

fn confirmation() -> App {
    let mut app = app(ConfigTab::Plugins);
    app.plugins.active_tab = PluginsViewTab::Marketplace;
    app.plugins.marketplaces = vec![MarketplaceSourceEntry {
        name: "target-market".into(),
        source: Some("github".into()),
        repo: Some("owner/target".into()),
    }];
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.config.confirmation_overlay().is_some());
    app.config.status_message = Some("BACKGROUND_STATUS".into());
    app
}
#[test]
fn confirmation_preserves_target_and_selected_choices_in_small_and_short_terminals() {
    for (w, h) in [(100, 28), (40, 8), (32, 6)] {
        let mut app = confirmation();
        for advance in [false, true] {
            if advance {
                key(&mut app, KeyCode::Down);
            }
            let b = draw(&mut app, w, h);
            let title = app.config.confirmation_overlay().unwrap().title.clone();
            let area = boxed(&b, &title);
            let lines = rows(&b, area);
            let heading = rows(&b, Rect::new(area.x, area.y - 1, area.width, 1));
            assert!(heading[0].contains("Target Market"));
            assert!(!lines.iter().any(|r| r.contains("BACKGROUND_STATUS")));
            let text = lines.join(" ");
            selected(&lines, if advance { "Remove" } else { "Cancel" });
            assert!(
                text.contains("Remove marketplace") && help(&lines).contains("Esc cancel"),
                "{w}x{h}: {lines:?}"
            );
        }
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.config.marketplace_actions_overlay().unwrap().name, "target-market");
    }
}

#[test]
fn callback_error_and_draft_occupy_the_foreground_modal_without_background_feedback() {
    let mut app = app(ConfigTab::Mcp);
    app.config.overlay = Some(ConfigOverlayState::McpCallbackUrl(McpCallbackUrlOverlayState {
        server_name: "chosen-server".into(),
        draft: "https://callback.test/code".into(),
        cursor: 26,
    }));
    let error = "CALLBACK_FAILED: the supplied callback was rejected. Retain this address for correction. ERROR_END";
    app.config.set_overlay_error(error);
    app.config.last_error = Some("BACKGROUND_ERROR".into());
    for (w, h) in [(100, 30), (80, 16)] {
        let b = draw(&mut app, w, h);
        let all = rows(&b, b.area);
        let title = all.iter().find(|r| r.contains('\u{250c}') && !r.contains("Search")).unwrap();
        let title = title.trim_matches(|c: char| {
            c == '\u{250c}' || c == '\u{2500}' || c == '\u{2510}' || c.is_whitespace()
        });
        let lines = rows(&b, boxed(&b, title));
        let start = find(&lines, "CALLBACK_FAILED");
        let end = find(&lines, "Esc");
        wrapped(&lines[start..end], error);
        assert!(find(&lines, "https://callback.test/code") < start);
        assert!(!lines.iter().any(|r| r.contains("BACKGROUND_ERROR")));
        assert!(help(&lines).contains("Esc"));
    }
}

#[test]
fn action_overlays_clip_descriptions_separately_and_keep_each_selected_action_visible() {
    for kind in 0..3 {
        let mut app = app(ConfigTab::Plugins);
        let description = format!(
            "DESCRIPTION_START {} DESCRIPTION_END",
            "independent long description ".repeat(20)
        );
        let (title, actions) = match kind {
            0 => {
                app.config.overlay = Some(ConfigOverlayState::InstalledPluginActions(
                    InstalledPluginActionOverlayState {
                        plugin_id: "target@market".into(),
                        title: "TARGET_PLUGIN".into(),
                        description,
                        scope: "user".into(),
                        project_path: None,
                        selected_index: 0,
                        actions: vec![
                            InstalledPluginActionKind::Disable,
                            InstalledPluginActionKind::Update,
                            InstalledPluginActionKind::Uninstall,
                        ],
                    },
                ));
                ("Installed plugin", vec!["Disable", "Update", "Uninstall"])
            }
            1 => {
                app.config.overlay =
                    Some(ConfigOverlayState::PluginInstallActions(PluginInstallOverlayState {
                        plugin_id: "target@market".into(),
                        title: "TARGET_PLUGIN".into(),
                        description,
                        selected_index: 0,
                        actions: vec![
                            PluginInstallActionKind::User,
                            PluginInstallActionKind::Project,
                            PluginInstallActionKind::Local,
                        ],
                    }));
                (
                    "Install plugin",
                    vec!["Install for user", "Install for project", "Install locally"],
                )
            }
            _ => {
                app.config.overlay =
                    Some(ConfigOverlayState::MarketplaceActions(MarketplaceActionsOverlayState {
                        name: "target-market".into(),
                        title: "TARGET_MARKET".into(),
                        description,
                        selected_index: 0,
                        actions: vec![MarketplaceActionKind::Update, MarketplaceActionKind::Remove],
                    }));
                ("Marketplace", vec!["Update", "Remove"])
            }
        };
        for (index, action) in actions.iter().enumerate() {
            if index > 0 {
                key(&mut app, KeyCode::Down);
            }
            for (w, h) in [(100, 30), (56, 14)] {
                let b = draw(&mut app, w, h);
                let lines = rows(&b, boxed(&b, title));
                let start = find(&lines, "DESCRIPTION_START");
                let separator = lines.iter().position(|r| r.trim().starts_with("-----")).unwrap();
                assert!(start < separator);
                let description = lines[start..separator].join(" ");
                assert!(description.contains("...") || description.contains('\u{2026}'));
                assert!(!lines.iter().any(|r| r.contains("DESCRIPTION_END")));
                selected(&lines[separator + 1..], action);
                assert!(help(&lines).contains("Esc cancel"));
            }
        }
    }
}

#[test]
fn marketplace_source_cursor_and_adjacent_unicode_survive_clipping_and_resize() {
    let mut app = app(ConfigTab::Plugins);
    app.plugins.active_tab = PluginsViewTab::Marketplace;
    key(&mut app, KeyCode::Enter);
    let prefix = format!("HEAD/{}A", "segment/".repeat(10));
    let draft = format!("{prefix}\u{754c}B/{}/END.json", "segment/".repeat(8));
    paste(&mut app, &draft);
    for position in [0, prefix.chars().count(), draft.chars().count()] {
        key(&mut app, KeyCode::Home);
        for _ in 0..position {
            key(&mut app, KeyCode::Right);
        }
        for (w, h) in [(64, 18), (100, 28)] {
            let b = draw(&mut app, w, h);
            let area = boxed(&b, "Add Marketplace");
            let lines = rows(&b, area);
            let needle = if position == 0 {
                "HEAD/"
            } else if position == prefix.chars().count() {
                "A\u{754c}"
            } else {
                "END.json"
            };
            let y = area.y + u16::try_from(find(&lines, needle)).unwrap();
            let x = if position == draft.chars().count() {
                (area.x..area.right()).find(|&x| b[(x, y)].symbol() == "E").unwrap() + 8
            } else {
                (area.x..area.right())
                    .find(|&x| b[(x, y)].symbol() == if position == 0 { "H" } else { "\u{754c}" })
                    .unwrap()
            };
            let right = if position == prefix.chars().count() { 2 } else { 1 };
            assert_ne!(b[(x, y)].bg, b[(x + right, y)].bg, "cursor must differ from its neighbor");
            assert_eq!(
                b[(x, y)].symbol(),
                if position == 0 {
                    "H"
                } else if position == prefix.chars().count() {
                    "\u{754c}"
                } else {
                    " "
                }
            );
            let field = &lines[usize::from(y - area.y)];
            assert!(
                field.chars().any(|ch| !ch.is_whitespace() && !draft.contains(ch)),
                "clipped input needs an overflow indicator"
            );
            assert!(help(&lines).contains("Enter add"));
        }
    }
    assert_eq!(app.config.plugin_source_input_overlay().unwrap().draft, draft);
}

#[test]
fn mcp_details_follow_the_chosen_server_and_keep_enabled_and_disabled_actions_distinct() {
    let mut app = app(ConfigTab::Mcp);
    app.mcp.servers = [("alpha-server", "alpha-command"), ("chosen-server", "chosen-command")]
        .into_iter()
        .map(|(name, command)| model::McpServerStatus {
            name: name.into(),
            status: model::McpServerConnectionStatus::Connected,
            server_info: None,
            error: None,
            config: Some(model::McpServerStatusConfig::Stdio {
                command: command.into(),
                args: vec![],
                env: std::collections::BTreeMap::new(),
                timeout: None,
                request_timeout_ms: None,
                always_load: None,
            }),
            scope: Some(if name == "chosen-server" { "project" } else { "user" }.into()),
            source: None,
            tools: vec![],
        })
        .collect();
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.config.mcp_details_overlay().unwrap().server_name, "chosen-server");
    key(&mut app, KeyCode::Down);
    for (w, h) in [(100, 32), (60, 22)] {
        key(&mut app, KeyCode::Home);
        let b = draw(&mut app, w, h);
        let lines = rows(&b, boxed(&b, "chosen-server"));
        assert!(
            lines.iter().any(|r| r.contains("project"))
                && lines.iter().any(|r| r.contains("stdio"))
        );
        selected(&lines, "Clear auth");
        assert!(lines[find(&lines, "Clear auth")].contains("not available"));
        assert!(!lines[find(&lines, "Reconnect server")].contains("not available"));
        assert!(help(&lines).contains("Esc cancel"));
        key(&mut app, KeyCode::End);
        let b = draw(&mut app, w, h);
        let lines = rows(&b, boxed(&b, "chosen-server"));
        find(&lines, "chosen-command");
        assert!(!lines.iter().any(|r| r.contains("alpha-command")));
        selected(&lines, "Clear auth");
    }
    app.mcp.auth_capabilities.clear_auth = true;
    let b = draw(&mut app, 100, 32);
    let lines = rows(&b, boxed(&b, "chosen-server"));
    selected(&lines, "Clear auth");
    assert!(!lines[find(&lines, "Clear auth")].contains("not available"));
    key(&mut app, KeyCode::Down);
    let b = draw(&mut app, 100, 32);
    let lines = rows(&b, boxed(&b, "chosen-server"));
    selected(&lines, "Reconnect server");
}
