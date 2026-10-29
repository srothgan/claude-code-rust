// SPDX-License-Identifier: Apache-2.0

use super::chat_terminal::{TerminalIo, test_io::TestIo};
use super::modes::{release_to_child_actions, return_from_child_actions};
use super::*;
use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Independent terminal modes, not mutually exclusive states.
struct ExternalState {
    raw: bool,
    focus: bool,
    wrap: bool,
    visible: bool,
    bar: bool,
    alternate: bool,
    keyboard_depth: i32,
}
impl Default for ExternalState {
    fn default() -> Self {
        Self {
            raw: false,
            focus: false,
            wrap: true,
            visible: true,
            bar: false,
            alternate: false,
            keyboard_depth: 0,
        }
    }
}
#[derive(Default)]
struct Observation {
    state: ExternalState,
    attempts: Vec<String>,
    failures: BTreeMap<String, usize>,
    fail_backend: bool,
    fail_writer: bool,
}
#[derive(Clone, Default)]
struct ObservedIo(Arc<Mutex<Observation>>);
impl ObservedIo {
    fn fail(&self, operation: &str, times: usize) {
        self.0.lock().unwrap().failures.insert(operation.to_owned(), times);
    }
    fn observe(&self, operation: &str) -> io::Result<()> {
        let mut observation = self.0.lock().unwrap();
        observation.attempts.push(operation.to_owned());
        if let Some(left) = observation.failures.get_mut(operation)
            && *left > 0
        {
            *left -= 1;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("fault {operation:?}"),
            ));
        }
        if observation.fail_writer && operation.starts_with('\x1b') {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "persistent writer failure"));
        }
        let state = &mut observation.state;
        match operation {
            "raw:on" => state.raw = true,
            "raw:off" => state.raw = false,
            "\x1b[?1004h" => state.focus = true,
            "\x1b[?1004l" => state.focus = false,
            "\x1b[?7h" => state.wrap = true,
            "\x1b[?7l" => state.wrap = false,
            "\x1b[?25h" => state.visible = true,
            "\x1b[?25l" => state.visible = false,
            "\x1b[5 q" => state.bar = true,
            "\x1b[0 q" => state.bar = false,
            "\x1b[?1049h" => state.alternate = true,
            "\x1b[?1049l" => state.alternate = false,
            "\x1b[>1u" => state.keyboard_depth += 1,
            "\x1b[<1u" => state.keyboard_depth -= 1,
            "size" | "cursor" | "backend" => {}
            other => panic!("unexpected terminal protocol: {other:?}"),
        }
        Ok(())
    }
    fn state(&self) -> ExternalState {
        self.0.lock().unwrap().state.clone()
    }
    fn attempts(&self) -> Vec<String> {
        self.0.lock().unwrap().attempts.clone()
    }
    fn shell(&self) {
        assert_eq!(self.state(), ExternalState::default());
    }
    fn chat(&self) {
        assert_eq!(
            self.state(),
            ExternalState {
                raw: true,
                focus: true,
                wrap: false,
                visible: true,
                bar: true,
                alternate: false,
                keyboard_depth: i32::from(cfg!(target_os = "macos")),
            }
        );
    }
}
impl ModeIo for ObservedIo {
    fn raw_mode(&self, enabled: bool) -> io::Result<()> {
        self.observe(if enabled { "raw:on" } else { "raw:off" })
    }
    fn command(&self, command: impl crossterm::Command) -> io::Result<()> {
        let mut bytes = String::new();
        command.write_ansi(&mut bytes).unwrap();
        self.observe(&bytes)
    }
}
impl RuntimeIo for ObservedIo {
    type FullscreenBackend = <TestIo as TerminalIo>::Backend;
    fn size(&self) -> io::Result<(u16, u16)> {
        self.observe("size")?;
        Ok((80, 24))
    }
    fn cursor_before_input_reader(&self) -> io::Result<(u16, u16)> {
        self.observe("cursor")?;
        Ok((0, 3))
    }
    fn fullscreen_backend(&self) -> Self::FullscreenBackend {
        self.observe("backend").unwrap();
        let backend = TestIo::new(80, 24);
        if self.0.lock().unwrap().fail_backend {
            backend.fail_on(Some("size"));
        }
        backend.backend()
    }
}

#[test]
fn lifecycle_recipes_apply_terminal_effects_and_preserve_screen_ownership() {
    let io = ObservedIo::default();
    io.0.lock().unwrap().state.visible = false;
    let modes = TerminalModes::new(io.clone());
    modes.apply(chat_startup_actions()).unwrap();
    io.chat();
    assert!(!io.attempts().iter().any(|s| s.contains("1049")));
    modes.apply(enter_fullscreen_actions()).unwrap();
    let full = io.state();
    assert!(full.alternate && full.raw && full.focus && full.wrap);
    assert!(!full.visible && !full.bar);
    let start = io.attempts().len();
    modes.exit_fullscreen().unwrap();
    io.chat();
    let operations = io.attempts();
    let returned = &operations[start..];
    assert!(
        returned.iter().position(|s| s == "\x1b[?1049l")
            < returned.iter().position(|s| s == "\x1b[5 q")
    );
    let start = operations.len();
    modes.apply(release_to_child_actions()).unwrap();
    io.shell();
    modes.apply(return_from_child_actions()).unwrap();
    io.chat();
    assert!(!io.attempts()[start..].iter().any(|s| s.contains("1049")));
    modes.restore().unwrap();
    io.shell();
    let operations = io.attempts();
    assert!(!modes.restore().unwrap());
    assert_eq!(io.attempts(), operations, "successful restoration is idempotent");
    #[cfg(target_os = "macos")]
    {
        assert_eq!(operations.iter().filter(|s| *s == "\x1b[>1u").count(), 2);
        assert_eq!(operations.iter().filter(|s| *s == "\x1b[<1u").count(), 2);
    }
}

fn isolated(name: &str, body: impl FnOnce()) {
    const KEY: &str = "CLAUDE_TERMINAL_LIFECYCLE_AUDIT_CASE";
    if std::env::var(KEY).ok().as_deref() == Some(name) {
        body();
        println!("AUDIT_CASE_OK:{name}");
        return;
    }
    let test = format!("app::terminal_runtime::lifecycle_audit_tests::{name}");
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test, "--nocapture", "--test-threads=1"])
        .env(KEY, name)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = result;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("isolated audit timed out: {}", String::from_utf8_lossy(&output.stdout));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(result.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&result.stderr));
    assert!(stdout.contains("running 1 test"));
    assert_eq!(stdout.matches(&format!("AUDIT_CASE_OK:{name}")).count(), 1);
}
fn fixture_app() -> App {
    let mut app = App::test_default();
    app.input.set_text("UNCHANGED_DRAFT");
    app.push_message_tracked(crate::app::ChatMessage::new(
        crate::app::MessageRole::User,
        vec![crate::app::MessageBlock::Text(crate::app::TextBlock::from_complete(
            "CANONICAL_HISTORY",
        ))],
        None,
    ));
    app
}
fn source_preserved(app: &App) {
    assert_eq!(app.input.text(), "UNCHANGED_DRAFT");
    assert!(app.transcript.messages.iter().any(|m| {
        m.blocks
            .iter()
            .any(|b| matches!(b,crate::app::MessageBlock::Text(t) if t.text == "CANONICAL_HISTORY"))
    }));
}
fn boot(app: &mut App, io: ObservedIo) -> anyhow::Result<TerminalRuntime<ObservedIo>> {
    TerminalRuntime::bootstrap_inner(app, BootstrapSeedMode::MeasureBeforeInputReader, io)
}

fn assert_failed_acquisition_cleans_up() {
    for fullscreen in [false, true] {
        for failure in ["raw:on", "size", "cursor"] {
            let io = ObservedIo::default();
            io.fail(failure, 1);
            if failure == "raw:on" {
                io.fail("\x1b[?1004h", 1);
            }
            let mut app = fixture_app();
            if fullscreen {
                app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
            }
            let error = boot(&mut app, io.clone()).err().unwrap();
            let cause = error.downcast_ref::<io::Error>().unwrap();
            assert_eq!(cause.kind(), io::ErrorKind::PermissionDenied);
            assert!(cause.to_string().contains(failure), "{error:#}");
            io.shell();
            source_preserved(&app);
            assert!(io.attempts().iter().any(|s| s == "raw:off"));
        }
    }
    // Conservative resume obtains dimensions without querying the input reader's cursor.
    let io = ObservedIo::default();
    io.fail("size", 1);
    let mut app = fixture_app();
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
    assert!(
        TerminalRuntime::bootstrap_inner(
            &mut app,
            BootstrapSeedMode::ConservativeAfterResume,
            io.clone()
        )
        .is_err()
    );
    io.shell();
    assert!(!io.attempts().iter().any(|s| s == "cursor"));
    for construction_failure in [false, true] {
        for initial_fullscreen in [false, true] {
            let io = ObservedIo::default();
            let mut app = fixture_app();
            if initial_fullscreen {
                app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
            }
            if construction_failure {
                io.0.lock().unwrap().fail_backend = true;
            } else {
                io.fail("\x1b[?25l", 1);
            }
            if initial_fullscreen {
                assert!(boot(&mut app, io.clone()).is_err());
            } else {
                let mut runtime = boot(&mut app, io.clone()).unwrap();
                app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
                assert!(runtime.sync_surface(&mut app).is_err());
                assert!(io.attempts().iter().any(|s| s == "\x1b[?1049h"));
                runtime.restore(&mut app);
            }
            io.shell();
            source_preserved(&app);
            assert!(io.attempts().iter().any(|s| s == "\x1b[?1049l"));
        }
    }
}

#[test]
fn runtime_acquisition_and_transition_errors_restore_acquired_modes() {
    isolated("runtime_acquisition_and_transition_errors_restore_acquired_modes", || {
        assert_failed_acquisition_cleans_up();
        for persistent in [false, true] {
            let io = ObservedIo::default();
            let mut app = fixture_app();
            app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
            let mut runtime = boot(&mut app, io.clone()).unwrap();
            assert!(io.state().alternate);
            io.fail("\x1b[?1049l", if persistent { usize::MAX } else { 1 });
            app.surface_mode = SurfaceMode::Chat;
            let error = runtime.sync_surface(&mut app).unwrap_err();
            assert_eq!(
                error.downcast_ref::<io::Error>().unwrap().kind(),
                io::ErrorKind::PermissionDenied
            );
            runtime.restore(&mut app);
            let operations = io.attempts();
            assert!(operations.iter().filter(|s| *s == "\x1b[?1049l").count() >= 2);
            if persistent {
                assert!(runtime.modes.restore().is_err());
                let state = io.state();
                assert!(!state.raw && !state.focus && state.wrap && state.visible && !state.bar);
                io.fail("\x1b[?1049l", 0);
                runtime.restore(&mut app);
            }
            io.shell();
            source_preserved(&app);
            let done = io.attempts();
            runtime.restore(&mut app);
            assert_eq!(io.attempts(), done);
        }
        // A broken output stream must not prevent the independent raw-mode cleanup.
        let io = ObservedIo::default();
        let mut app = fixture_app();
        app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
        let mut runtime = boot(&mut app, io.clone()).unwrap();
        io.0.lock().unwrap().fail_writer = true;
        let error = runtime.modes.restore().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(!io.state().raw);
        assert!(io.state().alternate);
        assert!(io.attempts().iter().any(|s| s == "\x1b[?1004l"));
        io.0.lock().unwrap().fail_writer = false;
        runtime.restore(&mut app);
        io.shell();
        source_preserved(&app);
        // A failed cleanup itself remains retryable, even after successful later actions.
        let io = ObservedIo::default();
        let mut app = fixture_app();
        let mut runtime = boot(&mut app, io.clone()).unwrap();
        io.fail("raw:off", 1);
        runtime.restore(&mut app);
        assert!(io.state().raw);
        runtime.restore(&mut app);
        io.shell();
    });
}

#[test]
fn runtime_panic_restores_terminal_before_reporting_original_payload() {
    isolated("runtime_panic_restores_terminal_before_reporting_original_payload", || {
        for fullscreen in [false, true] {
            for clean_first in [false, true] {
                let original = std::panic::take_hook();
                let io = ObservedIo::default();
                let observed = Arc::new(Mutex::new(Vec::new()));
                let report_sink = Arc::clone(&observed);
                let external = io.clone();
                std::panic::set_hook(Box::new(move |info| {
                    report_sink.lock().unwrap().push((
                        info.payload().downcast_ref::<&str>().copied().unwrap_or("wrong payload"),
                        external.state(),
                    ));
                }));
                let mut app = fixture_app();
                let mut runtime = boot(&mut app, io.clone()).unwrap();
                if fullscreen {
                    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);
                    runtime.sync_surface(&mut app).unwrap();
                    assert!(io.state().alternate);
                } else {
                    io.chat();
                }
                if clean_first {
                    runtime.restore(&mut app);
                }
                let prior = io.attempts();
                let caught = std::panic::catch_unwind(|| panic!("runtime-safe-payload"));
                assert!(caught.is_err());
                let reports = observed.lock().unwrap().clone();
                if reports != [("runtime-safe-payload", ExternalState::default())] {
                    eprintln!("restoration before diagnostic assertion: {reports:?}");
                }
                assert_eq!(reports, [("runtime-safe-payload", ExternalState::default())]);
                io.shell();
                if clean_first {
                    assert_eq!(io.attempts(), prior);
                }
                let restored = io.attempts();
                runtime.restore(&mut app);
                drop(runtime);
                assert_eq!(io.attempts(), restored);
                std::panic::set_hook(original);
            }
        }
    });
}

fn recorded(events: &Mutex<Vec<&'static str>>) -> Vec<&'static str> {
    events.lock().unwrap().clone()
}

#[test]
fn panic_hook_orders_reporting_and_survives_owner_unwinding() {
    isolated("panic_hook_orders_reporting_and_survives_owner_unwinding", || {
        use std::sync::atomic::AtomicBool;
        for pre_restored in [false, true] {
            let original = std::panic::take_hook();
            let events = Arc::new(Mutex::new(Vec::new()));
            let previous = Arc::clone(&events);
            std::panic::set_hook(Box::new(move |info| {
                previous.lock().unwrap().push(
                    info.payload().downcast_ref::<&str>().copied().unwrap_or("unexpected payload"),
                );
            }));
            let gate = Arc::new(AtomicBool::new(false));
            if pre_restored {
                super::panic_hook::restore_once(&gate, || events.lock().unwrap().push("restore"));
            }
            let restore = Arc::clone(&events);
            let shared = Arc::clone(&gate);
            let hook = PanicRestoreHook::install(move || {
                super::panic_hook::restore_once(&shared, || {
                    restore.lock().unwrap().push("restore");
                });
            });
            for payload in ["first", "second"] {
                assert!(std::panic::catch_unwind(|| std::panic::panic_any(payload)).is_err());
            }
            assert_eq!(recorded(&events), ["restore", "first", "second"]);
            drop(hook);
            assert!(std::panic::catch_unwind(|| panic!("after drop")).is_err());
            assert_eq!(recorded(&events), ["restore", "first", "second", "after drop"]);
            std::panic::set_hook(original);
        }
        let original = std::panic::take_hook();
        let events = Arc::new(Mutex::new(Vec::new()));
        let previous = Arc::clone(&events);
        std::panic::set_hook(Box::new(move |info| {
            previous.lock().unwrap().push(
                info.payload().downcast_ref::<&str>().copied().unwrap_or("unexpected payload"),
            );
        }));
        let restore = Arc::clone(&events);
        assert!(
            std::panic::catch_unwind(move || {
                let _hook =
                    PanicRestoreHook::install(move || restore.lock().unwrap().push("restore"));
                panic!("owner unwind");
            })
            .is_err()
        );
        assert_eq!(recorded(&events), ["restore", "owner unwind"]);
        // Drop intentionally cannot mutate a hook during unwinding; the child owns cleanup.
        std::panic::set_hook(original);
    });
}

#[test]
fn fullscreen_frames_retarget_populated_diff_to_editable_form_without_stale_content() {
    use super::fullscreen_session::draw_fullscreen_surface_frame;
    use crate::agent::{events::ClientEvent, types};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, layout::Rect};
    fn region(buffer: &Buffer, area: Rect) -> Vec<String> {
        (area.y..area.bottom())
            .map(|y| (area.x..area.right()).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }
    let directory = tempfile::tempdir().unwrap();
    let initialized = std::process::Command::new("git")
        .current_dir(directory.path())
        .args(["init", "--template=", "-q"])
        .output()
        .unwrap();
    assert!(initialized.status.success());
    std::fs::write(directory.path().join("selected.txt"), "OLD_DIFF_BODY\n").unwrap();
    let mut app = fixture_app();
    app.cwd_raw = directory.path().to_string_lossy().into_owned();
    app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("render-session"));
    crate::app::diff_view::open(&mut app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        crate::app::diff_view::poll(&mut app);
        if app.diff_view.as_ref().unwrap().content.is_some() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "diff fixture never became readable");
        assert!(
            app.diff_view.as_ref().unwrap().error.is_none(),
            "{:?}",
            app.diff_view.as_ref().unwrap().error
        );
        std::thread::yield_now();
    }
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    draw_fullscreen_surface_frame(&mut terminal, &mut app).unwrap();
    let before = terminal.backend().buffer();
    assert!(region(before, Rect::new(0, 0, 100, 3)).join(" ").contains("Git diff"));
    assert!(region(before, Rect::new(0, 3, 30, 33)).join(" ").contains("selected.txt"));
    assert!(region(before, Rect::new(30, 3, 70, 33)).join(" ").contains("OLD_DIFF_BODY"));
    assert!(region(before, Rect::new(0, 36, 100, 4)).join(" ").contains("refresh"));
    crate::app::events::handle_client_event(
        &mut app,
        ClientEvent::McpElicitationRequest {
            session_id: "render-session".into(),
            request: types::ElicitationRequest {
                request_id: "retarget-form".into(),
                server_name: "destination-server".into(),
                message: "NEW_FORM_BODY".into(),
                mode: types::ElicitationMode::Form,
                url: None,
                elicitation_id: None,
                field_order: vec!["destination".into(), "tenant".into()],
                requested_schema: Some(serde_json::json!({"type":"object","properties":{
                "destination":{"type":"string","default":"DESTINATION_VALUE"},
                "tenant":{"type":"string","default":"TENANT_VALUE"}},"required":["destination","tenant"]})),
            },
        },
    );
    for field in ["destination", "tenant"] {
        draw_fullscreen_surface_frame(&mut terminal, &mut app).unwrap();
        let buffer = terminal.backend().buffer();
        let popup = region(buffer, Rect::new(5, 4, 90, 32));
        assert!(popup.iter().any(|r| r.contains("MCP request")));
        assert!(popup.iter().any(|r| r.contains("NEW_FORM_BODY")));
        for (name, value) in [("destination", "DESTINATION_VALUE"), ("tenant", "TENANT_VALUE")] {
            let row = popup.iter().position(|r| r.contains(&format!("{name} (required)"))).unwrap();
            assert_eq!(popup[row].contains('\u{203a}'), name == field);
            assert!(popup[row + 1].contains(value));
        }
        assert!(popup.iter().any(|r| r.contains("Accept")));
        assert!(popup.iter().any(|r| r.contains("Decline")));
        assert!(region(buffer, Rect::new(5, 30, 90, 6)).join(" ").contains("Esc"));
        let all = region(buffer, Rect::new(0, 0, 100, 40)).join(" ");
        assert!(
            !all.contains("OLD_DIFF_BODY")
                && !all.contains("selected.txt")
                && !all.contains("Git diff")
        );
        crate::app::events::handle_terminal_event(
            &mut app,
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Tab,
                crossterm::event::KeyModifiers::NONE,
            )),
        );
    }
}
