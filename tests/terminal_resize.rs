// SPDX-License-Identifier: Apache-2.0
// Real TUI/PTY regression tests with a scripted NDJSON peer. These exercise
// terminal rendering and input without an SDK, subscription, or model call.

use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const COLS: u16 = 87;
const SHORT_ROWS: u16 = 38;
const TALL_ROWS: u16 = 55;
const RESIZES: u32 = 200;
const SCROLLBACK_ROWS: usize = 10_000;
const OWNED_REGION_ERROR: &str = "refusing to clear outside inline chat owned region";
// A liveness bound for outcomes that must happen. It only limits how long a
// broken build takes to fail, so it is sized for a loaded CI runner.
const WAIT_LIMIT: Duration = Duration::from_secs(60);
type PtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;

struct CapturedTerminal {
    parser: vt100::Parser<ScrollbackCallbacks>,
    raw: Vec<u8>,
    ended: bool,
}

struct ScrollbackCallbacks;

impl vt100::Callbacks for ScrollbackCallbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first_intermediate: Option<u8>,
        second_intermediate: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        // vt100 handles ED 0/1/2 but not ED 3 (erase saved lines). Preserve the
        // visible screen and modes while discarding scrollback, as a terminal does.
        if command == 'J'
            && first_intermediate.is_none()
            && second_intermediate.is_none()
            && params == [&[3][..]]
        {
            let (rows, cols) = screen.size();
            let mut cleared = vt100::Parser::new(rows, cols, SCROLLBACK_ROWS);
            cleared.process(&screen.state_formatted());
            *screen = cleared.screen().clone();
        }
    }
}

struct TestChild {
    process: Box<dyn Child + Send + Sync>,
    writer: PtyWriter,
}

#[allow(clippy::expect_used)]
impl TestChild {
    fn wait_for_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Some(status) = self.process.try_wait().expect("poll claude-rs") {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for TestChild {
    fn drop(&mut self) {
        if matches!(self.process.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.write_all(b"\x11"); // Ctrl+Q
            let _ = writer.flush();
        }
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            if matches!(self.process.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Only stop the child owned by this test if graceful cleanup fails.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }
}

fn tail(text: &str, chars: usize) -> String {
    let skip = text.chars().count().saturating_sub(chars);
    text.chars().skip(skip).collect::<String>().replace('\x1b', "<ESC>")
}

#[allow(clippy::expect_used)]
fn capture_output(
    mut reader: Box<dyn Read + Send>,
    writer: PtyWriter,
) -> Arc<Mutex<CapturedTerminal>> {
    let output = Arc::new(Mutex::new(CapturedTerminal {
        parser: vt100::Parser::new_with_callbacks(
            SHORT_ROWS,
            COLS,
            SCROLLBACK_ROWS,
            ScrollbackCallbacks,
        ),
        raw: Vec::new(),
        ended: false,
    }));
    let captured = Arc::clone(&output);
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        let mut query = Vec::new();
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            // Queries and UTF-8 characters may be split across reads.
            query.extend_from_slice(&buffer[..read]);
            let mut captured = captured.lock().expect("output lock");
            captured.parser.process(&buffer[..read]);
            captured.raw.extend_from_slice(&buffer[..read]);
            if query.windows(4).any(|s| s == b"\x1b[6n") {
                let (row, col) = captured.parser.screen().cursor_position();
                let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                let mut writer = writer.lock().expect("writer lock");
                let _ = writer.write_all(reply.as_bytes());
                let _ = writer.flush();
            }
            query.drain(..query.len().saturating_sub(3));
        }
        captured.lock().expect("output lock").ended = true;
    });
    output
}

struct TerminalTest {
    child: TestChild,
    master: Box<dyn MasterPty + Send>,
    output: Arc<Mutex<CapturedTerminal>>,
    journal: PathBuf,
    release_file: PathBuf,
    temp: tempfile::TempDir,
    /// The screen when the test last started acting. A needle found here was
    /// not produced by that action.
    before_action: String,
    settled: bool,
}

#[allow(clippy::expect_used, clippy::panic)]
impl TerminalTest {
    fn start(scenario: &str, lines: u16) -> Self {
        Self::start_with_auth(scenario, lines, None)
    }

    fn start_with_auth(scenario: &str, lines: u16, auth_mode: Option<&str>) -> Self {
        Self::start_with_options(scenario, lines, auth_mode, &[], false)
    }

    fn start_with_options(
        scenario: &str,
        lines: u16,
        auth_mode: Option<&str>,
        args: &[&str],
        has_initial_prompt: bool,
    ) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let profile = temp.path().join("profile");
        let project = temp.path().join("project");
        let journal = temp.path().join("commands.jsonl");
        let release_file = temp.path().join("release");
        std::fs::create_dir_all(&profile).expect("profile");
        std::fs::create_dir_all(&project).expect("project");
        if has_initial_prompt {
            std::fs::write(profile.join("settings.json"), r#"{"model":"haiku","effortLevel":"medium","permissions":{"defaultMode":"default"}}"#).expect("saved settings");
        }
        let bridge = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-bridge.js");
        let runtime = which::which("bun")
            .or_else(|_| which::which("node"))
            .expect("terminal tests need bun or node on PATH");

        let pair = native_pty_system().openpty(pty_size(SHORT_ROWS, COLS)).expect("open pty");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_claude-rs"));
        command.arg("--no-update-check");
        command.arg("--log-file");
        command.arg(temp.path().join("runtime.log"));
        command.env("RUST_LOG", "warn,app.lifecycle=debug,app.render=debug,bridge.protocol=debug");
        command.arg("--dir");
        command.arg(&project);
        for arg in args {
            command.arg(arg);
        }
        command.cwd(&project);
        command.env("CLAUDE_CONFIG_DIR", &profile);
        command.env("CLAUDE_RS_AGENT_BRIDGE", &bridge);
        command.env("CLAUDE_RS_AGENT_BRIDGE_RUNTIME", runtime);
        command.env("FAKE_BRIDGE_LINES", lines.to_string());
        command.env("FAKE_BRIDGE_INTERVAL_MS", "15");
        command.env("FAKE_BRIDGE_FOLLOW_UP_LINES", "3");
        command.env("FAKE_BRIDGE_SCENARIO", scenario);
        command.env("FAKE_BRIDGE_JOURNAL", &journal);
        command.env("FAKE_BRIDGE_RELEASE_FILE", &release_file);
        if let Some(mode) = auth_mode {
            let cli_dir = temp.path().join("bin");
            std::fs::create_dir(&cli_dir).expect("fake CLI directory");
            let cli = cli_dir.join(if cfg!(windows) { "claude.exe" } else { "claude" });
            if mode == "spawn-error" {
                let contents = if cfg!(unix) {
                    // macOS can run executable text without a shebang through
                    // a shell. A missing interpreter forces a spawn error.
                    format!("#!{}\nexit 99\n", cli_dir.join("missing-interpreter").display())
                } else {
                    "invalid executable fixture".to_owned()
                };
                std::fs::write(&cli, contents).expect("invalid executable");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
                        .expect("executable permission");
                }
            } else {
                let source =
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-claude.rs");
                let output = std::process::Command::new("rustc")
                    .arg("--edition=2024")
                    .arg(source)
                    .arg("-o")
                    .arg(&cli)
                    .output()
                    .expect("compile native fake CLI");
                assert!(
                    output.status.success(),
                    "fake CLI compile failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let mut paths = vec![cli_dir];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
            command.env("PATH", std::env::join_paths(paths).expect("fixture PATH"));
            command.env("FAKE_AUTH_MODE", mode);
        }
        let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("pty writer")));
        let reader = pair.master.try_clone_reader().expect("pty reader");
        let process = pair.slave.spawn_command(command).expect("spawn claude-rs");
        let child = TestChild { process, writer: Arc::clone(&writer) };
        drop(pair.slave);
        let output = capture_output(reader, writer);
        let mut test = Self {
            child,
            master: pair.master,
            output,
            journal,
            release_file,
            temp,
            before_action: String::new(),
            settled: true,
        };
        test.wait_screen("Trust this project");
        test.send(b"y");
        if has_initial_prompt {
            test.wait_until("initial prompt", |test| !test.prompts().is_empty());
        } else {
            test.wait_screen("Type a message");
            // The composer is editable during Connecting, but Enter cannot submit
            // until the connected event has been applied and painted.
            test.wait_screen("[READY]");
        }
        test
    }

    fn mark_action(&mut self) {
        if self.settled {
            self.before_action = self.screen();
            self.settled = false;
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.mark_action();
        let mut writer = self.child.writer.lock().expect("writer lock");
        writer.write_all(bytes).expect("write to pty");
        writer.flush().expect("flush pty");
    }

    fn paste(&mut self, text: &str) {
        self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
    }

    fn screen(&self) -> String {
        self.output.lock().expect("output lock").parser.screen().contents()
    }

    fn transcript_rows(&self) -> Vec<String> {
        let mut screen = self.output.lock().expect("output lock").parser.screen().clone();
        screen.set_scrollback(usize::MAX);
        let mut rows = Vec::new();
        for offset in (1..=screen.scrollback()).rev() {
            screen.set_scrollback(offset);
            rows.push(screen.contents().lines().next().unwrap_or_default().to_owned());
        }
        screen.set_scrollback(0);
        rows.extend(screen.contents().lines().map(str::to_owned));
        rows
    }

    fn missing_reply_row(&self, lines: u16) -> Option<u16> {
        let rows = self.transcript_rows();
        (1..=lines).find(|line| {
            let suffix = format!("streamed line {line}");
            !rows.iter().any(|row| row.trim_end().ends_with(&suffix))
        })
    }

    /// History reaches the terminal in batches, so the whole reply is awaited
    /// rather than asserted against whichever batch has arrived.
    fn wait_completed_reply_in_scrollback(&mut self, lines: u16) {
        let started = Instant::now();
        while let Some(line) = self.missing_reply_row(lines) {
            assert!(
                started.elapsed() < WAIT_LIMIT,
                "completed reply row {line} is missing from scrollback:\n{}\nTranscript:\n{}",
                self.diagnostics(),
                self.transcript_rows().join("\n"),
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        self.settled = true;
    }

    /// Returns once the app has drawn at the new size. The dimensions must
    /// differ from the current ones, or an earlier draw would satisfy the wait.
    fn resize_and_wait_for_draw(&mut self, rows: u16, cols: u16) {
        let current = self.output.lock().expect("output lock").parser.screen().size();
        assert_ne!(current, (rows, cols), "a draw at the current size proves no resize");
        let log_path = self.temp.path().join("runtime.log");
        let previous_length = std::fs::read(&log_path).expect("runtime log").len();
        self.resize(rows, cols);
        self.wait_until("draw at the new terminal dimensions", |test| {
            test.runtime_records_since(previous_length).iter().any(|record| {
                (record["event_name"] == "inline_chat_viewport_draw"
                    || record["event_name"] == "fullscreen_surface_draw")
                    && record["terminal_height"] == rows
                    && record["terminal_width"] == cols
            })
        });
    }

    /// Resizes, waits for the app to draw at the new size, and then requires
    /// everything in `kept` to be on that screen.
    fn resize_keeping(&mut self, rows: u16, cols: u16, kept: &[&str]) -> String {
        self.resize_and_wait_for_draw(rows, cols);
        self.wait_for(&format!("{kept:?} after resizing to {rows}x{cols}"), |test| {
            let screen = test.screen();
            kept.iter().all(|needle| screen.contains(needle)).then_some(screen)
        })
    }

    fn runtime_records_since(&self, offset: usize) -> Vec<Value> {
        let log = std::fs::read_to_string(self.temp.path().join("runtime.log")).unwrap_or_default();
        log.get(offset..)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn diagnostics(&self) -> String {
        let output = self.output.lock().expect("output lock");
        format!(
            "{}\nRaw output tail:\n{}\nRuntime log tail:\n{}",
            output.parser.screen().contents(),
            tail(&String::from_utf8_lossy(&output.raw), 2000),
            tail(
                &std::fs::read_to_string(self.temp.path().join("runtime.log")).unwrap_or_default(),
                6000
            )
        )
    }

    /// Polls `probe` until it yields a value twice in a row, so a transient
    /// partial paint is not sufficient to advance the test.
    fn wait_for<T>(&mut self, description: &str, probe: impl Fn(&Self) -> Option<T>) -> T {
        let started = Instant::now();
        let mut observed = false;
        while started.elapsed() < WAIT_LIMIT {
            match probe(self) {
                Some(value) if observed => {
                    self.settled = true;
                    return value;
                }
                value => observed = value.is_some(),
            }
            assert!(
                self.child.process.try_wait().expect("poll child").is_none(),
                "child exited waiting for {description}:\n{}",
                self.diagnostics()
            );
            let ended = self.output.lock().expect("output lock").ended;
            assert!(!ended, "PTY closed waiting for {description}:\n{}", self.diagnostics());
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {description}:\n{}", self.diagnostics());
    }

    fn wait_until(&mut self, description: &str, predicate: impl Fn(&Self) -> bool) {
        self.wait_for(description, |test| predicate(test).then_some(()));
    }

    /// Waits for `needle` to appear as a result of the last action.
    fn wait_screen(&mut self, needle: &str) {
        self.wait_screen_with(needle, &[]);
    }

    /// Waits for one screen that shows the new `needle` together with
    /// everything in `kept`, and returns that screen.
    fn wait_screen_with(&mut self, needle: &str, kept: &[&str]) -> String {
        assert!(
            !self.before_action.contains(needle),
            "{needle:?} was on screen before the test acted, so it cannot show the outcome:\n{}",
            self.before_action
        );
        self.wait_for(&format!("{needle:?} with {kept:?}"), |test| {
            let screen = test.screen();
            (screen.contains(needle) && kept.iter().all(|kept| screen.contains(kept)))
                .then_some(screen)
        })
    }

    /// The status leaves `[READY]` when a prompt is submitted and returns only
    /// when its turn completes, so `[READY]` beside output from that turn shows
    /// the app applied the completion.
    fn wait_turn_finished(&mut self, turn_output: &str) -> String {
        self.wait_screen_with(turn_output, &["[READY]"])
    }

    /// Returns the first streamed row shown while the viewport is pinned.
    fn wait_pinned_output(&mut self) -> String {
        self.wait_screen_with("Reading output", &["streamed line"])
            .lines()
            .find(|line| line.contains("streamed line"))
            .expect("visible output anchor")
            .trim()
            .to_owned()
    }

    fn release_auth_child(&mut self) {
        self.mark_action();
        std::fs::write(self.temp.path().join("profile/auth-release"), b"continue")
            .expect("release auth child");
    }

    /// Output written while the app exits is read after the exit is observed.
    fn wait_final_output(&self, needle: &str) -> String {
        let started = Instant::now();
        loop {
            let raw =
                String::from_utf8_lossy(&self.output.lock().expect("output lock").raw).into_owned();
            if raw.contains(needle) {
                return raw;
            }
            assert!(
                started.elapsed() < WAIT_LIMIT,
                "missing {needle:?} in final output: {}",
                tail(&raw, 4000)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Opening settings starts a refresh, and settings ignore edits until it completes.
    fn wait_settings_ready(&mut self, needle: &str) {
        self.wait_until(&format!("refreshed settings showing {needle}"), |test| {
            let screen = test.screen();
            screen.contains(needle) && !screen.contains("Refreshing settings")
        });
    }

    /// An editor ignores keys until its save is acknowledged, which closes it
    /// and takes its `draft` off the screen.
    fn wait_settings_saved(&mut self, saved: &str, draft: &str) {
        self.wait_until(&format!("saved settings showing {saved}"), |test| {
            let screen = test.screen();
            screen.contains(saved) && !screen.contains(draft)
        });
    }

    /// Opening settings starts a refresh, and settings ignore edits until it completes.
    fn wait_settings_ready(&mut self, needle: &str) {
        self.wait_until(&format!("refreshed settings showing {needle}"), |test| {
            let screen = test.screen();
            screen.contains(needle) && !screen.contains("Refreshing settings")
        });
    }

    /// An editor ignores keys until its save is acknowledged, which closes it
    /// and takes its `draft` off the screen.
    fn wait_settings_saved(&mut self, saved: &str, draft: &str) {
        self.wait_until(&format!("saved settings showing {saved}"), |test| {
            let screen = test.screen();
            screen.contains(saved) && !screen.contains(draft)
        });
    }

    fn wait_setting(&mut self, file: &str, pointer: &str, expected: Option<&Value>) {
        self.wait_until(&format!("persisted {pointer}"), |test| {
            std::fs::read(test.temp.path().join(file))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|document| document.pointer(pointer) == expected)
        });
    }

    fn journal(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.journal)
            .unwrap_or_default()
            .lines()
            // A reader can briefly observe the final append in progress.
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn wait_journal(&mut self, name: &str) {
        self.wait_until(name, |test| {
            test.journal().iter().any(|entry| entry["event"] == name || entry["name"] == name)
        });
    }

    fn prompts(&self) -> Vec<Value> {
        self.commands("prompt")
    }

    fn commands(&self, name: &str) -> Vec<Value> {
        self.journal().into_iter().filter(|entry| entry["command"] == name).collect()
    }

    fn assert_prompts(&self, expected: &[&str]) {
        let prompts = self.prompts();
        let actual: Vec<_> = prompts
            .iter()
            .map(|prompt| {
                let texts: Vec<_> = prompt["chunks"]
                    .as_array()
                    .expect("prompt chunks")
                    .iter()
                    .filter(|chunk| chunk["kind"] == "text")
                    .map(|chunk| chunk["value"].as_str().expect("text chunk"))
                    .collect();
                texts.concat()
            })
            .collect();
        assert_eq!(actual, expected, "incorrect or unintended submissions");
    }

    fn submit(&mut self, text: &str, visible: &str) {
        let log_offset =
            std::fs::read(self.temp.path().join("runtime.log")).expect("runtime log").len();
        self.paste(text);
        // The overview, tips, and transcript can already mention the command.
        // Check the editor region from a subsequent draw. ConPTY's native cursor
        // can remain on the spinner instead of the editor.
        self.wait_until(&format!("composer contains {visible:?}"), |test| {
            let Some(draw) = test
                .runtime_records_since(log_offset)
                .into_iter()
                .rev()
                .find(|record| record["event_name"] == "inline_chat_viewport_draw")
            else {
                return false;
            };
            let Some(top) = draw["editor_top"].as_u64().and_then(|top| usize::try_from(top).ok())
            else {
                return false;
            };
            let Some(height) =
                draw["editor_height"].as_u64().and_then(|height| usize::try_from(height).ok())
            else {
                return false;
            };
            test.screen().lines().skip(top).take(height).any(|line| line.contains(visible))
        });
        self.submit_draft();
    }

    fn submit_draft(&mut self) {
        // Windows falls back to key-burst paste detection. Its 250 ms trailing
        // Enter suppression deliberately treats an immediate CR as paste text.
        std::thread::sleep(Duration::from_millis(500));
        self.send(b"\r");
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.mark_action();
        self.output.lock().expect("output lock").parser.screen_mut().set_size(rows, cols);
        self.master.resize(pty_size(rows, cols)).expect("resize pty");
    }

    fn release(&mut self) {
        self.signal("continue");
    }

    /// Moves the scripted bridge past its current barrier.
    fn signal(&mut self, step: &str) {
        self.mark_action();
        std::fs::write(&self.release_file, step).expect("signal fixture barrier");
    }

    fn shutdown(&mut self) {
        self.send(b"\x11"); // Normal Ctrl+Q shutdown, not a forced kill.
        self.wait_shutdown();
    }

    fn wait_shutdown(&mut self) {
        let status = self.child.wait_for_exit(Duration::from_secs(15));
        assert!(
            status.as_ref().is_some_and(ExitStatus::success),
            "unclean shutdown ({status:?}):\n{}",
            self.diagnostics()
        );
        assert!(
            self.journal().iter().any(|entry| entry["command"] == "shutdown"),
            "the bridge did not receive graceful shutdown"
        );
        let output = self.output.lock().expect("output lock");
        let text = String::from_utf8_lossy(&output.raw);
        assert!(!text.contains(OWNED_REGION_ERROR), "{}", tail(&text, 2000));
    }

    fn wait_shutdown_screen(&self) {
        // ConPTY consumes focus-reporting and line-wrap mode changes. The
        // shutdown composer hides its cursor; terminal restoration shows it.
        let started = Instant::now();
        loop {
            let screen = self.output.lock().expect("output lock").parser.screen().clone();
            if screen.contents().contains("Shutting down") && !screen.hide_cursor() {
                return;
            }
            assert!(
                started.elapsed() < WAIT_LIMIT,
                "final shutdown screen: {}",
                self.diagnostics()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn shutdown_preserves_completed_history_without_reinserting_it() {
    for mode in ["live", "reading", "fullscreen"] {
        let mut test = TerminalTest::start("resize-replay", 240);
        test.submit("SHUTDOWN_HISTORY", "SHUTDOWN_HISTORY");
        test.wait_turn_finished("streamed line 240");
        test.wait_completed_reply_in_scrollback(240);
        if mode == "reading" {
            test.send(b"\x1b[5~");
            test.wait_screen("Reading output");
        } else if mode == "fullscreen" {
            test.submit("/config", "/config");
            test.wait_settings_ready("Description:");
            assert!(test.output.lock().expect("output lock").parser.screen().alternate_screen());
        }
        let offset =
            std::fs::read(test.temp.path().join("runtime.log")).expect("runtime log").len();
        test.shutdown();
        test.wait_shutdown_screen();
        let rows = test.transcript_rows();
        for line in 1..=240 {
            let suffix = format!("streamed line {line}");
            assert_eq!(
                rows.iter().filter(|row| row.trim_end().ends_with(&suffix)).count(),
                1,
                "{mode}: completed row {line} must survive exactly once"
            );
        }
        let records = test.runtime_records_since(offset);
        assert!(
            !records
                .iter()
                .any(|record| record["event_name"] == "inline_chat_scrollback_insert_applied"),
            "{mode}: closing must preserve committed history without writing it again"
        );
        let screen = test.output.lock().expect("output lock").parser.screen().clone();
        assert!(!screen.alternate_screen(), "shutdown must return to the main screen");
        assert!(!screen.hide_cursor(), "shutdown must restore the cursor");
        assert!(screen.contents().contains("Shutting down"));
    }
}

#[test]
fn shutdown_finishes_pending_replay_and_reconciles_a_further_resize() {
    let mut test = TerminalTest::start("resize-replay-shutdown-held", 2_000);
    test.submit("SHUTDOWN_REPLAY", "SHUTDOWN_REPLAY");
    test.wait_turn_finished("streamed line 2000");
    test.wait_completed_reply_in_scrollback(2_000);
    let offset = std::fs::read(test.temp.path().join("runtime.log")).expect("runtime log").len();
    test.resize(12, 64);
    test.wait_until("incomplete transcript recovery", |test| {
        test.runtime_records_since(offset).iter().any(|record| {
            record["event_name"] == "inline_chat_terminal_draw_transaction"
                && record["replay_incomplete"] == true
        })
    });
    test.send(b"\x11");
    test.wait_until("shutdown while recovery remains pending", |test| {
        test.runtime_records_since(offset).iter().any(|record| {
            record["event_name"] == "inline_chat_draw_summary"
                && record["composer_preview"]
                    .as_str()
                    .is_some_and(|text| text.contains("Shutting down"))
        })
    });
    test.resize(18, 72);
    test.wait_journal("shutdown-held");
    // A second resize happens after the first rendering drain, while the bridge
    // is still cleaning up. It must finish replay before terminal restoration.
    test.resize(25, 81);
    test.signal("close");
    test.wait_shutdown();
    test.wait_shutdown_screen();
    let records = test.runtime_records_since(offset);
    assert!(
        records.iter().any(|record| {
            record["event_name"] == "inline_chat_viewport_draw"
                && record["terminal_height"] == 18
                && record["terminal_width"] == 72
        }),
        "shutdown must reconcile the terminal dimensions during recovery"
    );
    assert!(
        records.iter().any(|record| {
            record["event_name"] == "inline_chat_viewport_draw"
                && record["terminal_height"] == 25
                && record["terminal_width"] == 81
        }),
        "shutdown must recover a resize during bridge cleanup"
    );
    assert!(
        records.iter().any(|record| {
            record["event_name"] == "inline_chat_terminal_draw_transaction"
                && record["replay_complete"] == true
        }),
        "shutdown must finish the pending transcript replay"
    );
    let rows = test.transcript_rows();
    for line in 1..=2_000 {
        let suffix = format!("streamed line {line}");
        assert_eq!(
            rows.iter().filter(|row| row.trim_end().ends_with(&suffix)).count(),
            1,
            "recovered row {line} must survive exactly once"
        );
    }
}

#[test]
fn shutdown_keeps_received_output_and_dismisses_pending_interactions() {
    for scenario in ["hold-success", "permission", "question"] {
        let mut test = TerminalTest::start(scenario, 8);
        test.submit("SHUTDOWN_ACTIVE", "SHUTDOWN_ACTIVE");
        let marker = match scenario {
            "permission" => "Allow once",
            "question" => "Choose fixture destination",
            _ => "streamed line 8",
        };
        test.wait_screen(marker);
        if scenario == "hold-success" {
            test.wait_journal("reply-held");
        }
        test.shutdown();
        test.wait_shutdown_screen();
        let screen = test.screen();
        assert!(screen.contains("Shutting down"));
        assert!(!screen.contains("Allow once") && !screen.contains("Choose fixture destination"));
        assert_eq!(test.commands("cancel_turn").len(), 1);
        if scenario == "hold-success" {
            let rows = test.transcript_rows();
            for line in 1..=8 {
                let suffix = format!("streamed line {line}");
                assert_eq!(rows.iter().filter(|row| row.trim_end().ends_with(&suffix)).count(), 1);
            }
        } else {
            let command =
                if scenario == "permission" { "permission_response" } else { "question_response" };
            let responses = test.commands(command);
            assert_eq!(responses.len(), 1);
            assert_eq!(responses[0]["tool_call_id"], "fixture-tool");
            assert_eq!(responses[0]["session_id"], "fake-session");
            let outcome = if scenario == "permission" {
                serde_json::json!({"outcome": "selected", "option_id": "deny-once"})
            } else {
                serde_json::json!({"outcome": "cancelled"})
            };
            assert_eq!(responses[0]["outcome"], outcome);
        }
    }
}

#[test]
fn shutdown_force_interrupt_can_stop_pending_transcript_recovery() {
    let mut test = TerminalTest::start("resize-replay", 2_000);
    test.submit("FORCE_SHUTDOWN_REPLAY", "FORCE_SHUTDOWN_REPLAY");
    test.wait_turn_finished("streamed line 2000");
    test.wait_completed_reply_in_scrollback(2_000);
    let offset = std::fs::read(test.temp.path().join("runtime.log")).expect("runtime log").len();
    test.resize(12, 64);
    test.wait_until("incomplete transcript recovery", |test| {
        test.runtime_records_since(offset).iter().any(|record| {
            record["event_name"] == "inline_chat_terminal_draw_transaction"
                && record["replay_incomplete"] == true
        })
    });
    test.send(b"\x11\x03"); // Request shutdown, then explicitly force it.
    let status = test.child.wait_for_exit(Duration::from_secs(15));
    assert!(status.as_ref().is_some_and(ExitStatus::success), "force exit: {}", test.diagnostics());
    test.wait_shutdown_screen();
    assert!(
        !test.runtime_records_since(offset).iter().any(|record| {
            record["event_name"] == "inline_chat_terminal_draw_transaction"
                && record["replay_complete"] == true
        }),
        "force exit must not wait for the remaining transcript replay"
    );
    assert!(
        test.missing_reply_row(2_000).is_some(),
        "the fixture must still have pending recovery"
    );
}

#[test]
fn interactive_cli_startup_sends_overrides_and_initial_prompt_without_saving_them() {
    let common =
        ["--model", "opus", "--effort", "max", "--permission-mode", "plan", "--agent", "reviewer"];
    for (selection, expected_session) in [
        (vec![], "fake-session"),
        (vec!["-r", "chosen-session"], "chosen-session"),
        (vec!["-c"], "fake-recent-session"),
        (vec!["resume", "chosen-session"], "chosen-session"),
    ] {
        let mut args = common.to_vec();
        args.extend(selection);
        args.push("Review this project");
        let mut test = TerminalTest::start_with_options("stream", 3, None, &args, true);
        test.wait_journal("turn_complete");
        test.assert_prompts(&["Review this project"]);
        assert_eq!(test.prompts()[0]["session_id"], expected_session);
        let launch = test
            .journal()
            .into_iter()
            .find(|entry| {
                entry["command"] == "create_session" || entry["command"] == "resume_session"
            })
            .expect("session launch command");
        assert_eq!(launch["launch_settings"]["model"], "opus");
        assert_eq!(launch["launch_settings"]["permission_mode"], "plan");
        assert_eq!(launch["launch_settings"]["effort"], "max");
        assert_eq!(launch["launch_settings"]["agent"], "reviewer");
        let saved: Value = serde_json::from_str(
            &std::fs::read_to_string(test.temp.path().join("profile/settings.json"))
                .expect("saved settings"),
        )
        .expect("settings JSON");
        assert_eq!(
            saved,
            serde_json::json!({"model":"haiku","effortLevel":"medium","permissions":{"defaultMode":"default"}})
        );
        test.shutdown();
    }
}

#[test]
fn auth_child_owns_stdin_and_output_and_returns_a_resized_usable_terminal() {
    for mode in ["success", "failure"] {
        let mut test = TerminalTest::start_with_auth("stream", 3, Some(mode));
        test.submit("/login", "/login");
        test.wait_screen("AUTH_CHILD_READY");
        // Once the child is waiting on stdin, the TUI must stop every output
        // source, including animated OSC window-title updates.
        let before = test.output.lock().expect("output lock").raw.len();
        std::thread::sleep(Duration::from_millis(250));
        let after = test.output.lock().expect("output lock").raw.len();
        assert_eq!(after, before, "TUI wrote while auth child owned the terminal");
        test.resize(25, 61);
        test.send(b"child_only_407\r");
        // The child records its input before it prints this marker.
        test.wait_screen("AUTH_INPUT_RECORDED");
        let profile = test.temp.path().join("profile");
        let received =
            std::fs::read_to_string(profile.join("auth-input")).expect("child stdin record");
        assert_eq!(received.trim_end_matches(['\r', '\n']), "child_only_407");
        test.release_auth_child();
        if mode == "failure" {
            test.wait_screen("/login failed (exit code: 7)");
        }
        test.wait_screen("[READY]");
        test.resize(38, 87);
        test.send("AFTER_AUTH 界".as_bytes());
        test.wait_screen("AFTER_AUTH 界");
        test.paste(" + PASTE 🦀");
        test.wait_screen("PASTE 🦀");
        test.submit_draft();
        test.wait_screen("reply 1 started");
        test.assert_prompts(&["AFTER_AUTH 界 + PASTE 🦀"]);
        test.shutdown();
    }
}

#[test]
fn background_permission_remains_interactive_after_launch_enters_history() {
    for (keys, option, outcome) in
        [(b"\r".as_slice(), "allow-once", "completed"), (b"\x1b".as_slice(), "deny-once", "killed")]
    {
        let mut test = TerminalTest::start("background-initial", 45);
        test.submit("BACKGROUND_PERMISSION", "BACKGROUND_PERMISSION");
        test.wait_journal("background-turn-1-held");
        test.wait_screen("PRIMARY_TASK");
        test.signal("followers");
        test.wait_until("background launch and followers enter history", |test| {
            let rows = test.transcript_rows();
            rows.iter().any(|row| row.contains("FOLLOWUP_045"))
                && rows.iter().filter(|row| row.contains("FOLLOWUP_")).count() == 45
        });
        test.signal("permission");
        test.wait_screen_with("Allow once", &["Deny once"]);
        test.resize_keeping(25, 61, &["Allow once", "Deny once"]);
        let screen = test.screen();
        let rows: Vec<_> = screen.lines().collect();
        let tool =
            rows.iter().rposition(|row| row.contains("PRIMARY_TASK")).expect("permission tool row");
        assert!(
            rows.iter()
                .skip(tool)
                .take(9)
                .any(|row| row.contains("Allow once") && row.contains("Deny once"))
        );
        assert!(test.commands("permission_response").is_empty());
        test.signal("turn-end");
        test.wait_screen_with("[READY]", &["Allow once", "Deny once"]);
        test.resize_keeping(TALL_ROWS, 120, &["Allow once", "Deny once"]);
        test.send(keys);
        test.wait_journal("background-permission-answered");
        test.wait_until(
            "permission controls disappear without completing the background task",
            |test| {
                !test.screen().contains("Allow once")
                    && !test.transcript_rows().iter().any(|row| row.contains("PRIMARY_RESULT_TASK"))
            },
        );
        let responses = test.commands("permission_response");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["tool_call_id"], "background-primary");
        assert_eq!(responses[0]["outcome"]["option_id"], option);
        test.submit("continue after background permission", "continue after background permission");
        test.wait_journal("background-turn-2-held");
        test.signal(outcome);
        test.wait_screen("RESULT_FENCE");
        test.resize_and_wait_for_draw(TALL_ROWS, COLS);
        let rows = test.transcript_rows();
        assert_eq!(rows.iter().filter(|row| row.contains("PRIMARY_TASK")).count(), 1);
        assert_eq!(rows.iter().filter(|row| row.contains("PRIMARY_RESULT_TASK")).count(), 1);
        test.signal("turn-end");
        test.wait_screen("[READY]");
        test.shutdown();
    }
}

#[test]
fn background_result_labels_follow_the_visible_speaker_through_replay_and_cancellation() {
    let mut test = TerminalTest::start("background-replies", 3);
    test.submit("BACKGROUND_LABELS", "BACKGROUND_LABELS");
    test.wait_turn_finished("Foreground reply is complete.");
    for number in 1..=2 {
        test.signal(&format!("result-{number}"));
        test.wait_journal(&format!("result-{number}"));
        test.wait_screen(&format!("OUTPUT_{number}"));
    }
    let assert_group = |test: &TerminalTest| {
        let rows = test.transcript_rows();
        let first = rows.iter().position(|row| row.trim() == "Claude").expect("speaker label");
        let last = rows.iter().position(|row| row.contains("OUTPUT_2")).expect("second result");
        assert_eq!(rows[first..=last].iter().filter(|row| row.trim() == "Claude").count(), 1);
        let positions: Vec<_> =
            ["LAUNCH_1", "LAUNCH_2", "LAUNCH_3", "Foreground reply", "RESULT_1", "RESULT_2"]
                .into_iter()
                .map(|needle| {
                    rows.iter().position(|row| row.contains(needle)).expect("ordered content")
                })
                .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        for number in 1..=2 {
            assert_eq!(
                rows.iter().filter(|row| row.contains(&format!("RESULT_{number}"))).count(),
                1
            );
        }
    };
    assert_group(&test);
    test.resize_and_wait_for_draw(TALL_ROWS, COLS);
    assert_group(&test);
    test.submit("NORMAL_FOLLOWUP", "NORMAL_FOLLOWUP");
    test.wait_turn_finished("Normal conversation continues completely.");
    test.submit("CANCEL_EMPTY_REPLY", "CANCEL_EMPTY_REPLY");
    test.wait_journal("cancellable-turn-started");
    test.wait_screen("Thinking…");
    test.send(b"\x1b");
    test.wait_until("cancelled turn settles", |test| {
        test.commands("cancel_turn").len() == 1 && test.screen().contains("[READY]")
    });
    test.signal("result-3");
    test.wait_screen("OUTPUT_3");
    test.resize_and_wait_for_draw(SHORT_ROWS, COLS);
    let rows = test.transcript_rows();
    let normal =
        rows.iter().position(|row| row.contains("NORMAL_FOLLOWUP")).expect("normal user message");
    let cancelled = rows
        .iter()
        .position(|row| row.contains("CANCEL_EMPTY_REPLY"))
        .expect("cancelled user message");
    let result = rows.iter().position(|row| row.contains("RESULT_3")).expect("later result");
    assert_eq!(rows[normal..cancelled].iter().filter(|row| row.trim() == "Claude").count(), 1);
    assert_eq!(rows[cancelled..=result].iter().filter(|row| row.trim() == "Claude").count(), 1);
    test.assert_prompts(&["BACKGROUND_LABELS", "NORMAL_FOLLOWUP", "CANCEL_EMPTY_REPLY"]);
    test.shutdown();
}

#[test]
fn background_completion_replies_keep_streamed_tails_mutable_and_preserve_normal_turns() {
    let mut test = TerminalTest::start("background-replies", 3);
    test.submit("BACKGROUND_STREAM", "BACKGROUND_STREAM");
    test.wait_turn_finished("Foreground reply is complete.");
    test.paste("NORMAL_AFTER_COMPLETIONS");
    test.wait_screen("NORMAL_AFTER_COMPLETIONS");
    for (number, prefix, full) in [
        (1, "The", "The first background task finished completely."),
        (2, "Short task 2 (`", "Short task 2 (task-two) finished with exit code 0."),
    ] {
        test.signal(&format!("result-{number}"));
        test.wait_screen(&format!("OUTPUT_{number}"));
        let log_offset =
            std::fs::read(test.temp.path().join("runtime.log")).expect("runtime log").len();
        test.signal(&format!("prefix-{number}"));
        test.wait_journal(&format!("prefix-{number}"));
        test.wait_screen(prefix);
        test.wait_until("unfinished reply remains in the mutable region", |test| {
            test.runtime_records_since(log_offset).iter().any(|record| {
                record["event_name"] == "inline_chat_draw_summary"
                    && record["status"] == "Running"
                    && record["first_mutable_boundary_kind"] == "Some(AssistantText)"
            })
        });
        test.resize_keeping(
            if number == 1 { TALL_ROWS } else { SHORT_ROWS },
            COLS,
            &[prefix, "NORMAL_AFTER_COMPLETIONS"],
        );
        test.signal(&format!("finish-{number}"));
        test.wait_journal(&format!("finish-{number}"));
        test.wait_turn_finished(full);
        let rows = test.transcript_rows();
        assert_eq!(rows.iter().filter(|row| row.trim() == full).count(), 1);
        assert!(!rows.iter().any(|row| row.trim() == prefix));
    }
    test.submit_draft();
    test.wait_turn_finished("Normal conversation continues completely.");
    test.resize_and_wait_for_draw(TALL_ROWS, COLS);
    let rows = test.transcript_rows();
    for full in [
        "Foreground reply is complete.",
        "The first background task finished completely.",
        "Short task 2 (task-two) finished with exit code 0.",
        "Normal conversation continues completely.",
    ] {
        assert_eq!(rows.iter().filter(|row| row.trim() == full).count(), 1);
    }
    test.assert_prompts(&["BACKGROUND_STREAM", "NORMAL_AFTER_COMPLETIONS"]);
    test.shutdown();
}

#[test]
fn background_tasks_flush_in_creation_order_and_append_results_once() {
    for (scenario, outcome) in [
        ("background-initial", "completed"),
        ("background-later", "failed"),
        ("background-later", "killed"),
        ("background-agent-initial", "completed"),
        ("background-agent-later", "failed"),
        ("background-resume", "completed"),
    ] {
        let mut test = if scenario == "background-resume" {
            let mut test = TerminalTest::start_with_options(
                scenario,
                45,
                None,
                &["--resume", "f96f66ae-b5d4-4bc2-b5a3-08336cf4e850"],
                false,
            );
            test.wait_screen("PRIMARY_TASK");
            assert_eq!(test.commands("resume_session").len(), 1);
            test
        } else {
            TerminalTest::start(scenario, 45)
        };
        test.submit("start background workflow", "start background workflow");
        test.wait_journal("background-turn-1-held");
        if scenario != "background-resume" {
            test.wait_screen("PRIMARY_TASK");
        }
        if scenario.ends_with("-later") {
            test.signal("detach");
            test.wait_until("primary tool becomes detached", |test| {
                test.transcript_rows()
                    .iter()
                    .any(|row| row.contains("PRIMARY_TASK") && row.contains('↗'))
            });
        }
        test.signal("followers");
        test.wait_journal("followers-created");
        test.wait_until("completed followers reach history while primary is running", |test| {
            let rows = test.transcript_rows();
            rows.iter().any(|row| row.contains("FOLLOWUP_045"))
                && rows.iter().any(|row| row.contains("PRIMARY_TASK") && row.contains('↗'))
                && (1..=45)
                    .all(|i| rows.iter().any(|row| row.contains(&format!("FOLLOWUP_{i:03}"))))
        });
        let rows = test.transcript_rows();
        let position = |needle: &str| {
            rows.iter().position(|row| row.contains(needle)).expect("ordered history entry")
        };
        assert!(position("PRIMARY_TASK") < position("FOLLOWUP_001"));
        let follower_positions: Vec<_> =
            (1..=45).map(|i| position(&format!("FOLLOWUP_{i:03}"))).collect();
        assert!(follower_positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!rows.iter().any(|row| row.contains("PRIMARY_FINAL_OUTPUT")));

        test.signal("progress");
        test.wait_until("background progress applied without rewriting launch", |test| {
            test.transcript_rows().iter().any(|row| row.contains("PROGRESS_FENCE"))
        });
        assert!(
            !test
                .transcript_rows()
                .iter()
                .any(|row| row.contains("MUTATED_LAUNCH")
                    || row.contains("BACKGROUND_PROGRESS_ONLY"))
        );
        test.signal("turn-end");
        test.wait_until("foreground turn completes independently", |test| {
            test.screen().contains("[READY]")
        });
        test.submit("another foreground turn", "another foreground turn");
        test.wait_journal("background-turn-2-held");
        test.signal(outcome);
        test.wait_until("linked background result is rendered", |test| {
            test.transcript_rows().iter().any(|row| row.contains("RESULT_FENCE"))
        });
        let rows = test.transcript_rows();
        let result =
            rows.iter().position(|row| row.contains("PRIMARY_RESULT_TASK")).expect("result card");
        assert!(
            result
                > rows.iter().position(|row| row.contains("FOLLOWUP_045")).expect("last follower")
        );
        assert_eq!(rows.iter().filter(|row| row.contains("PRIMARY_TASK")).count(), 1);
        assert_eq!(rows.iter().filter(|row| row.contains("PRIMARY_RESULT_TASK")).count(), 1);
        assert!(rows[result + 1..].iter().take(4).any(|row| row.contains("PRIMARY_FINAL_OUTPUT")));
        assert!(rows[result].contains(if outcome == "completed" { '✓' } else { '✗' }));
        test.resize_and_wait_for_draw(TALL_ROWS, COLS);
        let replayed = test.transcript_rows();
        assert_eq!(replayed.iter().filter(|row| row.contains("PRIMARY_TASK")).count(), 1);
        assert_eq!(replayed.iter().filter(|row| row.contains("PRIMARY_RESULT_TASK")).count(), 1);
        test.signal("turn-end");
        test.wait_until("second turn finishes", |test| test.screen().contains("[READY]"));
        test.shutdown();
    }
}

#[test]
fn auth_spawn_failure_restores_terminal_ownership_and_input() {
    let mut test = TerminalTest::start_with_auth("stream", 3, Some("spawn-error"));
    test.submit("/login", "/login");
    test.wait_screen_with("Failed to run claude auth login", &["[READY]"]);
    test.resize(25, 61);
    test.resize(38, 87);
    test.send(b"AFTER_SPAWN_ERROR");
    test.wait_screen("AFTER_SPAWN_ERROR");
    test.paste(" + PASTE");
    test.wait_screen("+ PASTE");
    test.submit_draft();
    test.wait_screen("reply 1 started");
    test.assert_prompts(&["AFTER_SPAWN_ERROR + PASTE"]);
    test.shutdown();
}

#[test]
fn fatal_bridge_exit_during_auth_stops_the_owned_child_and_finishes_shutdown() {
    let mut test = TerminalTest::start_with_auth("disconnect-during-auth", 3, Some("success"));
    test.submit("/login", "/login");
    test.wait_screen("AUTH_CHILD_READY");
    // The fake CLI is blocked in read_line. Shutdown must stop and reap the
    // owned child before returning the terminal and finishing the app exit.
    test.release();
    let status = test
        .child
        .wait_for_exit(Duration::from_secs(15))
        .expect("bounded shutdown while auth owns stdin");
    assert!(!status.success());
    assert!(!test.temp.path().join("profile/auth-input").exists());
    test.assert_prompts(&[]);
    test.wait_final_output("exited before completing the protocol");
}

#[test]
fn concurrent_resize_typing_and_paste_deliver_every_prompt_exactly_once() {
    let mut test = TerminalTest::start("stream", 3);
    let mut expected = Vec::new();
    for index in 0..15 {
        test.resize(25, 61);
        let typed = format!("RESIZE_INPUT_{index}");
        test.send(typed.as_bytes());
        test.resize(38, 87);
        test.paste(" + PASTE 界 🦀");
        // Earlier prompts stay in the transcript, so only this prompt's own
        // text shows that the composer holds the whole draft.
        let draft = format!("{typed} + PASTE 界 🦀");
        test.wait_screen(&draft);
        test.submit_draft();
        test.wait_turn_finished(&format!("reply {} started", index + 1));
        expected.push(draft);
        test.assert_prompts(&expected.iter().map(String::as_str).collect::<Vec<_>>());
    }
    test.shutdown();
}

#[test]
fn tips_setting_preserves_immediate_activity_and_heading_through_real_config_saves() {
    let mut test = TerminalTest::start("hold-activity-off", 3);
    test.submit("NO_OUTPUT_YET", "NO_OUTPUT_YET");
    test.wait_journal("reply-held");
    test.wait_until("disabled tips retain immediate activity and assistant heading", |test| {
        let screen = test.screen();
        screen.contains("NO_OUTPUT_YET")
            && screen.lines().filter(|line| line.trim() == "Claude").count() == 1
            && screen.lines().any(|line| line.starts_with("│ ◆ "))
            && !screen.contains("│ Tip:")
    });
    let activity_header = test
        .screen()
        .lines()
        .find(|line| line.starts_with("│ ◆ "))
        .expect("immediate activity header")
        .trim_end()
        .to_owned();
    for (index, enabled) in [true, false, true].into_iter().enumerate() {
        test.submit("/config", "/config");
        test.wait_settings_ready("Show tips");
        if index == 0 {
            test.send(b"\x1b[B\x1b[B"); // Language -> Reduce motion -> Show tips.
        }
        test.send(b" "); // Reopening settings retains the selected control.
        test.wait_setting(
            "profile/settings.json",
            "/spinnerTipsEnabled",
            Some(&Value::Bool(enabled)),
        );
        test.send(b"\x1b");
        test.wait_until("only tips follow the acknowledged setting", |test| {
            let screen = test.screen();
            screen.lines().any(|line| line.trim() == "NO_OUTPUT_YET")
                && screen.lines().filter(|line| line.trim() == "Claude").count() == 1
                && screen.lines().any(|line| line.trim_end() == activity_header)
                && screen.contains("│ Tip:") == enabled
        });
        test.resize_and_wait_for_draw(25, 61);
        test.resize_and_wait_for_draw(SHORT_ROWS, COLS);
        test.wait_until("resize retains activity and heading independently of tips", |test| {
            let screen = test.screen();
            screen.lines().filter(|line| line.trim() == "Claude").count() == 1
                && screen.lines().any(|line| line.trim_end() == activity_header)
                && screen.contains("│ Tip:") == enabled
        });
    }
    test.signal("requires_action");
    test.wait_until("waiting hides activity and its empty heading", |test| {
        let screen = test.screen();
        !screen.lines().any(|line| line.trim() == "Claude")
            && !screen.contains(&activity_header)
            && !screen.contains("│ Tip:")
    });
    test.signal("running");
    let resumed = test.wait_screen_with("│ Tip:", &[&activity_header]);
    assert_eq!(resumed.lines().filter(|line| line.trim() == "Claude").count(), 1);
    test.signal("done");
    test.wait_until("completion removes the temporary heading", |test| {
        let screen = test.screen();
        screen.contains("[READY]")
            && !screen.lines().any(|line| line.trim() == "Claude")
            && !screen.contains(&activity_header)
            && !screen.contains("│ Tip:")
    });
    assert_eq!(test.commands("mutate_setting").len(), 3);
    assert!(
        test.commands("mutate_setting")
            .iter()
            .all(|command| command["mutation"]["id"] == "spinnerTipsEnabled")
    );
    test.assert_prompts(&["NO_OUTPUT_YET"]);
    test.shutdown();
}

#[test]
fn activity_stays_above_queue_and_out_of_scrollback_through_wait_resize_and_completion() {
    let mut test = TerminalTest::start_with_options(
        "hold-activity",
        3,
        None,
        &["--log-filter", "warn,app.lifecycle=debug,app.render=debug,bridge.protocol=debug"],
        false,
    );
    test.submit("START_ACTIVITY", "START_ACTIVITY");
    test.wait_journal("reply-held");
    let initial_screen = test.wait_screen_with("│ ◆ ", &["│ Tip:", "START_ACTIVITY"]);
    let initial_rows: Vec<_> = initial_screen.lines().map(str::trim_end).collect();
    let heading =
        initial_rows.iter().position(|line| *line == "Claude").expect("assistant heading");
    let activity = initial_rows.iter().position(|line| line.starts_with("│ ◆ ")).expect("activity");
    assert_eq!(initial_rows.iter().filter(|line| **line == "Claude").count(), 1);
    assert!(
        initial_rows.iter().position(|line| line.contains("START_ACTIVITY")).expect("user message")
            < heading
    );
    assert!(heading < activity, "tip text mentioning Claude must not count as the heading");
    assert!(!initial_screen.contains("Thinking…"));
    let activity_header = initial_rows[activity].to_owned();
    test.signal("thinking");
    // Thinking retains the activity verb.
    let thinking = test.wait_screen_with("◆ Thinking…", &[&activity_header]);
    assert_eq!(thinking.matches("Thinking…").count(), 1);
    test.submit("QUEUED_ACTIVITY", "QUEUED_ACTIVITY");
    test.paste("DRAFT_ACTIVITY");
    let screen = test.wait_screen_with(
        "DRAFT_ACTIVITY",
        &["Thinking…", &activity_header, "│ Tip:", "QUEUED_ACTIVITY"],
    );
    assert!(
        screen.find("Thinking…").expect("assistant thinking")
            < screen.find(&activity_header).expect("activity")
    );
    assert!(
        screen.find(&activity_header).expect("activity")
            < screen.find("│ Tip:").expect("grouped tip")
    );
    assert!(
        screen.find("│ Tip:").expect("tip") < screen.find("QUEUED_ACTIVITY").expect("queued field")
    );
    test.resize_and_wait_for_draw(25, 61);
    test.wait_until("cursor remains in editor after resize", |test| {
        let output = test.output.lock().expect("output");
        let screen = output.parser.screen();
        let (row, _) = screen.cursor_position();
        let contents = screen.contents();
        contents.contains("◆ Thinking…")
            && contents
                .lines()
                .nth(usize::from(row))
                .is_some_and(|line| line.contains("DRAFT_ACTIVITY"))
    });
    test.signal("requires_action");
    test.wait_until("hidden activity while input is required", |test| {
        let screen = test.screen();
        !screen.contains("Thinking…") && !screen.contains("Tip:")
    });
    test.signal("running");
    test.wait_screen_with("Thinking…", &["Tip:"]);
    test.signal("working");
    test.wait_until("ordinary activity after thinking retains the activity verb", |test| {
        let screen = test.screen();
        !screen.contains("Thinking…")
            && screen.contains("Tip:")
            && screen.contains(&activity_header)
    });
    test.resize_and_wait_for_draw(SHORT_ROWS, COLS);
    test.signal("done");
    test.wait_until("activity removed and draft kept after completion", |test| {
        let screen = test.screen();
        screen.contains("[READY]")
            && screen.contains("DRAFT_ACTIVITY")
            && !screen.contains("Tip:")
            && !screen.contains("Thinking…")
    });
    let output = test.output.lock().expect("output");
    let mut screen = output.parser.screen().clone();
    for offset in 0..100 {
        screen.set_scrollback(offset);
        assert!(!screen.contents().contains("Thinking…"));
        assert!(!screen.contents().contains("Tip:"));
        assert!(!screen.contents().contains(&activity_header));
    }
}

#[test]
fn queued_prompt_starts_once_after_active_reply_and_preserves_the_next_draft() {
    let mut test = TerminalTest::start("hold-success", 3);
    test.submit("FIRST", "FIRST");
    test.wait_journal("reply-held");
    test.submit("QUEUED 界", "QUEUED 界");
    test.wait_journal("user_message_queued");
    test.paste("UNSENT 🦀");
    test.wait_screen("UNSENT 🦀");
    test.resize(25, 61);
    test.resize(38, 87);
    test.assert_prompts(&["FIRST", "QUEUED 界"]);
    let prompts = test.prompts();
    let queued_id = &prompts[1]["message_uuid"];
    assert!(
        !test
            .journal()
            .iter()
            .any(|e| e["event"] == "user_message_started" && &e["message_uuid"] == queued_id)
    );

    test.release();
    test.wait_screen_with("reply 2 started", &["[READY]", "UNSENT 🦀"]);
    let journal = test.journal();
    let queued = journal
        .iter()
        .position(|e| e["event"] == "user_message_queued" && &e["message_uuid"] == queued_id)
        .expect("queued event");
    let completed =
        journal.iter().position(|e| e["event"] == "turn_complete").expect("first completion");
    assert_eq!(journal[completed]["queued_turn_count"], 1);
    let started: Vec<_> = journal
        .iter()
        .enumerate()
        .filter(|(_, e)| e["event"] == "user_message_started" && &e["message_uuid"] == queued_id)
        .collect();
    assert_eq!(started.len(), 1, "queued message must start exactly once");
    assert!(queued < completed && completed < started[0].0);
    test.submit_draft();
    test.wait_screen("reply 3 started");
    test.assert_prompts(&["FIRST", "QUEUED 界", "UNSENT 🦀"]);
    test.shutdown();
}

#[test]
fn cancelling_a_stream_preserves_the_draft_and_allows_the_next_turn() {
    let mut test = TerminalTest::start("hold-success", 3);
    test.submit("CANCEL_ME", "CANCEL_ME");
    test.wait_journal("reply-held");
    test.paste("AFTER_CANCEL 界");
    test.wait_screen("AFTER_CANCEL 界");
    test.send(b"\x1b"); // Escape cancels a turn; Ctrl+C clears the composer.
    test.wait_journal("turn_interrupt_receipt");
    test.wait_screen_with("[READY]", &["AFTER_CANCEL 界"]);
    test.resize_keeping(25, 61, &["AFTER_CANCEL 界"]);
    test.resize_keeping(38, 87, &["AFTER_CANCEL 界"]);
    // Release the old barrier after cancel: a stray timer must not resume it.
    let before = test.journal().iter().filter(|e| e["event"] == "session_update").count();
    test.release();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(test.journal().iter().filter(|e| e["event"] == "session_update").count(), before);
    assert_eq!(test.commands("cancel_turn").len(), 1);
    assert!(test.journal().iter().any(|e| e["terminal_reason"] == "aborted_streaming"));
    test.submit_draft();
    test.wait_screen("reply 2 started");
    test.assert_prompts(&["CANCEL_ME", "AFTER_CANCEL 界"]);
    test.shutdown();
}

#[test]
fn permission_accept_and_deny_survive_resize_and_send_one_correlated_response() {
    for (keys, option) in [(b"\r".as_slice(), "allow-once"), (b"\x1b".as_slice(), "deny-once")] {
        let mut test = TerminalTest::start("permission", 3);
        test.submit("PERMISSION", "PERMISSION");
        test.wait_screen_with("Allow once", &["Deny once"]);
        test.resize_keeping(25, 61, &["Allow once", "Deny once"]);
        test.resize_keeping(38, 87, &["Allow once", "Deny once"]);
        assert!(test.commands("permission_response").is_empty());
        test.send(keys);
        test.wait_screen("[READY]");
        let responses = test.commands("permission_response");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["session_id"], "fake-session");
        assert_eq!(responses[0]["tool_call_id"], "fixture-tool");
        assert_eq!(
            responses[0]["outcome"],
            serde_json::json!({"outcome": "selected", "option_id": option})
        );
        test.submit("AFTER_DIALOG", "AFTER_DIALOG");
        test.wait_screen("reply 2 started");
        test.assert_prompts(&["PERMISSION", "AFTER_DIALOG"]);
        test.shutdown();
    }
}

#[test]
fn question_selection_and_cancellation_survive_resize_without_submitting_a_prompt() {
    for answered in [true, false] {
        let mut test = TerminalTest::start("question", 3);
        test.submit("QUESTION", "QUESTION");
        test.wait_screen_with("Choose fixture destination", &["Beta"]);
        test.resize_keeping(25, 61, &["Choose fixture destination", "Beta"]);
        test.resize_keeping(38, 87, &["Choose fixture destination", "Beta"]);
        assert!(test.commands("question_response").is_empty());
        if answered {
            test.send(b"\x1b[C"); // Select Beta.
            test.send(b"\t"); // Edit notes while the question owns focus.
            test.send(b"fixture note");
            test.wait_screen("fixture note");
            test.resize_keeping(25, 61, &["fixture note"]);
            test.resize_keeping(38, 87, &["fixture note"]);
            test.send(b"\t");
            test.send(b"\r");
        } else {
            test.send(b"\x1b");
        }
        test.wait_screen("[READY]");
        let responses = test.commands("question_response");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["session_id"], "fake-session");
        assert_eq!(responses[0]["tool_call_id"], "fixture-tool");
        if answered {
            assert_eq!(responses[0]["outcome"]["outcome"], "answered");
            assert_eq!(responses[0]["outcome"]["selected_option_ids"], serde_json::json!(["beta"]));
            assert_eq!(responses[0]["outcome"]["annotation"]["notes"], "fixture note");
        } else {
            assert_eq!(responses[0]["outcome"], serde_json::json!({"outcome": "cancelled"}));
        }
        test.submit("AFTER_QUESTION", "AFTER_QUESTION");
        test.wait_screen("reply 2 started");
        test.assert_prompts(&["QUESTION", "AFTER_QUESTION"]);
        test.shutdown();
    }
}

#[test]
fn bridge_eof_and_malformed_output_report_failure_and_exit_without_extra_submission() {
    for scenario in ["hold-eof", "hold-malformed"] {
        let mut test = TerminalTest::start(scenario, 3);
        test.submit("FAIL_BRIDGE", "FAIL_BRIDGE");
        test.wait_journal("reply-held");
        test.paste("UNSENT_AT_FAILURE 界");
        test.wait_screen("UNSENT_AT_FAILURE 界");
        test.resize(25, 61);
        test.resize(38, 87);
        test.release();
        let status = test.child.wait_for_exit(Duration::from_secs(15)).expect("bounded fatal exit");
        assert!(!status.success(), "protocol failure must produce a failing exit status");
        test.assert_prompts(&["FAIL_BRIDGE"]);
        let raw = test.wait_final_output(if scenario == "hold-eof" {
            "exited before completing the protocol"
        } else {
            "failed to decode bridge event json"
        });
        assert!(!raw.contains(OWNED_REGION_ERROR));
        if scenario == "hold-malformed" {
            assert_eq!(
                test.commands("shutdown").len(),
                1,
                "a live failed bridge must be shut down"
            );
        }
    }
}

#[test]
fn resizing_a_streamed_reply_preserves_rendering_and_clean_shutdown() {
    let mut test = TerminalTest::start("stream", 1500);
    test.submit("hello\n\nhow are you", "how are you");
    // The stream scrolls the prompt off the screen, so it is read from the
    // whole transcript. Terminal padding can leave spaces on visually blank rows.
    test.wait_until("submitted paragraph gap", |test| {
        let transcript = test.transcript_rows();
        let rows: Vec<_> = transcript.iter().map(|row| row.trim_end()).collect();
        rows.windows(3).any(|rows| rows == ["hello", "", "how are you"])
    });
    test.assert_prompts(&["hello\n\nhow are you"]);

    for step in 0..RESIZES {
        let rows = if step % 2 == 0 { TALL_ROWS } else { SHORT_ROWS };
        test.resize(rows, COLS);
        std::thread::sleep(Duration::from_millis(40 + u64::from(step % 7) * 23));
        assert!(
            test.child.process.try_wait().expect("poll child").is_none(),
            "child exited after {} resizes:\n{}",
            step + 1,
            test.diagnostics()
        );
    }

    test.wait_screen("streamed line 1500");
    test.wait_journal("turn_complete");
    test.submit("again", "again");
    test.wait_screen("reply 2 started");
    test.assert_prompts(&["hello\n\nhow are you", "again"]);
    test.shutdown();
}

#[test]
fn width_and_height_resizes_preserve_unicode_multiline_draft_and_exact_submission() {
    let mut test = TerminalTest::start("hold-success", 8);
    test.submit("go", "❯ go");
    test.wait_journal("reply-held");
    test.wait_screen("streamed line 8");

    let draft = "keep 界 🦀 e\u{301}\nsecond paragraph";
    test.paste(draft);
    test.wait_screen("second paragraph");
    for _ in 0..3 {
        for (rows, cols) in [(18, 61), (55, 120), (12, 30), (38, 87), (38, 87)] {
            test.resize(rows, cols);
            std::thread::sleep(Duration::from_millis(40));
        }
    }
    test.resize_keeping(30, 100, &["keep 界 🦀 e\u{301}", "second paragraph"]);
    test.assert_prompts(&["go"]); // Resizing must not submit the draft.
    test.release();
    test.wait_journal("turn_complete");
    test.submit_draft();
    test.wait_screen("reply 2 started");
    test.assert_prompts(&["go", draft]);
    test.shutdown();
}

#[test]
fn fullscreen_resize_and_repeated_return_preserve_chat_and_next_submission() {
    let mut test = TerminalTest::start("hold-success", 8);
    test.submit("go", "❯ go");
    test.wait_journal("reply-held");
    test.wait_screen("streamed line 8");

    for (rows, cols) in [(55, 120), (25, 61), (38, 87)] {
        test.submit("/config", "/config");
        test.wait_screen("Saved in User");
        test.wait_settings_ready("Description:");
        assert!(test.output.lock().expect("output lock").parser.screen().alternate_screen());
        test.send(b" ");
        test.wait_screen("Enter save");
        test.send(b" draft");
        test.wait_screen("German draft");
        test.resize_keeping(rows, cols, &["German draft"]);
        test.send(b"\x1b");
        test.wait_until("editor closed without saving", |test| {
            let screen = test.screen();
            screen.contains("Saved in User") && !screen.contains("German draft")
        });
        test.send(b"\x1b");
        test.wait_screen("streamed line 8");
        assert!(!test.output.lock().expect("output lock").parser.screen().alternate_screen());
        test.assert_prompts(&["go"]); // /config is handled locally.
    }

    test.release();
    test.wait_journal("turn_complete");
    test.submit("after settings 界", "after settings 界");
    test.wait_screen("reply 2 started");
    test.assert_prompts(&["go", "after settings 界"]);
    test.shutdown();
}

#[test]
fn structured_settings_editor_adds_individual_rules_and_preserves_them_on_resize() {
    let mut test = TerminalTest::start("hold-success", 8);
    std::fs::write(
        test.temp.path().join("profile/settings.json"),
        r#"{"permissions":{"defaultMode":"default"}}"#,
    )
    .expect("initial settings");
    test.submit("go", "go");
    test.wait_journal("reply-held");
    test.submit("/permissions", "/permissions");
    test.wait_settings_ready("Permissions: deny rules");
    test.send(b" ");
    test.wait_screen("Ctrl+S save");
    test.send(b"a");
    test.paste("Bash(git push *)");
    test.send(b"\r");
    test.wait_screen("Bash(git push *)");
    for (rows, cols) in [(18, 61), (38, 87), (55, 120)] {
        test.resize_keeping(rows, cols, &["Bash(git push *)"]);
    }
    test.send(b"\x13");
    test.wait_until("settings mutation", |test| test.commands("mutate_setting").len() == 1);
    assert_eq!(
        test.commands("mutate_setting")[0]["mutation"]["value"],
        serde_json::json!(["Read(./.env)", "Bash(git push *)"])
    );
    test.wait_settings_saved("2 items", "Bash(git push *)");
    let document: Value = serde_json::from_str(
        &std::fs::read_to_string(test.temp.path().join("profile/settings.json"))
            .expect("saved file"),
    )
    .expect("settings object");
    assert_eq!(
        document["permissions"]["deny"],
        serde_json::json!(["Read(./.env)", "Bash(git push *)"])
    );
    assert_eq!(document["permissions"]["defaultMode"], "default");
    test.send(b"\x1b");
    test.wait_screen("streamed line 8");
    test.assert_prompts(&["go"]);
    test.release();
    test.wait_journal("turn_complete");
    test.shutdown();
}

#[test]
fn guided_hook_creation_survives_resize_and_saves_a_complete_hook_with_existing_data() {
    let mut test = TerminalTest::start("hold-hooks", 8);
    std::fs::write(
        test.temp.path().join("profile/settings.json"),
        r#"{"permissions":{"defaultMode":"default"},"unrelated":"keep"}"#,
    )
    .expect("initial settings");
    test.submit("go", "go");
    test.wait_journal("reply-held");
    test.submit("/hooks", "/hooks");
    test.wait_settings_ready("Definitions");
    test.send(b"\r");
    test.wait_screen("Stop");
    test.send(b"a");
    test.wait_screen("Choose event");
    test.send(b"\x1b[H\r");
    test.wait_screen("Matcher (optional)");
    test.paste("Write|Edit");
    test.send(b"\r");
    test.wait_screen("Choose action");
    test.wait_screen("mcp_tool");
    test.send(b"\r");
    test.wait_screen("Command *");
    test.paste("npm run lint");
    test.send(b"\r");
    test.wait_screen("Review");
    test.send(b"\r\x1b[B");
    for (rows, cols) in [(16, 42), (38, 87), (55, 120)] {
        test.resize_keeping(rows, cols, &["npm run lint"]);
    }
    test.send(b"\x13");
    test.wait_until("hook settings mutation", |test| test.commands("mutate_setting").len() == 1);
    let expected = serde_json::json!({"PreToolUse":[{"matcher":"Write|Edit","hooks":[{"type":"command","command":"npm run lint"}]}],"Stop":[{"hooks":[{"type":"command","command":"keep-original","future":"keep"}]}]});
    assert_eq!(test.commands("mutate_setting")[0]["mutation"]["value"], expected);
    test.wait_setting("profile/settings.json", "/hooks", Some(&expected));
    let saved: Value = serde_json::from_slice(
        &std::fs::read(test.temp.path().join("profile/settings.json")).expect("saved file"),
    )
    .expect("settings JSON");
    assert_eq!(saved["hooks"], expected);
    assert_eq!(saved["unrelated"], "keep");
    assert_eq!(saved["permissions"]["defaultMode"], "default");
    test.wait_settings_saved("2 entries", "npm run lint");
    test.send(b"\x1b");
    test.wait_screen("streamed line 8");
    test.assert_prompts(&["go"]);
    test.release();
    test.wait_journal("turn_complete");
    test.shutdown();
}

#[test]
fn unicode_pastes_cross_the_placeholder_boundary_without_payload_loss() {
    let mut test = TerminalTest::start("stream", 3);
    let mut expected = Vec::new();
    for chars in [999, 1000, 1001] {
        let prefix = "Unicode 界 🦀\n";
        let suffix = format!("\nEDGE_{chars}");
        let payload = format!(
            "{prefix}{}{suffix}",
            "x".repeat(chars - prefix.chars().count() - suffix.chars().count())
        );
        let start_marker = "\x1b[200~";
        let framed_input = format!("{start_marker}{payload}\x1b[201~");
        // Exercise transport fragmentation on native Unix PTYs. ConPTY
        // converts fragmented escape sequences into console key records, so
        // Windows receives the complete terminal paste action in one write.
        if cfg!(windows) {
            test.send(framed_input.as_bytes());
        } else {
            // The start marker stays whole: a read that ends on its ESC byte
            // is an Esc key press, not the beginning of a paste.
            let (start, rest) = framed_input.as_bytes().split_at(start_marker.len());
            test.send(start);
            for byte in rest {
                test.send(&[*byte]);
            }
        }
        let visible =
            if chars > 1000 { "[Pasted Text".to_owned() } else { format!("EDGE_{chars}") };
        test.wait_screen(&visible);
        test.resize(25, 61);
        test.resize(38, 87);
        test.submit_draft();
        let reply = format!("reply {} started", expected.len() + 1);
        test.wait_screen(&reply);
        expected.push(payload);
        let texts: Vec<_> = expected.iter().map(String::as_str).collect();
        test.assert_prompts(&texts);
        let prompts = test.prompts();
        let pastes = prompts.last().expect("prompt recorded")["inline_pastes"].as_array();
        assert_eq!(pastes.map_or(0, Vec::len), usize::from(chars > 1000));
        if chars > 1000 {
            assert_eq!(
                pastes.expect("folded paste")[0].as_str(),
                expected.last().map(String::as_str)
            );
        }
    }
    test.shutdown();
}

#[test]
fn a_stream_error_preserves_the_next_unsent_draft_through_resize() {
    let mut test = TerminalTest::start("hold-error", 8);
    test.submit("go", "❯ go");
    test.wait_journal("reply-held");

    test.paste("NEXT_DRAFT 界 🦀\nkeep this paragraph");
    test.wait_screen("keep this paragraph");
    test.release();
    test.wait_journal("turn_error");
    let draft = ["NEXT_DRAFT 界 🦀", "keep this paragraph"];
    test.wait_screen_with("Fixture service unavailable", &draft);
    test.resize_keeping(55, 120, &draft);
    test.resize_keeping(38, 87, &draft);
    test.assert_prompts(&["go"]);
    test.shutdown();
}

#[test]
fn presentation_clocks_and_copy_picker_navigation_survive_terminal_resize() {
    let mut test = TerminalTest::start("presentation", 3);
    test.submit("Clock test", "Clock test");
    test.wait_screen("Elapsed 2.0s");
    test.wait_screen("Done ");
    test.wait_screen("15:02Z");
    test.submit("/copy", "/copy");
    // The command menu already shows the picker's title as its description.
    test.wait_screen_with("Code block 1 (rust)", &["Copy last response"]);
    test.send(b"\x1b[B");
    test.resize_keeping(24, 42, &["Code block 1 (rust)"]);
    test.send(b"\x1b");
    test.wait_screen("Type a message");
    test.submit("Follow up", "Follow up");
    test.wait_screen("reply 2 started");
    test.assert_prompts(&["Clock test", "Follow up"]);
    test.shutdown();
}

#[test]
fn scrollback_capture_honors_split_saved_lines_erasure_without_changing_visible_rows() {
    let mut parser = vt100::Parser::new_with_callbacks(4, 20, SCROLLBACK_ROWS, ScrollbackCallbacks);
    parser.process(b"old row\r\nvisible 1\r\nvisible 2\r\nvisible 3\r\nvisible 4");
    let visible = parser.screen().contents();
    let cursor = parser.screen().cursor_position();
    parser.screen_mut().set_scrollback(usize::MAX);
    assert!(parser.screen().scrollback() > 0);
    assert!(parser.screen().contents().contains("old row"));
    parser.screen_mut().set_scrollback(0);
    parser.process(b"\x1b[");
    parser.process(b"3J");
    parser.screen_mut().set_scrollback(usize::MAX);
    assert_eq!(parser.screen().scrollback(), 0);
    assert_eq!(parser.screen().contents(), visible);
    assert_eq!(parser.screen().cursor_position(), cursor);
}

#[test]
fn completed_transcript_remains_scrollable_after_zoom_in_and_out() {
    let mut test = TerminalTest::start_with_options(
        "resize-ready",
        80,
        None,
        &["--log-filter", "warn,app.render=debug,bridge.protocol=debug"],
        false,
    );
    test.submit("SCROLLBACK_CONTROL", "SCROLLBACK_CONTROL");
    test.wait_turn_finished("streamed line 80");
    test.wait_completed_reply_in_scrollback(80);
    for (rows, cols) in [(30, 64), (2, 64), (1, 64), (55, 120), (38, 87)] {
        test.resize_and_wait_for_draw(rows, cols);
        test.wait_completed_reply_in_scrollback(80);
    }
    test.shutdown();
}

#[test]
fn a_new_resize_supersedes_active_replay_and_restores_history_at_the_latest_size() {
    let mut test = TerminalTest::start_with_options(
        "resize-replay",
        2_000,
        None,
        &["--log-filter", "warn,app.render=debug,bridge.protocol=debug"],
        false,
    );
    test.submit("LARGE_REPLAY", "LARGE_REPLAY");
    test.wait_turn_finished("streamed line 2000");
    test.wait_completed_reply_in_scrollback(2_000);
    let offset = std::fs::read(test.temp.path().join("runtime.log")).expect("runtime log").len();
    test.resize(12, 64);
    test.wait_until("replay has started inserting history", |test| {
        test.runtime_records_since(offset).iter().any(|record| {
            record["event_name"] == "inline_chat_history_insert_request"
                && record["batch_kind"] == "Replay"
        })
    });
    test.resize_and_wait_for_draw(55, 120);
    test.wait_completed_reply_in_scrollback(2_000);
    let replay_frames: Vec<_> = test
        .runtime_records_since(offset)
        .into_iter()
        .filter(|record| {
            record["event_name"] == "inline_chat_terminal_draw_transaction"
                && (record["replay_incomplete"] == true || record["replay_complete"] == true)
        })
        .collect();
    test.shutdown();
    assert!(
        replay_frames.iter().any(|record| record["replay_incomplete"] == true),
        "a large replay must yield before completing: {replay_frames:?}",
    );
    assert!(
        replay_frames
            .iter()
            .all(|record| record["flushed_rows"].as_u64().is_some_and(|rows| rows <= 160)),
        "a single large message must respect the replay row budget: {replay_frames:?}",
    );
    assert!(
        test.runtime_records_since(offset).iter().any(|record| {
            record["event_name"] == "inline_chat_purge_replay_cleared"
                && record["terminal_height"] == 55
                && record["terminal_width"] == 120
        }),
        "the latest resize must clear stale replay output before restoring history",
    );
}

#[test]
fn completed_transcript_remains_scrollable_when_zooming_during_an_unfinished_code_block() {
    let mut test = TerminalTest::start_with_options(
        "resize-code",
        80,
        None,
        &["--log-filter", "warn,app.render=debug,bridge.protocol=debug"],
        false,
    );
    test.submit("COMPLETED_PREFIX", "COMPLETED_PREFIX");
    test.wait_turn_finished("streamed line 80");
    test.wait_completed_reply_in_scrollback(80);
    test.submit("UNFINISHED_CODE_BLOCK", "UNFINISHED_CODE_BLOCK");
    test.wait_journal("reply-held");
    test.wait_screen("code line 80");
    test.resize_keeping(30, 64, &["code line 80"]);
    let prefix_survived_zoom_in =
        test.transcript_rows().iter().any(|row| row.trim_end().ends_with("streamed line 1"));
    let zoom_in_diagnostics = test.diagnostics();
    test.resize_and_wait_for_draw(120, 120);
    test.wait_completed_reply_in_scrollback(80);
    test.release();
    test.wait_screen("[READY]");
    test.shutdown();
    assert!(
        prefix_survived_zoom_in,
        "zooming in lost the completed transcript; zooming out restored it:\n{zoom_in_diagnostics}",
    );
}

#[test]
fn completed_transcript_remains_scrollable_when_resizing_in_reading_mode() {
    let mut test = TerminalTest::start_with_options(
        "resize-ready",
        80,
        None,
        &["--log-filter", "warn,app.render=debug,bridge.protocol=debug"],
        false,
    );
    test.submit("READING_PREFIX", "READING_PREFIX");
    test.wait_turn_finished("streamed line 80");
    test.wait_completed_reply_in_scrollback(80);
    test.send(b"\x1b[5~");
    test.wait_screen("Reading output");
    // Establish actual terminal history before the resize clears it.
    test.wait_completed_reply_in_scrollback(80);
    test.resize_keeping(30, 64, &["Reading output"]);
    test.wait_completed_reply_in_scrollback(80);
    test.shutdown();
}

#[test]
fn reading_output_stays_anchored_through_streaming_resize_and_explicit_return_live() {
    let mut test = TerminalTest::start("hold-presentation", 80);
    test.submit("Reading test", "Reading test");
    test.wait_journal("reading-barrier");
    // The barrier is the bridge's view. Page Up must reach an app that has
    // already rendered the held output.
    test.wait_screen("streamed line 20");
    test.send(b"\x1b[5~"); // Page Up enters reading mode.
    let pinned = test.wait_pinned_output();
    test.release();
    let completed = test.wait_screen_with("[READY]", &[&pinned]);
    assert!(
        !completed.contains("streamed line 80"),
        "paused output jumped to the tail: {}",
        test.diagnostics()
    );
    test.resize_keeping(TALL_ROWS, 64, &[&pinned]);
    test.send(b"\x1b[1;5F"); // Ctrl+End explicitly returns to live output.
    test.wait_screen("streamed line 80");
    test.wait_until("following restored", |test| !test.screen().contains("Reading output"));
    test.shutdown();
}

#[test]
fn saved_auto_scroll_off_holds_new_output_until_the_user_returns_live() {
    let mut test = TerminalTest::start("hold-presentation", 80);
    test.submit("Saved scroll test", "Saved scroll test");
    test.wait_journal("reading-barrier");
    test.submit("/config", "/config");
    test.wait_settings_ready("Auto-scroll");
    test.send(b"\x1b[C"); // On -> Off at the selected user scope.
    test.wait_setting("profile/settings.json", "/autoScrollEnabled", Some(&Value::Bool(false)));
    test.send(b"\x1b");
    let pinned = test.wait_pinned_output();
    test.release();
    test.wait_screen_with("[READY]", &[&pinned]);
    test.send(b"\x1b[1;5F");
    // The saved Off preference still holds after the jump.
    test.wait_screen_with("streamed line 80", &["Reading output"]);
    let saved: Value = serde_json::from_slice(
        &std::fs::read(test.temp.path().join("profile/settings.json")).expect("saved preferences"),
    )
    .expect("settings JSON");
    assert_eq!(saved["autoScrollEnabled"], false);
    test.shutdown();
}

#[allow(clippy::expect_used)]
fn notification_bells(test: &TerminalTest) -> usize {
    let output = test.output.lock().expect("captured terminal");
    let mut in_osc = false;
    let mut bells = 0;
    for (index, byte) in output.raw.iter().enumerate() {
        if *byte == b']' && index > 0 && output.raw[index - 1] == 0x1b {
            in_osc = true;
        }
        if *byte == 7 {
            if !in_osc {
                bells += 1;
            }
            in_osc = false;
        }
        if *byte == b'\\' && index > 0 && output.raw[index - 1] == 0x1b {
            in_osc = false;
        }
    }
    bells
}

#[test]
fn notifications_follow_focus_saved_categories_and_sdk_delivery_provenance_in_a_real_terminal() {
    let mut test = TerminalTest::start_with_options(
        "notifications",
        3,
        None,
        &["--diagnostics-preset", "full"],
        false,
    );
    test.send(b"\x1b[O"); // Focus lost.
    test.submit("Notify test", "Notify test");
    test.wait_journal("turn_complete");
    test.wait_until("one proactive bell", |test| notification_bells(test) == 1);
    test.wait_screen("Native notice 1"); // A visible notice does not imply a desktop alert.
    test.send(b"\x1b[I"); // Focus gained.
    test.submit("Focused test", "Focused test");
    // The bell count is only meaningful once the app has applied the turn.
    test.wait_screen_with("Native notice 2", &["[READY]"]);
    assert_eq!(notification_bells(&test), 1);
    test.submit("/config", "/config");
    test.wait_settings_ready("Notification method");
    test.send(b"\x1b[B\x1b[B"); // Pass turn completion and select proactive alerts.
    test.send(b" "); // On -> Off, immediate acknowledged save.
    test.wait_setting(
        "profile/app-settings.json",
        "/notifications/modelDirected",
        Some(&Value::Bool(false)),
    );
    test.send(b"\x1b");
    test.wait_screen("Type a message");
    test.send(b"\x1b[O");
    test.submit("Disabled category", "Disabled category");
    test.wait_screen_with("Native notice 3", &["[READY]"]);
    assert_eq!(notification_bells(&test), 1);
    let saved: Value = serde_json::from_slice(
        &std::fs::read(test.temp.path().join("profile/app-settings.json"))
            .expect("saved categories"),
    )
    .expect("category JSON");
    assert_eq!(saved["notifications"]["modelDirected"], false);
    assert_eq!(saved["notifications"]["turnComplete"], false);
    assert_eq!(saved["notifications"]["actionsRequired"], true);
    test.assert_prompts(&["Notify test", "Focused test", "Disabled category"]);
    test.shutdown();
    let diagnostics = std::fs::read_to_string(test.temp.path().join("runtime.log"))
        .expect("notification diagnostics");
    let records: Vec<Value> = diagnostics
        .lines()
        .map(|line| serde_json::from_str(line).expect("diagnostic JSON"))
        .collect();
    for reason in
        ["duplicate", "replay", "upstream_local_delivery", "terminal_focused", "category_disabled"]
    {
        assert!(
            records
                .iter()
                .any(|record| record["target"] == "app.notify" && record["reason"] == reason),
            "missing {reason}"
        );
    }
    assert!(records.iter().any(|record| record["event_name"] == "notification_focus_observed"
        && record["terminal_focused"] == false));
    let bell = records
        .iter()
        .find(|record| {
            record["event_name"] == "notification_transport_result" && record["transport"] == "bell"
        })
        .expect("bell result");
    assert_eq!(bell["outcome"], "success");
    assert_eq!(bell["span"]["tool_call_id"], "push-1");
}
