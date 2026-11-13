// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::agent::settings::SettingsSnapshot;
use crate::app::App;

#[derive(Default)]
struct Sink {
    bytes: Vec<u8>,
    flushed: bool,
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushed = true;
        Ok(())
    }
}
fn emitted(app: &App) -> String {
    let mut sink = Sink::default();
    update_to(app, &mut sink);
    assert!(sink.flushed);
    String::from_utf8_lossy(&sink.bytes).into_owned()
}
fn setting(app: &mut App, name: &str, value: bool) {
    let snapshot = app.config.snapshot.get_or_insert_with(SettingsSnapshot::default);
    snapshot.values.retain(|entry| entry.id != name);
    snapshot.values.extend(SettingsSnapshot::test_value(name, serde_json::json!(value)).values);
}

#[test]
fn emitted_title_projects_effective_status_and_motion_preferences() {
    let mut app = App::test_default();
    app.cwd_raw = "/projects/Orbit Lab".into();
    setting(&mut app, "presentation.showStatusInTerminalTab", true);
    app.start_assistant_response();
    let mut moving = std::collections::BTreeSet::new();
    for frame in 0..80 {
        app.spinner_frame = frame;
        moving.insert(emitted(&app));
    }
    assert!(moving.len() > 1, "ordinary activity can animate");
    for state in [AppStatus::Connecting, AppStatus::CommandPending, AppStatus::Running] {
        app.status = state;
        setting(&mut app, "prefersReducedMotion", true);
        let fixed = emitted(&app);
        for frame in [0, 19, 77] {
            app.spinner_frame = frame;
            assert_eq!(emitted(&app), fixed);
        }
        for idle in [AppStatus::Ready, AppStatus::Error] {
            app.status = idle;
            let output = emitted(&app);
            assert!(output.starts_with("\x1b]2;") && output.ends_with(" Orbit Lab\x07"));
            assert_ne!(output, fixed);
        }
    }
    for state in [AppStatus::Running, AppStatus::Ready, AppStatus::Error] {
        app.status = state;
        setting(&mut app, "presentation.showStatusInTerminalTab", false);
        assert_eq!(emitted(&app), "\x1b]2;Orbit Lab\x07");
    }
    app.config.snapshot = None;
    app.status = AppStatus::Running;
    let reset: std::collections::BTreeSet<_> = (0..80)
        .map(|frame| {
            app.spinner_frame = frame;
            emitted(&app)
        })
        .collect();
    assert_eq!(reset, moving);
}

#[test]
fn terminal_label_preserves_distinct_leaf_names_and_root_fallbacks() {
    for (path, label) in [
        ("/work/Orbit", "Orbit"),
        ("C:\\work\\Lunar\\", "Lunar"),
        ("plain", "plain"),
        ("/work/夜 空/", "夜 空"),
        ("", "claude_rust"),
        ("///", "claude_rust"),
        ("\\\\", "claude_rust"),
    ] {
        assert_eq!(folder_name(path), label);
    }
}

#[test]
fn active_and_restored_titles_cannot_inject_payload_controls() {
    for (path, payload) in [
        ("/work/夜 空", "夜 空"),
        ("/work/Orb\x07\x1b]9;injected\u{9c}\n\r\tLab", "Orb]9;injected   Lab"),
    ] {
        let mut app = App::test_default();
        app.cwd_raw = path.into();
        setting(&mut app, "presentation.showStatusInTerminalTab", false);
        assert_eq!(emitted(&app), format!("\x1b]2;{payload}\x07"));
        let mut sink = Sink::default();
        restore_to(path, &mut sink);
        assert!(sink.flushed);
        assert_eq!(sink.bytes, format!("\x1b]2;{payload}\x07").as_bytes());
    }
}
