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
    temp: tempfile::TempDir,
}

#[allow(clippy::expect_used, clippy::panic)]
impl TerminalTest {
    fn start(scenario: &str, lines: u16) -> Self {
        Self::start_with_auth(scenario, lines, None)
    }

    fn start_with_auth(scenario: &str, lines: u16, auth_mode: Option<&str>) -> Self {
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
        command.arg("--log-file");
        command.arg(temp.path().join("runtime.log"));
        command.env("RUST_LOG", "warn,app.lifecycle=debug,bridge.protocol=debug");
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
        let mut test = Self { child, master: pair.master, output, journal, release_file, temp };
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
            "{}\nRaw output tail:\n{}\nRuntime log tail:\n{}",
            output.parser.screen().contents(),
            tail(&String::from_utf8_lossy(&output.raw), 2000),
            tail(
                &std::fs::read_to_string(self.temp.path().join("runtime.log")).unwrap_or_default(),
                6000
            )
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
        test.wait_screen("AUTH_CHILD_READ");
        let profile = test.temp.path().join("profile");
        let received =
            std::fs::read_to_string(profile.join("auth-input")).expect("child stdin record");
        assert_eq!(received.trim_end_matches(['\r', '\n']), "child_only_407");
        std::fs::write(profile.join("auth-release"), b"continue").expect("release auth child");
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
fn auth_spawn_failure_restores_terminal_ownership_and_input() {
    let mut test = TerminalTest::start_with_auth("stream", 3, Some("spawn-error"));
    test.submit("/login", "/login");
    test.wait_screen("Failed to run claude auth login");
    test.wait_screen("[READY]");
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
    let output = test.output.lock().expect("output lock");
    assert!(String::from_utf8_lossy(&output.raw).contains("exited before completing the protocol"));
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
        test.wait_screen("+ PASTE 界 🦀");
        test.submit_draft();
        test.wait_screen(&format!("reply {} started", index + 1));
        test.wait_screen("[READY]");
        expected.push(format!("{typed} + PASTE 界 🦀"));
        test.assert_prompts(&expected.iter().map(String::as_str).collect::<Vec<_>>());
    }
    test.shutdown();
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
    test.wait_screen("reply 2 started");
    test.wait_screen("[READY]");
    test.wait_screen("UNSENT 🦀");
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
    test.wait_screen("[READY]");
    test.resize(25, 61);
    test.resize(38, 87);
    test.wait_screen("AFTER_CANCEL 界");
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
        test.wait_screen("Allow once");
        test.resize(25, 61);
        test.resize(38, 87);
        test.wait_screen("Deny once");
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
        test.wait_screen("Choose fixture destination");
        test.resize(25, 61);
        test.resize(38, 87);
        test.wait_screen("Beta");
        assert!(test.commands("question_response").is_empty());
        if answered {
            test.send(b"\x1b[C"); // Select Beta.
            test.send(b"\t"); // Edit notes while the question owns focus.
            test.send(b"fixture note");
            test.wait_screen("fixture note");
            test.resize(25, 61);
            test.resize(38, 87);
            test.wait_screen("fixture note");
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
        let output = test.output.lock().expect("output lock");
        let raw = String::from_utf8_lossy(&output.raw);
        let expected = if scenario == "hold-eof" {
            "exited before completing the protocol"
        } else {
            "failed to decode bridge event json"
        };
        assert!(raw.contains(expected), "missing user-facing error: {}", tail(&raw, 4000));
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
