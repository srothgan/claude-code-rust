// SPDX-License-Identifier: Apache-2.0
use super::installed_plugin_entry;
use super::overlay_support::*;

fn plugins() -> App {
    let mut app = app(ConfigTab::Plugins);
    app.cwd_raw = "C:/audit/project".into();
    app.plugins.marketplace =
        vec![catalog("beta", "BETA_DESCRIPTION"), catalog("alpha", "ALPHA_DESCRIPTION")];
    app.plugins.installed = vec![
        installed_plugin_entry("alpha@market", "user", None),
        installed_plugin_entry("beta@market", "project", Some("C:/audit/project")),
    ];
    app.plugins.installed[1].enabled = false;
    app
}
fn list_rows(buffer: &Buffer) -> Vec<String> {
    let search = boxed(buffer, "Search");
    let all = rows(buffer, buffer.area);
    let end =
        all.iter().position(|r| r.contains("Enter actions") || r.contains("Type search")).unwrap();
    all[usize::from(search.bottom() + 1)..end].to_vec()
}
#[test]
fn installed_and_catalog_navigation_open_the_selected_identity_description_and_action_set() {
    for installed in [true, false] {
        let mut app = plugins();
        if !installed {
            key(&mut app, KeyCode::Right);
        }
        // Catalog order is deliberately different from installed order.
        let (first, chosen, description) = if installed {
            ("Alpha", "Beta", "BETA_DESCRIPTION")
        } else {
            ("Beta", "Alpha", "ALPHA_DESCRIPTION")
        };
        let b = draw(&mut app, 100, 40);
        selected(&list_rows(&b), first);
        key(&mut app, KeyCode::Down);
        for (w, h) in [(100, 40), (40, 20)] {
            let b = draw(&mut app, w, h);
            let lines = list_rows(&b);
            selected(&lines, chosen);
            let start = find(&lines, chosen);
            assert!(
                lines[start..]
                    .iter()
                    .any(|r| r.contains(&format!("Plugin: {}@market", chosen.to_lowercase())))
            );
        }
        key(&mut app, KeyCode::Enter);
        let title = if installed { "Installed plugin" } else { "Install plugin" };
        let b = draw(&mut app, 100, 32);
        let lines = rows(&b, boxed(&b, title));
        assert!(lines.iter().any(|r| r.contains(description)));
        if installed {
            let target = app.config.installed_plugin_actions_overlay().unwrap();
            assert_eq!(target.plugin_id, "beta@market");
            assert_eq!(target.scope, "project");
            assert_eq!(target.project_path.as_deref(), Some("C:/audit/project"));
            for action in ["Enable", "Update", "Uninstall"] {
                find(&lines, action);
            }
            assert!(!lines.iter().any(|r| r.contains("Disable")));
            selected(&lines, "Enable");
        } else {
            assert_eq!(app.config.plugin_install_overlay().unwrap().plugin_id, "alpha@market");
            for action in ["Install for user", "Install for project", "Install locally"] {
                find(&lines, action);
            }
            selected(&lines, "Install for user");
        }
    }
}

#[test]
fn search_focus_and_normalized_paste_filter_the_list_without_changing_the_chat_draft() {
    for installed in [true, false] {
        let mut app = plugins();
        app.plugins.marketplace[0].description = Some("beta market".into());
        app.input.set_text("CHAT_DRAFT");
        if !installed {
            key(&mut app, KeyCode::Right);
        }
        key(&mut app, KeyCode::Up);
        let b = draw(&mut app, 100, 40);
        assert!(!list_rows(&b).iter().any(|r| r.contains('\u{203a}')));
        paste(&mut app, if installed { "beta" } else { "beta\r\nmarket" });
        let b = draw(&mut app, 100, 32);
        let query = rows(&b, boxed(&b, "Search")).join("");
        assert!(query.contains(if installed { "beta" } else { "beta market" }));
        assert_eq!(app.input.text(), "CHAT_DRAFT");
        assert!(!list_rows(&b).iter().any(|r| r.contains("alpha@market")));
        key(&mut app, KeyCode::Down);
        let b = draw(&mut app, 100, 32);
        selected(&list_rows(&b), "Beta");
        key(&mut app, KeyCode::Enter);
        let target = if installed {
            &app.config.installed_plugin_actions_overlay().unwrap().plugin_id
        } else {
            &app.config.plugin_install_overlay().unwrap().plugin_id
        };
        assert_eq!(target, "beta@market");
        assert_eq!(app.input.text(), "CHAT_DRAFT");
    }
}

#[test]
fn long_focused_search_shows_its_suffix_cursor_and_separate_empty_result_and_close_control() {
    let mut app = plugins();
    key(&mut app, KeyCode::Up);
    paste(&mut app, &format!("{}QUERY_TAIL", "unmatched ".repeat(40)));
    for (w, h) in [(40, 16), (80, 24)] {
        let b = draw(&mut app, w, h);
        let search = boxed(&b, "Search");
        let lines = rows(&b, search);
        let y = search.y + u16::try_from(find(&lines, "QUERY_TAIL")).unwrap();
        let x = (search.x..search.right()).find(|&x| b[(x, y)].symbol() == "Q").unwrap() + 10;
        assert_eq!(b[(x, y)].symbol(), " ");
        assert_ne!(b[(x, y)].bg, b[(x - 1, y)].bg);
        let all = rows(&b, b.area);
        assert!(find(&all, "No plugins") > usize::from(search.bottom()));
        assert!(help(&all).contains("Esc close"));
    }
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
}

#[test]
fn marketplace_rows_and_empty_add_action_open_the_correct_source_overlay() {
    for empty in [false, true] {
        let mut app = app(ConfigTab::Plugins);
        app.plugins.active_tab = PluginsViewTab::Marketplace;
        if !empty {
            app.plugins.marketplaces = vec![
                MarketplaceSourceEntry {
                    name: "alpha".into(),
                    source: Some("github".into()),
                    repo: Some("owner/alpha".into()),
                },
                MarketplaceSourceEntry {
                    name: "beta".into(),
                    source: Some("git".into()),
                    repo: Some("owner/beta".into()),
                },
            ];
            for (index, title, repo) in [(0, "Alpha", "owner/alpha"), (1, "Beta", "owner/beta")] {
                if index > 0 {
                    key(&mut app, KeyCode::Down);
                }
                for (w, h) in [(100, 30), (40, 20)] {
                    let b = draw(&mut app, w, h);
                    let all = rows(&b, b.area);
                    let start = find(&all, title);
                    selected(&all[start..], title);
                    let end = all[start + 1..]
                        .iter()
                        .position(|r| r.contains("Add marketplace") || r.contains("Beta"))
                        .map_or(all.len(), |n| start + 1 + n);
                    assert!(all[start..end].iter().any(|r| r.contains(repo)));
                }
                key(&mut app, KeyCode::Enter);
                assert_eq!(
                    app.config.marketplace_actions_overlay().unwrap().name,
                    title.to_lowercase()
                );
                let b = draw(&mut app, 100, 30);
                let lines = rows(&b, boxed(&b, "Marketplace"));
                find(&lines, repo);
                find(&lines, if index == 0 { "github" } else { "git" });
                selected(&lines, "Update");
                find(&lines, "Remove");
                key(&mut app, KeyCode::Esc);
            }
            key(&mut app, KeyCode::Down);
        }
        let b = draw(&mut app, 40, 20);
        selected(&rows(&b, b.area), "Add marketplace");
        key(&mut app, KeyCode::Enter);
        assert!(app.config.plugin_source_input_overlay().is_some());
        paste(&mut app, "owner/new-market");
        let b = draw(&mut app, 80, 24);
        let lines = rows(&b, boxed(&b, "Add Marketplace"));
        find(&lines, "owner/new-market");
        assert!(help(&lines).contains("Enter add"));
        key(&mut app, KeyCode::Esc);
        assert!(app.config.overlay.is_none());
    }
}

#[test]
fn plugin_metadata_remains_complete_and_joins_reordered_catalog_descriptions_by_identity() {
    let descriptions = [
        "Alpha description preserves every middle word and the final alpha tail.",
        "Beta description has distinct middle words and ends in its own beta tail.",
    ];
    let sources = [
        "https://alpha.example/long/path/with/middle/segments/source-alpha.json",
        "https://beta.example/different/path/with/middle/segments/source-beta.json",
    ];
    for installed in [false, true] {
        let mut app = plugins();
        app.plugins.marketplace =
            vec![catalog("beta", descriptions[1]), catalog("alpha", descriptions[0])];
        app.plugins.marketplace[0].source = Some(serde_json::json!(sources[1]));
        app.plugins.marketplace[1].source = Some(serde_json::json!(sources[0]));
        for (i, entry) in app.plugins.installed.iter_mut().enumerate() {
            entry.version = Some(format!("inventory-{i}"));
            entry.installed_at = Some(format!("2026-09-0{}", i + 1));
            entry.last_updated = Some(format!("2026-10-0{}", i + 1));
            entry.mcp_server_names = vec![format!("server-{i}"), format!("helper-{i}")];
        }
        if !installed {
            key(&mut app, KeyCode::Right);
        }
        let b = draw(&mut app, 60, 60);
        let lines = list_rows(&b);
        for (name, i, other) in if installed {
            [("Alpha", 0, "Beta"), ("Beta", 1, "")]
        } else {
            [("Beta", 1, "Alpha"), ("Alpha", 0, "")]
        } {
            let start = find(&lines, name);
            let end = if other.is_empty() { lines.len() } else { find(&lines, other) };
            let block = &lines[start..end];
            find(block, &format!("Plugin: {}@market", name.to_lowercase()));
            field(
                block,
                "Description:",
                if installed { "Version:" } else { "Marketplace:" },
                &format!("Description: {}", descriptions[i]),
            );
            if installed {
                for text in [
                    format!("Version: inventory-{i}"),
                    format!("Installed: 2026-09-0{}", i + 1),
                    format!("Updated: 2026-10-0{}", i + 1),
                    format!("MCP servers: server-{i}, helper-{i}"),
                ] {
                    find(block, &text);
                }
                find(block, if i == 0 { "Status: enabled" } else { "Status: disabled" });
                find(block, if i == 0 { "Scope: user" } else { "Scope: project" });
                if i == 1 {
                    find(block, "Project: C:/audit/project");
                }
                assert!(!block.iter().any(|r| r.contains("catalog-version")));
            } else {
                find(block, "Marketplace: market");
                find(block, "Version: catalog-version");
                find(block, "Installs: 123");
                field(block, "Source:", "", &format!("Source: {}", sources[i]));
            }
        }
    }
}

#[test]
fn installed_list_filters_other_projects_but_keeps_user_and_current_project_entries() {
    let mut app = plugins();
    app.plugins
        .installed
        .insert(0, installed_plugin_entry("foreign@market", "local", Some("C:/audit/other")));
    let b = draw(&mut app, 100, 40);
    let lines = list_rows(&b);
    find(&lines, "alpha@market");
    find(&lines, "beta@market");
    assert!(!lines.iter().any(|r| r.contains("foreign@market")));
}

#[test]
fn loading_and_settled_empty_inventory_have_distinct_content_with_working_close() {
    for loading in [true, false] {
        let mut app = app(ConfigTab::Plugins);
        app.plugins.loading = loading;
        let b = draw(&mut app, 80, 24);
        let lines = list_rows(&b);
        let content = lines.join(" ");
        assert_eq!(content.contains("Loading plugins"), loading);
        assert_eq!(content.contains("No plugins"), !loading);
        assert!(help(&rows(&b, b.area)).contains("Esc close"));
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.surface_mode, SurfaceMode::Chat);
    }
}
