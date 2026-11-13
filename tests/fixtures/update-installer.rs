// SPDX-License-Identifier: Apache-2.0
// Harmless executable used only by update audit fixtures. Never installs software.
use std::{
    env, fs,
    path::PathBuf,
    process::{Command, exit},
    thread,
    time::{Duration, Instant},
};
fn path(name: &str) -> PathBuf {
    PathBuf::from(env::var_os(name).unwrap())
}
fn wait_gate() {
    if let Some(p) = env::var_os("UPDATE_GATE") {
        let deadline = Instant::now() + Duration::from_secs(45);
        while !PathBuf::from(&p).exists() {
            if Instant::now() > deadline {
                exit(91)
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
#[cfg(windows)]
fn alive(pid: u32) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, ms: u32) -> u32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(0x00100000, 0, pid);
        if h.is_null() {
            return false;
        }
        let running = WaitForSingleObject(h, 0) == 258;
        CloseHandle(h);
        running
    }
}
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat.rsplit_once(") ").is_some_and(|(_, s)| s.starts_with('Z')) {
            return false;
        }
    }
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 }
}
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "probe") {
        exit(i32::from(!alive(args[1].parse().unwrap())));
    }
    if args.first().is_some_and(|a| a == "kill") {
        let pid: u32 = args[1].parse().unwrap();
        if !alive(pid) {
            return;
        }
        #[cfg(windows)]
        {
            let _ =
                Command::new("taskkill.exe").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
        }
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            unsafe {
                kill(pid as i32, 9);
            }
        }
        return;
    }
    if args.first().is_some_and(|a| a == "tree-child" || a == "detached") {
        fs::write(path("UPDATE_READY"), std::process::id().to_string()).unwrap();
        wait_gate();
        fs::write(path("UPDATE_MARKER"), "completed").unwrap();
        return;
    }
    if args.first().is_some_and(|a| a == "tree-parent") {
        let child_ready = path("UPDATE_CHILD_READY");
        let mut child = Command::new(env::current_exe().unwrap())
            .arg("tree-child")
            .env("UPDATE_READY", &child_ready)
            .spawn()
            .unwrap();
        fs::write(path("UPDATE_READY"), format!("{} {}", std::process::id(), child.id())).unwrap();
        wait_gate();
        let _ = child.wait();
        return;
    }
    let exe = env::current_exe().unwrap();
    let name = exe.file_stem().unwrap().to_string_lossy();
    let mut journal = format!(
        "program={name}\npid={}\ncwd={}\n",
        std::process::id(),
        env::current_dir().unwrap().display()
    );
    for arg in &args {
        journal.push_str(&format!("arg={arg}\n"));
    }
    for (k, v) in env::vars().filter(|(k, _)| k.starts_with("CLAUDE_RS_")) {
        journal.push_str(&format!("{k}={v}\n"));
    }
    fs::write(path("UPDATE_JOURNAL"), &journal).unwrap();
    if name == "curl" || name == "wget" {
        fs::write(path("UPDATE_DOWNLOAD_JOURNAL"), &journal).unwrap();
        let index = args.iter().position(|a| a == "-o" || a == "-O").unwrap() + 1;
        let script = PathBuf::from(&args[index]);
        fs::write(path("UPDATE_DOWNLOAD_PATH"), script.display().to_string()).unwrap();
        let mode = env::var("UPDATE_DOWNLOAD_MODE").unwrap_or_default();
        if mode == "fail" {
            exit(43)
        }
        if mode != "missing" {
            let helper = path("UPDATE_EXECUTABLE");
            let quoted = helper.to_string_lossy().replace(char::from(39), "'\"'\"'");
            fs::write(script, format!("#!/bin/sh\nexec '{quoted}' \"$@\"\n")).unwrap();
        }
        return;
    }
    fs::write(path("UPDATE_READY"), std::process::id().to_string()).unwrap();
    println!("fixture-installer-output");
    wait_gate();
    let code = env::var("UPDATE_EXIT").unwrap_or_else(|_| "0".into()).parse::<i32>().unwrap();
    fs::write(path("UPDATE_MARKER"), format!("exit={code}")).unwrap();
    exit(code);
}
