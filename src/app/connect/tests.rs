use super::type_converters::map_session_update;
use crate::Cli;
use crate::agent::model;
use crate::agent::types;
use crate::app::{FullscreenView, SurfaceMode, TerminalLifecycleState};

#[test]
fn map_session_update_preserves_config_option_update() {
    let mapped = map_session_update(types::SessionUpdate::ConfigOptionUpdate {
        option_id: "model".to_owned(),
        value: serde_json::Value::String("sonnet".to_owned()),
    });

    let Some(model::SessionUpdate::ConfigOptionUpdate(cfg)) = mapped else {
        panic!("expected ConfigOptionUpdate mapping");
    };
    assert_eq!(cfg.option_id, "model");
    assert_eq!(cfg.value, serde_json::Value::String("sonnet".to_owned()));
}

#[test]
fn create_app_prewarms_file_index_and_routes_untrusted_cwd_to_trust_surface() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cli = Cli {
        command: None,
        no_update_check: true,
        dir: Some(dir.path().to_path_buf()),
        bridge_script: None,
        enable_logs: false,
        diagnostics_preset: None,
        log_file: None,
        log_filter: None,
        log_append: false,
        enable_perf: false,
        perf_log: None,
        perf_append: false,
    };

    let app = super::create_app(&cli);

    assert_eq!(app.file_index.root.as_deref(), Some(dir.path()));
    assert!(app.file_index.scan.is_some());
    assert!(app.file_index.watch.is_some());
    assert_eq!(app.surface_mode, SurfaceMode::Fullscreen(FullscreenView::Trusted));
    assert_eq!(app.terminal_lifecycle, TerminalLifecycleState::Bootstrapping);
}
