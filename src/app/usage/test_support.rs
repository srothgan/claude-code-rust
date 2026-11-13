// SPDX-License-Identifier: Apache-2.0
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) struct Server {
    pub endpoint: String,
    pub request: tokio::sync::oneshot::Receiver<String>,
    stop: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<bool>>,
}
impl Server {
    pub fn new(status: u16, body: &str, mode: &str, journal: Option<PathBuf>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/api/oauth/usage", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&stop);
        let release = Arc::new(AtomicBool::new(false));
        let released = Arc::clone(&release);
        let body = body.as_bytes().to_vec();
        let mode = mode.to_owned();
        let (tx, request) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(12);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("{e}"),
                }
                if cancelled.load(Ordering::SeqCst) || Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(2));
            };
            // Windows inherits the listener's nonblocking mode on accepted sockets.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0];
            while !bytes.ends_with(b"\r\n\r\n") {
                if stream.read(&mut byte).unwrap() == 0 {
                    return false;
                }
                bytes.push(byte[0]);
            }
            let request = String::from_utf8(bytes).unwrap();
            if let Some(path) = journal {
                writeln!(
                    fs::OpenOptions::new().create(true).append(true).open(path).unwrap(),
                    "oauth"
                )
                .unwrap();
            }
            let _ = tx.send(request);
            while mode == "gated" && !released.load(Ordering::SeqCst) {
                if cancelled.load(Ordering::SeqCst) || Instant::now() > deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            if mode == "stall" {
                return stream.read(&mut byte).unwrap() == 0;
            }
            if mode == "disconnect" {
                return false;
            }
            let length = if mode == "short-body" { body.len() + 100 } else { body.len() };
            write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n").unwrap();
            stream.write_all(&body).unwrap();
            true
        });
        Self { endpoint, request, stop, release, worker: Some(worker) }
    }
    pub fn release(&self) {
        self.release.store(true, Ordering::SeqCst);
    }
    pub async fn finish(&mut self) -> bool {
        self.stop.store(true, Ordering::SeqCst);
        let worker = self.worker.take().unwrap();
        tokio::task::spawn_blocking(move || worker.join().unwrap()).await.unwrap()
    }
    pub fn probe(&self) -> oauth::Probe {
        oauth::Probe {
            credentials: Some(crate::app::auth::ClaudeOAuthCredentials {
                access_token: "synthetic-usage-token".into(),
                expires_at: None,
            }),
            endpoint: self.endpoint.clone(),
            timeout: Duration::from_secs(5),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn native_executable() -> PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    if let Some(path) = std::env::var_os("CLAUDE_RS_USAGE_FIXTURE") {
        return path.into();
    }
    BINARY
        .get_or_init(|| {
            let root = tempfile::tempdir().unwrap().keep();
            let path = root.join(format!("claude{}", std::env::consts::EXE_SUFFIX));
            let output = std::process::Command::new("rustc")
                .args(["--edition=2024"])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/usage-probe.rs"))
                .arg("-o")
                .arg(&path)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            path
        })
        .clone()
}

pub(super) struct CliFixture {
    pub root: tempfile::TempDir,
    pub executable: PathBuf,
    pub project: PathBuf,
}
impl CliFixture {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let executable = bin.join(format!("claude{}", std::env::consts::EXE_SUFFIX));
        fs::copy(native_executable(), &executable).unwrap();
        let project = root.path().join("selected project");
        fs::create_dir(&project).unwrap();
        let fixture = Self { root, executable, project };
        fixture.configure("ok", "Current session\n23% used\nResets fixture hour", "");
        fixture
    }
    pub fn configure(&self, mode: &str, stdout: &str, stderr: &str) {
        for (name, value) in [("mode", mode), ("stdout", stdout), ("stderr", stderr)] {
            fs::write(self.project.join(name), value).unwrap();
        }
    }
    pub fn probe(&self) -> cli::Probe {
        cli::Probe { executable: Some(self.executable.clone()), timeout: Duration::from_secs(10) }
    }
    pub async fn pid(&self) -> u32 {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(text) = fs::read_to_string(self.project.join("pid")) {
                    return text.parse().unwrap();
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap()
    }
    pub fn stopped(&self, pid: u32) -> bool {
        let output = std::process::Command::new(&self.executable)
            .args(["--probe", &pid.to_string()])
            .output()
            .unwrap();
        assert!(matches!(output.status.code(), Some(0 | 1)));
        output.status.code() == Some(1)
    }
    pub fn assert_invocation(&self) {
        assert_eq!(fs::read(self.project.join("argv")).unwrap(), b"/usage\0--allowed-tools\0\0");
        assert_eq!(
            PathBuf::from(fs::read_to_string(self.project.join("cwd")).unwrap()),
            self.project
        );
    }
}
impl Drop for CliFixture {
    fn drop(&mut self) {
        // Failure cleanup is separate from the test's pre-drop termination assertion.
        let _ = fs::write(self.project.join("release"), b"");
        if let Ok(text) = fs::read_to_string(self.project.join("pid"))
            && let Ok(pid) = text.parse()
        {
            let deadline = Instant::now() + Duration::from_secs(3);
            while !self.stopped(pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

pub(super) struct ProbeGuard(Option<HostProbe>);
impl ProbeGuard {
    pub fn install(probe: HostProbe) -> Self {
        Self(TEST_PROBE.with(|slot| slot.replace(Some(probe))))
    }
}
impl Drop for ProbeGuard {
    fn drop(&mut self) {
        TEST_PROBE.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}
