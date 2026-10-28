// SPDX-License-Identifier: Apache-2.0

use crate::app::{App, handle_client_event};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(super) struct InventoryCli {
    pub project: tempfile::TempDir,
    pub executable: PathBuf,
}

impl InventoryCli {
    pub fn new() -> Self {
        static EXECUTABLE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
        let (_, executable) = EXECUTABLE.get_or_init(|| {
            let dir = tempfile::tempdir().expect("fixture executable directory");
            let exe = dir.path().join(format!("inventory-cli{}", std::env::consts::EXE_SUFFIX));
            let output = std::process::Command::new("rustc")
                .args(["--edition=2024", "-Dwarnings"])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/plugin-inventory-cli.rs"),
                )
                .arg("-o")
                .arg(&exe)
                .output()
                .expect("compile isolated inventory CLI");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            (dir, exe)
        });
        let fixture = Self {
            project: tempfile::tempdir().expect("isolated plugin registry"),
            executable: executable.clone(),
        };
        fixture.write("installed.json", r#"[
            {"id":"alpha@fixture","scope":"user","enabled":true,"version":"2.7","installedAt":"2026-01-02","lastUpdated":"2026-02-03","mcpServers":{"search":{"ignored":"payload"},"fetch":{}}},
            {"id":"beta@other","scope":"project","enabled":false,"projectPath":"/fixture/project"}
        ]"#);
        fixture.write(
            "installed-after.json",
            r#"[{"id":"alpha@fixture","scope":"user","enabled":false,"version":"2.8"}]"#,
        );
        fixture.write("available.json", r#"{"available":[
            {"pluginId":"alpha@fixture","name":"Alpha browser","description":"Browse documents","marketplaceName":"fixture","version":"3.4","installCount":42,"source":{"path":"./alpha"}},
            {"pluginId":"gamma@other","name":"Gamma"}
        ]}"#);
        fixture.write(
            "sources.json",
            r#"[{"name":"fixture","source":"github","repo":"team/plugins"},{"name":"other"}]"#,
        );
        fixture
    }

    pub fn cwd(&self) -> String {
        self.project.path().to_str().expect("fixture cwd").to_owned()
    }

    pub fn write(&self, name: &str, text: &str) {
        std::fs::write(self.project.path().join(name), text).expect("write fixture input");
    }

    pub fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.project.path().join(name)).expect("read fixture output")
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        let journal =
            std::fs::read_to_string(self.project.path().join("journal")).unwrap_or_default();
        journal
            .lines()
            .map(|line| {
                let (cwd, argv) = line.split_once('\t').expect("journal cwd and argv");
                assert_eq!(Path::new(cwd), self.project.path(), "command cwd");
                argv.split('\t').map(str::to_owned).collect()
            })
            .collect()
    }

    pub fn attach(&self, app: &mut App) {
        app.cwd_raw = self.cwd();
        app.plugins.claude_path = Some(self.executable.clone());
    }
}

// Await every producer created by the action before asserting absence of extra work.
pub(super) async fn settle(app: &mut App, action: impl FnOnce(&mut App)) {
    let tasks = tokio::task::LocalSet::new();
    tasks
        .run_until(async {
            action(app);
        })
        .await;
    tokio::time::timeout(std::time::Duration::from_secs(15), tasks)
        .await
        .expect("fixture producers complete");
    while let Ok(event) = app.event_rx.try_recv() {
        handle_client_event(app, event);
    }
}

pub(super) fn assert_fixture_snapshot(snapshot: &super::PluginsInventorySnapshot) {
    use super::{InstalledPluginEntry, MarketplaceEntry, MarketplaceSourceEntry};
    use std::collections::BTreeMap;
    let installed = [
        InstalledPluginEntry {
            id: "alpha@fixture".into(),
            version: Some("2.7".into()),
            scope: "user".into(),
            enabled: true,
            installed_at: Some("2026-01-02".into()),
            last_updated: Some("2026-02-03".into()),
            project_path: None,
            mcp_server_names: vec!["fetch".into(), "search".into()],
        },
        InstalledPluginEntry {
            id: "beta@other".into(),
            version: None,
            scope: "project".into(),
            enabled: false,
            installed_at: None,
            last_updated: None,
            project_path: Some("/fixture/project".into()),
            mcp_server_names: vec![],
        },
    ];
    let mut installed_actual = snapshot.installed.clone();
    for entry in &mut installed_actual {
        entry.mcp_server_names.sort();
    }
    assert_eq!(
        installed_actual.iter().map(|p| (&p.id, p)).collect::<BTreeMap<_, _>>(),
        installed.iter().map(|p| (&p.id, p)).collect::<BTreeMap<_, _>>()
    );
    let available = [
        MarketplaceEntry {
            plugin_id: "alpha@fixture".into(),
            name: "Alpha browser".into(),
            description: Some("Browse documents".into()),
            marketplace_name: Some("fixture".into()),
            version: Some("3.4".into()),
            install_count: Some(42),
            source: Some(serde_json::json!({"path":"./alpha"})),
        },
        MarketplaceEntry {
            plugin_id: "gamma@other".into(),
            name: "Gamma".into(),
            description: None,
            marketplace_name: None,
            version: None,
            install_count: None,
            source: None,
        },
    ];
    assert_eq!(
        snapshot.marketplace.iter().map(|p| (&p.plugin_id, p)).collect::<BTreeMap<_, _>>(),
        available.iter().map(|p| (&p.plugin_id, p)).collect::<BTreeMap<_, _>>()
    );
    let sources = [
        MarketplaceSourceEntry {
            name: "fixture".into(),
            source: Some("github".into()),
            repo: Some("team/plugins".into()),
        },
        MarketplaceSourceEntry { name: "other".into(), source: None, repo: None },
    ];
    assert_eq!(
        snapshot.marketplaces.iter().map(|p| (&p.name, p)).collect::<BTreeMap<_, _>>(),
        sources.iter().map(|p| (&p.name, p)).collect::<BTreeMap<_, _>>()
    );
}
