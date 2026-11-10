// SPDX-License-Identifier: Apache-2.0
use super::*;

fn plugin_surface(app: &mut App) {
    app.surface_mode = crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Config);
    app.config.active_tab = crate::app::ConfigTab::Plugins;
}

#[tokio::test]
async fn eligible_inventory_waits_for_explicit_refresh_after_inner_tab_navigation() {
    let fixture = InventoryCli::new();
    let (mut app, mut commands) = app_with_connection();
    fixture.attach(&mut app);
    plugin_surface(&mut app);
    settle(&mut app, request_inventory_refresh_if_needed).await;
    let initial = fixture.calls();
    assert!(!initial.is_empty());
    // The cache is eligible for refresh; a missing runtime or a fresh TTL cannot
    // explain the absence of reads after the real keyboard dispatch.
    app.plugins.last_inventory_refresh_at = None;
    fixture.write("installed.json", &fixture.read("installed-after.json"));
    settle(&mut app, |app| {
        for code in [
            KeyCode::Right,
            KeyCode::Right,
            KeyCode::Right,
            KeyCode::Left,
            KeyCode::Left,
            KeyCode::Left,
        ] {
            press(app, code);
        }
    })
    .await;
    assert_eq!(fixture.calls(), initial);
    assert!(app.plugins.installed.iter().any(|p| p.id == "alpha@fixture" && p.enabled));
    settle(&mut app, |app| press(app, KeyCode::Char('r'))).await;
    assert_eq!(app.plugins.installed.len(), 1);
    assert_eq!(
        (
            &*app.plugins.installed[0].id,
            app.plugins.installed[0].enabled,
            app.plugins.installed[0].version.as_deref()
        ),
        ("alpha@fixture", false, Some("2.8"))
    );
    assert_eq!(&fixture.calls()[initial.len()..], &initial);
    assert!(matches!(commands.try_recv().unwrap().command, BridgeCommand::ReloadPlugins { .. }));
    assert!(commands.try_recv().is_err());
}

#[tokio::test]
async fn invalid_marketplace_source_stays_local_then_corrected_input_reaches_the_cli() {
    for draft in ["", "   "] {
        let fixture = InventoryCli::new();
        let (mut app, mut commands) = app_with_connection();
        fixture.attach(&mut app);
        plugin_surface(&mut app);
        app.plugins.active_tab = PluginsViewTab::Marketplace;
        let sources = r#"[{"name":"fixture","source":"github","repo":"team/plugins"},{"name":"other"},{"name":"new-market","source":"github","repo":"team/new-market"}]"#;
        fixture.write("sources-after.json", sources);
        let original = fixture.read("sources.json");
        settle(&mut app, |app| {
            press(app, KeyCode::Enter);
            crate::app::handle_terminal_event(app, Event::Paste(draft.into()));
            press(app, KeyCode::Enter);
        })
        .await;
        assert!(fixture.calls().is_empty());
        assert_eq!(fixture.read("sources.json"), original);
        assert!(app.config.plugin_source_input_overlay().is_some());
        assert_eq!(
            app.config.overlay_message.as_ref().unwrap().kind,
            crate::app::config::OverlayMessageKind::Error
        );
        assert!(app.config.last_error.is_none());
        assert!(commands.try_recv().is_err());
        settle(&mut app, |app| {
            crate::app::handle_terminal_event(app, Event::Paste("team/new-market".into()));
            press(app, KeyCode::Enter);
        })
        .await;
        assert_eq!(fixture.read("sources.json"), sources);
        assert_eq!(
            fixture.calls()[0],
            vec!["plugin", "marketplace", "add", "team/new-market", "--scope", "user"]
        );
        assert_eq!(
            app.plugins.marketplaces.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["fixture", "new-market", "other"]
        );
        assert!(matches!(
            commands.try_recv().unwrap().command,
            BridgeCommand::ReloadPlugins { .. }
        ));
        assert!(app.config.overlay.is_none());
    }
}
