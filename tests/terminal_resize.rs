// SPDX-License-Identifier: Apache-2.0
// End-to-end terminal test: runs the real `claude-rs` binary in a pseudo
// terminal against `tests/fixtures/fake-bridge.js` and resizes the terminal
// while a reply is streaming.

use portable_pty::{Child, CommandBuilder, ExitStatus, PtySize, native_pty_system};
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
}

struct TestChild {
    process: Box<dyn Child + Send + Sync>,
    writer: PtyWriter,
}

impl TestChild {
    #[allow(clippy::expect_used)]
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
        // Clean up this test's child even when an assertion fails. Give the app
        // time to shut down its fixture bridge before stopping the TUI itself.
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
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn pty_size(rows: u16) -> PtySize {
    PtySize { rows, cols: COLS, pixel_width: 0, pixel_height: 0 }
}

#[allow(clippy::expect_used)]
fn bridge_runtime() -> PathBuf {
    which::which("bun")
        .or_else(|_| which::which("node"))
        .expect("the terminal test needs `bun` or `node` on PATH to run the fake bridge")
}

#[allow(clippy::expect_used)]
fn wait_for(output: &Mutex<CapturedTerminal>, needle: &str, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if output.lock().expect("output lock").parser.screen().contents().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn tail(text: &str, chars: usize) -> String {
    let skip = text.chars().count().saturating_sub(chars);
    text.chars().skip(skip).collect::<String>().replace('\x1b', "<ESC>")
}

#[allow(clippy::expect_used)]
fn output_tail(output: &Mutex<CapturedTerminal>) -> String {
    let output = output.lock().expect("output lock");
    format!(
        "{}\nRaw output tail:\n{}",
        output.parser.screen().contents(),
        tail(&String::from_utf8_lossy(&output.raw), 2000)
    )
}

#[allow(clippy::expect_used)]
fn capture_output(
    mut reader: Box<dyn Read + Send>,
    writer: PtyWriter,
) -> Arc<Mutex<CapturedTerminal>> {
    let output = Arc::new(Mutex::new(CapturedTerminal {
        parser: vt100::Parser::new(SHORT_ROWS, COLS, 0),
        raw: Vec::new(),
    }));
    let captured = Arc::clone(&output);
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        let mut query = Vec::new();
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            // Answer the cursor-position query a terminal emulator would answer.
            // Queries and UTF-8 characters may be split across reads.
            query.extend_from_slice(&buffer[..read]);
            if query.windows(4).any(|s| s == b"\x1b[6n") {
                let _ = writer.lock().expect("writer lock").write_all(b"\x1b[1;1R");
            }
            query.drain(..query.len().saturating_sub(3));
            let mut captured = captured.lock().expect("output lock");
            captured.parser.process(&buffer[..read]);
            captured.raw.extend_from_slice(&buffer[..read]);
        }
    });
    output
}

#[test]
fn resizing_a_streamed_reply_preserves_rendering_and_clean_shutdown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let profile = temp.path().join("profile");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&profile).expect("profile");
    std::fs::create_dir_all(&project).expect("project");
    let bridge = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-bridge.js");

    let pair = native_pty_system().openpty(pty_size(SHORT_ROWS)).expect("open pty");
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_claude-rs"));
    command.arg("--no-update-check");
    command.arg("--dir");
    command.arg(&project);
    command.cwd(&project);
    command.env("CLAUDE_CONFIG_DIR", &profile);
    command.env("CLAUDE_RS_AGENT_BRIDGE", &bridge);
    command.env("CLAUDE_RS_AGENT_BRIDGE_RUNTIME", bridge_runtime());
    command.env("FAKE_BRIDGE_LINES", "1500");
    command.env("FAKE_BRIDGE_INTERVAL_MS", "15");
    command.env("FAKE_BRIDGE_FOLLOW_UP_LINES", "3");
    let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("pty writer")));
    let reader = pair.master.try_clone_reader().expect("pty reader");
    let process = pair.slave.spawn_command(command).expect("spawn claude-rs");
    let mut child = TestChild { process, writer: Arc::clone(&writer) };
    drop(pair.slave);

    let output = capture_output(reader, Arc::clone(&writer));
    let send = |bytes: &[u8]| {
        let mut writer = writer.lock().expect("writer lock");
        writer.write_all(bytes).expect("write to pty");
        writer.flush().expect("flush pty");
    };

    assert!(
        wait_for(&output, "Trust this project", Duration::from_secs(20)),
        "trust dialog never appeared:\n{}",
        output_tail(&output)
    );
    send(b"y");
    assert!(
        wait_for(&output, "Type a message", Duration::from_secs(20)),
        "composer never became available:\n{}",
        output_tail(&output)
    );
    send(b"go");
    std::thread::sleep(Duration::from_millis(500));
    send(b"\r");
    assert!(
        wait_for(&output, "streamed line 5", Duration::from_secs(20)),
        "the fake bridge reply never rendered:\n{}",
        output_tail(&output)
    );

    let mut exit = None;
    let mut resizes = 0;
    for step in 0..RESIZES {
        let rows = if step % 2 == 0 { TALL_ROWS } else { SHORT_ROWS };
        output.lock().expect("output lock").parser.screen_mut().set_size(rows, COLS);
        pair.master.resize(pty_size(rows)).expect("resize pty");
        resizes = step + 1;
        // Vary the gap so resizes land at different points of the draw loop.
        std::thread::sleep(Duration::from_millis(40 + u64::from(step % 7) * 23));
        exit = child.process.try_wait().expect("poll claude-rs");
        if exit.is_some() {
            break;
        }
    }

    assert!(
        exit.is_none(),
        "claude-rs exited after {resizes} resizes ({exit:?}):\n{}",
        output_tail(&output)
    );
    assert!(
        wait_for(&output, "streamed line 1500", Duration::from_secs(20)),
        "the reply did not finish rendering after resizing:\n{}",
        output_tail(&output)
    );
    send(b"again");
    std::thread::sleep(Duration::from_millis(500));
    send(b"\r");
    assert!(
        wait_for(&output, "reply 2 started", Duration::from_secs(20)),
        "the app did not accept and render a new reply after resizing:\n{}",
        output_tail(&output)
    );
    send(b"\x11"); // Ctrl+Q: exercise the normal shutdown path.
    let status = child.wait_for_exit(Duration::from_secs(15));
    assert!(
        status.as_ref().is_some_and(ExitStatus::success),
        "claude-rs did not exit cleanly ({status:?}):\n{}",
        output_tail(&output)
    );
    let output = output.lock().expect("output lock");
    let text = String::from_utf8_lossy(&output.raw);
    assert!(!text.contains(OWNED_REGION_ERROR), "{}", tail(&text, 2000));
}
