// SPDX-License-Identifier: Apache-2.0
use super::{
    InstalledPluginEntry, MarketplaceEntry, MarketplaceSourceEntry, PluginsInventorySnapshot,
};
use crate::app::claude_cli;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct InstalledPluginJson {
    id: String,
    version: Option<String>,
    scope: String,
    enabled: bool,
    #[serde(rename = "installedAt")]
    installed_at: Option<String>,
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct MarketplaceListJson {
    available: Vec<AvailablePluginJson>,
}

#[derive(Debug, Deserialize)]
struct AvailablePluginJson {
    #[serde(rename = "pluginId")]
    plugin_id: String,
    name: String,
    description: Option<String>,
    #[serde(rename = "marketplaceName")]
    marketplace_name: Option<String>,
    version: Option<String>,
    #[serde(rename = "installCount")]
    install_count: Option<u64>,
    source: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct MarketplaceSourceJson {
    name: String,
    source: Option<String>,
    repo: Option<String>,
}

pub(super) async fn refresh_inventory(
    cwd_raw: String,
    cached_claude_path: Option<PathBuf>,
) -> Result<(PluginsInventorySnapshot, PathBuf), String> {
    tokio::task::spawn_blocking(move || {
        let claude_path = claude_cli::resolve_claude_path(cached_claude_path)?;
        let snapshot = refresh_inventory_blocking(&claude_path, &cwd_raw)?;
        Ok((snapshot, claude_path))
    })
    .await
    .map_err(|error| format!("Plugin inventory task failed: {error}"))?
}

pub(super) async fn run_cli_command_and_refresh(
    cwd_raw: String,
    cached_claude_path: Option<PathBuf>,
    args: Vec<String>,
) -> Result<(PluginsInventorySnapshot, PathBuf), String> {
    tokio::task::spawn_blocking(move || {
        let claude_path = claude_cli::resolve_claude_path(cached_claude_path)?;
        claude_cli::run_command(&claude_path, &cwd_raw, &args)?;
        let snapshot = refresh_inventory_blocking(&claude_path, &cwd_raw).map_err(|error| {
            format!(
                "Plugin action `claude {}` completed, but inventory refresh failed: {error}",
                args.join(" ")
            )
        })?;
        Ok((snapshot, claude_path))
    })
    .await
    .map_err(|error| format!("Plugin CLI action task failed: {error}"))?
}

fn refresh_inventory_blocking(
    claude_path: &Path,
    cwd_raw: &str,
) -> Result<PluginsInventorySnapshot, String> {
    let installed = claude_cli::parse_json_command::<Vec<InstalledPluginJson>>(
        claude_path,
        cwd_raw,
        &["plugin", "list", "--json"],
    )?;
    let available = claude_cli::parse_json_command::<MarketplaceListJson>(
        claude_path,
        cwd_raw,
        &["plugin", "list", "--available", "--json"],
    )?;
    let marketplaces = claude_cli::parse_json_command::<Vec<MarketplaceSourceJson>>(
        claude_path,
        cwd_raw,
        &["plugin", "marketplace", "list", "--json"],
    )?;

    let mut installed_entries =
        installed.into_iter().map(installed_entry_from_json).collect::<Vec<_>>();
    installed_entries.sort_by_cached_key(|entry| entry.id.to_ascii_lowercase());

    let mut marketplace_entries = available
        .available
        .into_iter()
        .map(|entry| MarketplaceEntry {
            plugin_id: entry.plugin_id,
            name: entry.name,
            description: entry.description,
            marketplace_name: entry.marketplace_name,
            version: entry.version,
            install_count: entry.install_count,
            source: entry.source,
        })
        .collect::<Vec<_>>();
    marketplace_entries.sort_by_cached_key(|entry| {
        (
            entry.marketplace_name.as_deref().unwrap_or_default().to_ascii_lowercase(),
            entry.name.to_ascii_lowercase(),
        )
    });

    let mut marketplace_sources = marketplaces
        .into_iter()
        .map(|entry| MarketplaceSourceEntry {
            name: entry.name,
            source: entry.source,
            repo: entry.repo,
        })
        .collect::<Vec<_>>();
    marketplace_sources.sort_by_cached_key(|entry| entry.name.to_ascii_lowercase());

    Ok(PluginsInventorySnapshot {
        installed: installed_entries,
        marketplace: marketplace_entries,
        marketplaces: marketplace_sources,
    })
}

fn installed_entry_from_json(entry: InstalledPluginJson) -> InstalledPluginEntry {
    let mcp_server_names = mcp_server_names_from_map(&entry.mcp_servers);
    InstalledPluginEntry {
        id: entry.id,
        version: entry.version,
        scope: entry.scope,
        enabled: entry.enabled,
        installed_at: entry.installed_at,
        last_updated: entry.last_updated,
        project_path: entry.project_path,
        mcp_server_names,
    }
}

fn mcp_server_names_from_map(servers: &BTreeMap<String, Value>) -> Vec<String> {
    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{InventoryCli, assert_fixture_snapshot, settle};
    use super::*;
    use crate::app::test_support::{app_with_connection, press};
    use crossterm::event::KeyCode;

    #[test]
    fn inventory_commands_map_complete_independent_fixtures() {
        let fixture = InventoryCli::new();
        let snapshot =
            refresh_inventory_blocking(&fixture.executable, &fixture.cwd()).expect("inventory");
        assert_fixture_snapshot(&snapshot);
        assert_eq!(
            fixture.calls(),
            [
                vec!["plugin", "list", "--json"],
                vec!["plugin", "list", "--available", "--json"],
                vec!["plugin", "marketplace", "list", "--json"],
            ]
        );
        fixture.write("installed.json", "[]");
        fixture.write("available.json", r#"{"available":[]}"#);
        fixture.write("sources.json", "[]");
        let empty = refresh_inventory_blocking(&fixture.executable, &fixture.cwd())
            .expect("empty inventory");
        assert_eq!(
            empty,
            PluginsInventorySnapshot {
                installed: vec![],
                marketplace: vec![],
                marketplaces: vec![]
            }
        );
    }

    #[tokio::test]
    async fn failed_inventory_after_a_persisted_action_recovers_without_repeating_the_mutation() {
        for (mode, mutate) in
            [("available", false), ("malformed", false), ("available", true), ("mutation", true)]
        {
            let fixture = InventoryCli::new();
            let (mut app, mut commands) = app_with_connection();
            fixture.attach(&mut app);
            app.surface_mode =
                crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
            app.config.active_tab = crate::app::ConfigTab::Plugins;
            settle(&mut app, super::super::inventory::request_inventory_refresh).await;
            let known = PluginsInventorySnapshot {
                installed: app.plugins.installed.clone(),
                marketplace: app.plugins.marketplace.clone(),
                marketplaces: app.plugins.marketplaces.clone(),
            };
            assert_fixture_snapshot(&known);
            assert!(commands.try_recv().is_err());
            fixture.write("journal", "");
            let before = fixture.read("installed.json");
            fixture.write("failure", mode);
            settle(&mut app, |app| {
                if mutate {
                    press(app, KeyCode::Enter); // alpha's installed action menu
                    let overlay = app
                        .config
                        .installed_plugin_actions_overlay()
                        .expect("selected action target");
                    assert_eq!(overlay.plugin_id, "alpha@fixture");
                    assert_eq!(overlay.scope, "user");
                    press(app, KeyCode::Enter); // disable
                } else {
                    press(app, KeyCode::Char('r'));
                }
            })
            .await;
            let error = if mutate {
                app.config.last_error.as_deref()
            } else {
                app.plugins.last_error.as_deref()
            }
            .expect("visible failure");
            let expected_stage = if mode == "mutation" {
                "plugin disable alpha@fixture --scope user"
            } else {
                "plugin list --available --json"
            };
            assert!(error.contains(expected_stage), "{error}");
            assert!(
                error.contains(if mode == "malformed" {
                    "JSON"
                } else if mode == "mutation" {
                    "registration locked"
                } else {
                    "marketplace unavailable"
                }),
                "{error}"
            );
            let persisted = mutate && mode != "mutation";
            if persisted {
                assert!(
                    error.contains("completed") && error.contains("refresh"),
                    "must distinguish completed action from failed inventory: {error}"
                );
            }
            assert_eq!(app.plugins.installed, known.installed);
            assert_eq!(app.plugins.marketplace, known.marketplace);
            assert_eq!(app.plugins.marketplaces, known.marketplaces);
            assert!(commands.try_recv().is_err(), "unconfirmed result must not reload runtime");
            assert_eq!(
                fixture.read("installed.json"),
                if persisted { fixture.read("installed-after.json") } else { before.clone() }
            );
            let mutation = vec!["plugin", "disable", "alpha@fixture", "--scope", "user"];
            let mut expected: Vec<Vec<&str>> = if mutate { vec![mutation.clone()] } else { vec![] };
            if mode != "mutation" {
                expected.extend([
                    vec!["plugin", "list", "--json"],
                    vec!["plugin", "list", "--available", "--json"],
                ]);
            }
            assert_eq!(fixture.calls(), expected);
            recover_inventory_without_mutation(
                &fixture,
                &mut app,
                &mut commands,
                persisted,
                expected,
            )
            .await;
        }
    }

    async fn recover_inventory_without_mutation(
        fixture: &InventoryCli,
        app: &mut crate::app::App,
        commands: &mut crate::agent::client::CommandReceiver,
        persisted: bool,
        mut expected: Vec<Vec<&str>>,
    ) {
        // Repair the listing, then use a read-only manual refresh. No action is replayed.
        fixture.write("failure", "");
        fixture.write(
            "available.json",
            r#"{"available":[{"pluginId":"recovered@new","name":"Recovered"}]}"#,
        );
        fixture.write("sources.json", r#"[{"name":"new","source":"git","repo":"recovery/repo"}]"#);
        if !persisted {
            fixture.write("installed.json", &fixture.read("installed-after.json"));
        }
        settle(app, |app| press(app, KeyCode::Char('r'))).await;
        assert_eq!(
            app.plugins
                .installed
                .iter()
                .map(|p| (p.id.as_str(), p.enabled, p.version.as_deref()))
                .collect::<Vec<_>>(),
            [("alpha@fixture", false, Some("2.8"))]
        );
        assert_eq!(
            app.plugins
                .marketplace
                .iter()
                .map(|p| (p.plugin_id.as_str(), p.name.as_str()))
                .collect::<Vec<_>>(),
            [("recovered@new", "Recovered")]
        );
        assert_eq!(
            app.plugins
                .marketplaces
                .iter()
                .map(|p| (p.name.as_str(), p.repo.as_deref()))
                .collect::<Vec<_>>(),
            [("new", Some("recovery/repo"))]
        );
        expected.extend([
            vec!["plugin", "list", "--json"],
            vec!["plugin", "list", "--available", "--json"],
            vec!["plugin", "marketplace", "list", "--json"],
        ]);
        assert_eq!(fixture.calls(), expected);
        let reload = commands.try_recv().expect("explicit refresh reload");
        assert!(
            matches!(reload.command, crate::agent::wire::BridgeCommand::ReloadPlugins {ref session_id, force:false, ref add_plugins} if session_id=="session-1" && add_plugins.is_empty())
        );
        assert!(reload.request_id.is_some());
        crate::app::handle_client_event(
            app,
            crate::agent::events::ClientEvent::RuntimeReloadCompleted {
                session_id: "session-1".into(),
                request_id: reload.request_id,
                error_count: 0,
                reconciliation_error: None,
            },
        );
        assert!(app.config.last_error.is_none());
        assert!(app.plugins.last_error.is_none());
        assert!(app.plugins.pending_reload.is_none());
        assert!(commands.try_recv().is_err());
    }
}
