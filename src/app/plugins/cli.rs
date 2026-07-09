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
        let snapshot = refresh_inventory_blocking(&claude_path, &cwd_raw)?;
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
    use super::*;

    #[test]
    fn parses_installed_plugin_entries() {
        let json = r#"
[
  {
    "id": "frontend-design@claude-plugins-official",
    "version": "55b58ec6e564",
    "scope": "local",
    "enabled": false,
    "installedAt": "2026-02-05T15:37:39.555Z",
    "lastUpdated": "2026-03-02T18:10:00.820Z",
    "projectPath": "C:\\work"
  }
]
"#;

        let parsed = serde_json::from_str::<Vec<InstalledPluginJson>>(json).expect("parse json");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "frontend-design@claude-plugins-official");
        assert_eq!(parsed[0].scope, "local");
        assert!(!parsed[0].enabled);
        assert_eq!(parsed[0].project_path.as_deref(), Some("C:\\work"));
    }

    #[test]
    fn detects_mcp_plugins_from_installed_payload() {
        let json = r#"
[
  {
    "id": "supabase@claude-plugins-official",
    "scope": "local",
    "enabled": true,
    "mcpServers": {
      "supabase": {
        "type": "http",
        "url": "https://mcp.supabase.com/mcp"
      }
    }
  }
]
"#;

        let parsed = serde_json::from_str::<Vec<InstalledPluginJson>>(json).expect("parse json");
        let entry = installed_entry_from_json(parsed.into_iter().next().expect("entry"));

        assert_eq!(entry.mcp_server_names, vec!["supabase"]);
    }

    #[test]
    fn parses_marketplace_entries_and_sources() {
        let available_json = r#"
{
  "installed": [],
  "available": [
    {
      "pluginId": "frontend-design@claude-plugins-official",
      "name": "frontend-design",
      "description": "Create distinctive interfaces",
      "marketplaceName": "claude-plugins-official",
      "version": "1.0.0",
      "source": "./plugins/frontend-design",
      "installCount": 42
    }
  ]
}
"#;
        let source_json = r#"
[
  {
    "name": "claude-plugins-official",
    "source": "github",
    "repo": "anthropics/claude-plugins-official"
  }
]
"#;

        let parsed_available =
            serde_json::from_str::<MarketplaceListJson>(available_json).expect("parse available");
        let parsed_sources =
            serde_json::from_str::<Vec<MarketplaceSourceJson>>(source_json).expect("parse sources");

        assert_eq!(parsed_available.available.len(), 1);
        assert_eq!(
            parsed_available.available[0].marketplace_name.as_deref(),
            Some("claude-plugins-official")
        );
        assert_eq!(parsed_available.available[0].install_count, Some(42));
        assert_eq!(parsed_sources[0].repo.as_deref(), Some("anthropics/claude-plugins-official"));
    }
}
