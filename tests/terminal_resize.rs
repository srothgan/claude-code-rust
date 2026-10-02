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
const OWNED_REGION_ERROR: &str = "refusing to clear outside inline chat owned region";
type PtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;

struct CapturedTerminal {
    parser: vt100::Parser,
    raw: Vec<u8>,
    ended: bool,
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
        parser: vt100::Parser::new(SHORT_ROWS, COLS, 0),
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
    _temp: tempfile::TempDir,
}

#[allow(clippy::expect_used, clippy::panic)]
impl TerminalTest {
    fn start(scenario: &str, lines: u16) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let profile = temp.path().join("profile");
        let project = temp.path().join("project");
        let journal = temp.path().join("commands.jsonl");
        let release_file = temp.path().join("release");
        std::fs::create_dir_all(&profile).expect("profile");
        std::fs::create_dir_all(&project).expect("project");
        let bridge = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-bridge.js");
        let runtime = which::which("bun")
            .or_else(|_| which::which("node"))
            .expect("terminal tests need bun or node on PATH");

        let pair = native_pty_system().openpty(pty_size(SHORT_ROWS, COLS)).expect("open pty");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_claude-rs"));
        command.arg("--no-update-check");
        command.arg("--dir");
        command.arg(&project);
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
        let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("pty writer")));
        let reader = pair.master.try_clone_reader().expect("pty reader");
        let process = pair.slave.spawn_command(command).expect("spawn claude-rs");
        let child = TestChild { process, writer: Arc::clone(&writer) };
        drop(pair.slave);
        let output = capture_output(reader, writer);
        let mut test =
            Self { child, master: pair.master, output, journal, release_file, _temp: temp };
        test.wait_screen("Trust this project");
        test.send(b"y");
        test.wait_screen("Type a message");
        // The composer is editable during Connecting, but Enter cannot submit
        // until the connected event has been applied and painted.
        test.wait_screen("[READY]");
        test
    }

    fn send(&self, bytes: &[u8]) {
        let mut writer = self.child.writer.lock().expect("writer lock");
        writer.write_all(bytes).expect("write to pty");
        writer.flush().expect("flush pty");
    }

    fn paste(&self, text: &str) {
        self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes());
    }

    fn screen(&self) -> String {
        self.output.lock().expect("output lock").parser.screen().contents()
    }

    fn diagnostics(&self) -> String {
        let output = self.output.lock().expect("output lock");
        format!(
            "{}\nRaw output tail:\n{}",
            output.parser.screen().contents(),
            tail(&String::from_utf8_lossy(&output.raw), 2000)
        )
    }

    fn wait_until(&mut self, description: &str, predicate: impl Fn(&Self) -> bool) {
        let started = Instant::now();
        let mut observed = false;
        while started.elapsed() < Duration::from_secs(20) {
            // Require two observations, so a transient partial paint is not
            // sufficient to advance the test.
            let matches = predicate(self);
            if matches && observed {
                return;
            }
            observed = matches;
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

    fn wait_screen(&mut self, needle: &str) {
        self.wait_until(needle, |test| test.screen().contains(needle));
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
        self.journal().into_iter().filter(|entry| entry["command"] == "prompt").collect()
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
        self.paste(text);
        self.wait_screen(visible);
        self.submit_draft();
    }

    fn submit_draft(&self) {
        // Windows falls back to key-burst paste detection. Its 250 ms trailing
        // Enter suppression deliberately treats an immediate CR as paste text.
        std::thread::sleep(Duration::from_millis(500));
        self.send(b"\r");
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.output.lock().expect("output lock").parser.screen_mut().set_size(rows, cols);
        self.master.resize(pty_size(rows, cols)).expect("resize pty");
    }

    fn release(&self) {
        std::fs::write(&self.release_file, b"continue").expect("release fixture barrier");
    }

    fn shutdown(&mut self) {
        self.send(b"\x11"); // Normal Ctrl+Q shutdown, not a forced kill.
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
}

#[test]
fn resizing_a_streamed_reply_preserves_rendering_and_clean_shutdown() {
    let mut test = TerminalTest::start("stream", 1500);
    test.submit("hello\n\nhow are you", "how are you");
    test.wait_screen("streamed line 5");
    assert!(
        test.screen().contains("hello\n\nhow are you"),
        "submitted paragraph gap disappeared:\n{}",
        test.diagnostics()
    );
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
    test.wait_screen("keep 界 🦀 e\u{301}");
    test.wait_screen("second paragraph");
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
        test.wait_screen("Always Thinking");
        assert!(test.output.lock().expect("output lock").parser.screen().alternate_screen());
        test.resize(rows, cols);
        test.wait_screen("Always Thinking");
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
        let framed_input = format!("\x1b[200~{payload}\x1b[201~");
        // Exercise transport fragmentation on native Unix PTYs. ConPTY
        // converts fragmented escape sequences into console key records, so
        // Windows receives the complete terminal paste action in one write.
        if cfg!(windows) {
            test.send(framed_input.as_bytes());
        } else {
            for byte in framed_input.as_bytes() {
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
    test.wait_screen("Fixture service unavailable");
    test.resize(55, 120);
    test.resize(38, 87);
    test.wait_screen("NEXT_DRAFT 界 🦀");
    test.wait_screen("keep this paragraph");
    test.assert_prompts(&["go"]);
    test.shutdown();
}
