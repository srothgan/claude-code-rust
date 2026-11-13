// SPDX-License-Identifier: Apache-2.0
// Linux provides verified HOME/XDG isolation for diagnostics as well as app preferences.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]
#[allow(dead_code)]
#[path = "support/update_fixture.rs"]
mod fixture;
use fixture::{Fixture, wait};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
struct AppProcess {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    screen: Arc<Mutex<String>>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}
impl AppProcess {
    fn send(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    }
    fn wait_text(&self, text: &str) {
        wait(text, || self.screen.lock().unwrap().contains(text));
    }
    fn exit(&mut self) {
        self.send(b"\x11");
        let mut status = None;
        wait("normal app exit before installer release", || {
            status = self.child.try_wait().unwrap();
            status.is_some()
        });
        assert!(status.unwrap().success());
    }
}
impl Drop for AppProcess {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
fn launch(f: &Fixture, exe: &Path, profile: &Path, tools: &Path, code: i32) -> AppProcess {
    let pair = native_pty_system()
        .openpty(PtySize { rows: 38, cols: 100, pixel_width: 0, pixel_height: 0 })
        .unwrap();
    let mut c = CommandBuilder::new(exe);
    c.cwd(f.root.path());
    c.args(["--log-file", f.path("runtime.log").to_str().unwrap()]);
    let runtime = which::which("node").or_else(|_| which::which("bun")).unwrap();
    c.env("CLAUDE_RS_AGENT_BRIDGE_RUNTIME", runtime);
    c.env(
        "CLAUDE_RS_AGENT_BRIDGE",
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-bridge.js"),
    );
    for key in ["CLAUDE_RUST_NO_UPDATE_CHECK", "RUST_LOG", "CLAUDE_CODE_PROJECT_DIR_NAME"] {
        c.env_remove(key);
    }
    c.env("CLAUDE_RS_CONFIG_DIR", profile);
    c.env("CLAUDE_CONFIG_DIR", f.path("claude-profile"));
    c.env("HOME", f.path("home"));
    c.env("XDG_CONFIG_HOME", f.path("config"));
    c.env("XDG_DATA_HOME", f.path("data"));
    c.env("XDG_CACHE_HOME", f.path("cache"));
    c.env("TMPDIR", f.path("tmp"));
    c.env("PATH", tools);
    for (key, file) in [
        ("UPDATE_JOURNAL", "journal"),
        ("UPDATE_READY", "ready"),
        ("UPDATE_GATE", "release"),
        ("UPDATE_MARKER", "marker"),
    ] {
        c.env(key, f.path(file));
    }
    c.env("UPDATE_EXIT", code.to_string());
    c.env("FAKE_BRIDGE_JOURNAL", f.path("bridge-journal"));
    let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let mut reader = pair.master.try_clone_reader().unwrap();
    let child = pair.slave.spawn_command(c).unwrap();
    drop(pair.slave);
    let screen = Arc::new(Mutex::new(String::new()));
    let out = screen.clone();
    let reply = writer.clone();
    std::thread::spawn(move || {
        let mut parser = vt100::Parser::new(
            std::num::NonZeroU16::new(38).unwrap(),
            std::num::NonZeroU16::new(100).unwrap(),
            0,
        );
        let mut bytes = [0; 8192];
        let mut tail = Vec::new();
        while let Ok(n) = reader.read(&mut bytes) {
            if n == 0 {
                break;
            }
            parser.process(&bytes[..n]);
            *out.lock().unwrap() = parser.screen().contents();
            tail.extend_from_slice(&bytes[..n]);
            while let Some(i) = tail.windows(4).position(|b| b == b"\x1b[6n") {
                let _ = reply.lock().unwrap().write_all(b"\x1b[1;1R");
                tail.drain(..i + 4);
            }
            if tail.len() > 3 {
                tail.drain(..tail.len() - 3);
            }
        }
    });
    AppProcess { child, writer, screen, _master: pair.master }
}
fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
#[test]
fn normal_exit_detaches_the_real_worker_and_preserves_absolute_and_relative_profiles() {
    for relative in [false, true] {
        let mut f = Fixture::new();
        let tools = f.install_tools();
        for dir in ["chosen", "claude-profile", "tmp/chosen", "home", "config/claude-code-rust"] {
            std::fs::create_dir_all(f.path(dir)).unwrap();
        }
        let chosen = f.path("chosen/settings.json");
        let wrong = f.path("tmp/chosen/settings.json");
        let unrelated = f.path("config/claude-code-rust/settings.json");
        let sentinel = r#"{"sentinel":"unrelated profile"}"#;
        std::fs::write(&wrong, sentinel).unwrap();
        std::fs::write(&unrelated, sentinel).unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        std::fs::write(&chosen,json!({"updates":{"autoInstall":true,"future":42,"last_result":{"checked_at_unix_secs":now,"current_version":env!("CARGO_PKG_VERSION"),"latest_version":"99.0.0","release_url":"https://example.invalid/new","source":"github_release"}},"presentation":{"copyFullResponse":true}}).to_string()).unwrap();
        let project = f.root.path().canonicalize().unwrap();
        std::fs::write(
            f.path("claude-profile/.claude.json"),
            json!({"projects":{project.to_str().unwrap():{"hasTrustDialogAccepted":true}}})
                .to_string(),
        )
        .unwrap();
        let package = f.path("owned-package");
        std::fs::create_dir_all(package.join("bin")).unwrap();
        let exe = package.join("bin/claude-rs");
        std::fs::copy(env!("CARGO_BIN_EXE_claude-rs"), &exe).unwrap();
        std::fs::write(
            package.join("package.json"),
            r#"{"name":"@srothgan/claude-code-rust-linux-x64-gnu"}"#,
        )
        .unwrap();
        let profile = if relative { PathBuf::from("chosen") } else { f.path("chosen") };
        for code in [41, 0] {
            for name in ["release", "ready", "marker", "journal"] {
                let _ = std::fs::remove_file(f.path(name));
            }
            let mut app = launch(&f, &exe, &profile, &tools, code);
            app.wait_text("[READY]");
            if code == 0 {
                app.wait_text("Automatic update failed");
                app.wait_text("41");
            }
            app.exit();
            wait("real worker installer readiness", || f.path("ready").exists());
            f.collect_pids();
            assert!(!f.path("marker").exists(), "parent exited while installer is gated");
            let journal = std::fs::read_to_string(f.path("journal")).unwrap();
            let args: Vec<_> = journal.lines().filter_map(|l| l.strip_prefix("arg=")).collect();
            assert_eq!(args, ["install", "-g", "claude-code-rust@99.0.0"]);
            assert!(
                journal.contains(&format!("cwd={}", f.path("tmp").display())),
                "worker actually changes cwd: {journal}"
            );
            f.release();
            wait("installer outcome", || f.path("marker").exists());
            wait("selected-profile worker persistence", || {
                assert_eq!(
                    std::fs::read_to_string(&wrong).unwrap(),
                    sentinel,
                    "worker must not write the temp-relative profile; relative={relative}"
                );
                let v = read(&chosen);
                let error = v["updates"].get("last_install_error");
                if code == 0 {
                    error.is_none()
                } else {
                    error
                        .and_then(Value::as_str)
                        .is_some_and(|e| e.contains("41") && e.contains("v99.0.0"))
                }
            });
            let log = f.path("data/claude-code-rust/logs/claude-rs-update.log");
            wait("isolated worker log", || {
                std::fs::read_to_string(&log).is_ok_and(|s| s.contains("fixture-installer-output"))
            });
            for pid in &f.pids {
                wait("installer stopped", || fixture::stopped(*pid));
            }
            assert_eq!(std::fs::read_to_string(&wrong).unwrap(), sentinel);
            assert_eq!(std::fs::read_to_string(&unrelated).unwrap(), sentinel);
            let v = read(&chosen);
            assert_eq!(v["updates"]["autoInstall"], true);
            assert_eq!(v["updates"]["future"], 42);
            assert_eq!(v["presentation"]["copyFullResponse"], true);
        }
    }
}
