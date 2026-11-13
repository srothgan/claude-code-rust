// SPDX-License-Identifier: Apache-2.0
// Shared native-process fixtures; all environment changes belong to child commands.
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    sync::OnceLock,
    time::{Duration, Instant},
};

pub fn executable() -> &'static Path {
    static FIXTURE: OnceLock<PathBuf> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            let dir = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!("update-fixture-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let output = Command::new("rustc")
                .arg("--edition=2024")
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/fixtures/update-installer.rs"),
                )
                .arg("-o")
                .arg(dir.join(format!("fixture{}", std::env::consts::EXE_SUFFIX)))
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            dir
        })
        .as_path()
}
pub fn binary() -> PathBuf {
    executable().join(format!("fixture{}", std::env::consts::EXE_SUFFIX))
}
pub fn wait(description: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {description}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
pub struct OwnedChild(pub Child);
impl OwnedChild {
    pub fn finish(&mut self) -> ExitStatus {
        let mut result = None;
        wait("owned child exit", || {
            result = self.0.try_wait().unwrap();
            result.is_some()
        });
        result.unwrap()
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
pub struct Fixture {
    pub root: tempfile::TempDir,
    pub pids: Vec<u32>,
}
impl Fixture {
    pub fn new() -> Self {
        Self { root: tempfile::tempdir().unwrap(), pids: Vec::new() }
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = Command::new(program);
        self.environment(&mut c);
        c
    }
    pub fn environment(&self, c: &mut Command) {
        for (key, file) in [
            ("UPDATE_JOURNAL", "journal"),
            ("UPDATE_READY", "ready"),
            ("UPDATE_CHILD_READY", "child-ready"),
            ("UPDATE_GATE", "release"),
            ("UPDATE_MARKER", "marker"),
            ("UPDATE_DOWNLOAD_JOURNAL", "download-journal"),
            ("UPDATE_DOWNLOAD_PATH", "download-path"),
        ] {
            c.env(key, self.path(file));
        }
        c.env("UPDATE_EXECUTABLE", binary());
    }
    pub fn install_tools(&self) -> PathBuf {
        let bin = self.path("tools");
        std::fs::create_dir(&bin).unwrap();
        #[cfg(windows)]
        let names = ["npm.exe", "powershell.exe"];
        #[cfg(unix)]
        let names = ["npm", "curl"];
        for name in names {
            std::fs::copy(binary(), bin.join(name)).unwrap();
        }
        #[cfg(windows)]
        std::fs::write(
            bin.join("npm.cmd"),
            "@echo off\r\n\"%~dp0npm.exe\" %*\r\nexit /b %errorlevel%\r\n",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/bin/sh", bin.join("sh")).unwrap();
        bin
    }
    pub fn collect_pids(&mut self) {
        if let Ok(text) = std::fs::read_to_string(self.path("ready")) {
            for pid in text.split_whitespace().filter_map(|s| s.parse().ok()) {
                if !self.pids.contains(&pid) {
                    self.pids.push(pid);
                }
            }
        }
    }
    pub fn release(&self) {
        std::fs::write(self.path("release"), "").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.collect_pids();
        let _ = std::fs::write(self.path("release"), "");
        for pid in &self.pids {
            let _ = Command::new(binary()).args(["kill", &pid.to_string()]).status();
        }
    }
}
pub fn stopped(pid: u32) -> bool {
    Command::new(binary()).args(["probe", &pid.to_string()]).status().unwrap().code() == Some(1)
}
