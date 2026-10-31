// SPDX-License-Identifier: Apache-2.0
// Direct native child for usage tests. It never reads account data or starts another process.
use std::{
    fs,
    io::Write,
    time::{Duration, Instant},
};

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // Signal zero observes this test's recorded PID without changing it. Zombies count as live.
    unsafe {
        kill(i32::try_from(pid).unwrap(), 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(3)
    }
}
#[cfg(windows)]
fn alive(pid: u32) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, ms: u32) -> u32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn GetLastError() -> u32;
    }
    // A synchronization-only handle observes the exact fixture child; every acquired handle closes.
    unsafe {
        let handle = OpenProcess(0x00100000, 0, pid);
        if handle.is_null() {
            return GetLastError() != 87;
        }
        let result = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        result != 0
    }
}
fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("--probe") {
        std::process::exit(i32::from(!alive(args[1].parse().unwrap())));
    }
    assert_eq!(args, ["/usage", "--allowed-tools", ""]);
    let cwd = std::env::current_dir().unwrap();
    fs::write(cwd.join("argv"), b"/usage\0--allowed-tools\0\0").unwrap();
    fs::write(cwd.join("cwd"), cwd.to_str().unwrap()).unwrap();
    let mut journal =
        fs::OpenOptions::new().create(true).append(true).open(cwd.join("attempts")).unwrap();
    writeln!(journal, "cli").unwrap();
    fs::write(cwd.join("pid.tmp"), std::process::id().to_string()).unwrap();
    fs::rename(cwd.join("pid.tmp"), cwd.join("pid")).unwrap();
    let mode = fs::read_to_string(cwd.join("mode")).unwrap();
    if mode == "hang" {
        let deadline = Instant::now() + Duration::from_secs(45);
        while cwd.exists() && !cwd.join("release").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    std::io::stdout().write_all(&fs::read(cwd.join("stdout")).unwrap()).unwrap();
    std::io::stderr().write_all(&fs::read(cwd.join("stderr")).unwrap()).unwrap();
    if mode == "fail" {
        std::process::exit(7);
    }
}
