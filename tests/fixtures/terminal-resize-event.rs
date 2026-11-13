// SPDX-License-Identifier: Apache-2.0
// Deliver an OS resize event or native Windows keyboard records to an owned child.
// This executable has no application dependency or test protocol endpoint.
use std::io;
use std::path::PathBuf;

fn main() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let pid = args
        .next()
        .and_then(|value| value.to_str().and_then(|text| text.parse::<u32>().ok()))
        .filter(|pid| *pid != 0)
        .ok_or_else(|| io::Error::other("expected the owned application PID"))?;
    let report = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("expected delivery report path"))?;
    let keys = args.next().map(PathBuf::from);
    let details = deliver(pid, keys.as_deref())?;
    // AttachConsole may replace standard handles. Use a file so fixture
    // diagnostics can never become application terminal content or NDJSON.
    std::fs::write(report, details)
}

#[cfg(unix)]
fn deliver(pid: u32, keys: Option<&std::path::Path>) -> io::Result<String> {
    if keys.is_some() {
        return Err(io::Error::other("Unix keyboard tests use the PTY byte stream"));
    }
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
    // SIGWINCH is 28 on the supported Linux/macOS native PTY platforms. The
    // unchanged PTY winsize remains authoritative when crossterm reads it.
    // SAFETY: kill receives an owned live child PID and a constant signal;
    // it neither accesses Rust memory nor transfers any pointer ownership.
    if unsafe { kill(pid, 28) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(format!(
        r#"{{"injector_pid":{},"target_pid":{pid},"event":"SIGWINCH"}}"#,
        std::process::id()
    ))
}

#[cfg(windows)]
fn deliver(pid: u32, keys: Option<&std::path::Path>) -> io::Result<String> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Coord {
        x: i16,
        y: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }
    #[repr(C)]
    #[derive(Default)]
    struct BufferInfo {
        size: Coord,
        cursor: Coord,
        attributes: u16,
        window: SmallRect,
        maximum: Coord,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeyRecord {
        down: i32,
        repeat: u16,
        virtual_key: u16,
        scan: u16,
        unicode: u16,
        modifiers: u32,
    }
    #[repr(C)]
    union EventData {
        size: Coord,
        key: KeyRecord,
        storage: [u32; 4],
    }
    #[repr(C)]
    struct InputRecord {
        kind: u16,
        event: EventData,
    }
    const {
        assert!(std::mem::size_of::<InputRecord>() == 20);
    }
    const {
        assert!(std::mem::size_of::<BufferInfo>() == 22);
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FreeConsole() -> i32;
        fn AttachConsole(pid: u32) -> i32;
        fn GetConsoleScreenBufferInfo(handle: *mut c_void, info: *mut BufferInfo) -> i32;
        fn WriteConsoleInputW(
            handle: *mut c_void,
            events: *const InputRecord,
            count: u32,
            written: *mut u32,
        ) -> i32;
    }
    struct Attachment;
    impl Drop for Attachment {
        fn drop(&mut self) {
            // SAFETY: detaches only this fixture process, never the app.
            unsafe {
                FreeConsole();
            }
        }
    }
    // SAFETY: these parameterless/PID-only OS calls do not borrow Rust memory.
    // The supplied PID is the child held by the PTY test, not a discovered process.
    unsafe {
        FreeConsole();
    }
    if unsafe { AttachConsole(pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _attachment = Attachment;
    let input = std::fs::OpenOptions::new().read(true).write(true).open("CONIN$")?;
    let output = std::fs::OpenOptions::new().read(true).open("CONOUT$")?;
    let mut info = BufferInfo::default();
    // SAFETY: output is a live console handle; info has the documented C layout
    // and remains writable for the complete synchronous call.
    if unsafe { GetConsoleScreenBufferInfo(output.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let events = if let Some(path) = keys {
        // Encode physical keys rather than sending unsupported CSI sequences
        // through ConPTY. A logical CRLF line break is one native Return key.
        let source = std::fs::read_to_string(path)?;
        let mut remaining = source.as_str();
        let mut events = Vec::new();
        while !remaining.is_empty() {
            let specials: &[(&str, u16, u16, u32)] = &[
                ("\x1b[13;2u", 0x0d, 0x0d, 0x10),
                ("\x1b[D", 0x25, 0, 0),
                ("\x1b[C", 0x27, 0, 0),
                ("\x1b[A", 0x26, 0, 0),
                ("\x1b[B", 0x28, 0, 0),
                ("\x1b[H", 0x24, 0, 0),
                ("\x1b[F", 0x23, 0, 0),
                ("\x1b[3~", 0x2e, 0, 0),
                ("\x7f", 0x08, 0x08, 0),
                ("\r\n", 0x0d, 0x0d, 0),
                ("\r", 0x0d, 0x0d, 0),
                ("\n", 0x0d, 0x0d, 0),
            ];
            let mut append = |virtual_key, unicode, modifiers| {
                events.push(InputRecord {
                    kind: 0x0001,
                    event: EventData {
                        key: KeyRecord {
                            down: 1,
                            repeat: 1,
                            virtual_key,
                            scan: 0,
                            unicode,
                            modifiers,
                        },
                    },
                })
            };
            if let Some((prefix, key, unicode, modifiers)) =
                specials.iter().find(|(prefix, ..)| remaining.starts_with(prefix))
            {
                append(*key, *unicode, *modifiers);
                remaining = &remaining[prefix.len()..];
            } else {
                let ch = remaining.chars().next().ok_or_else(|| io::Error::other("missing key"))?;
                if ch.is_control() {
                    return Err(io::Error::other("unsupported fixture key encoding"));
                }
                for unit in ch.encode_utf16(&mut [0; 2]) {
                    append(0xe7, *unit, 0);
                }
                remaining = &remaining[ch.len_utf8()..];
            }
        }
        events
    } else {
        let mut event = InputRecord { kind: 0x0004, event: EventData { storage: [0; 4] } };
        event.event.size = info.size;
        vec![event]
    };
    let count = u32::try_from(events.len()).map_err(io::Error::other)?;
    let mut written = 0;
    // SAFETY: the initialized INPUT_RECORD has the Windows union size/alignment,
    // the matching event tag and actual console dimensions or explicit keys.
    // Both pointers remain valid throughout the synchronous write.
    if unsafe { WriteConsoleInputW(input.as_raw_handle(), events.as_ptr(), count, &mut written) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if written != count {
        return Err(io::Error::other("native input records were not enqueued"));
    }
    Ok(format!(
        r#"{{"injector_pid":{},"target_pid":{pid},"event":"{}","written":{written},"buffer_width":{},"buffer_height":{},"visible_width":{},"visible_height":{}}}"#,
        std::process::id(),
        if keys.is_some() { "KEY_EVENT" } else { "WINDOW_BUFFER_SIZE_EVENT" },
        info.size.x,
        info.size.y,
        info.window.right - info.window.left + 1,
        info.window.bottom - info.window.top + 1
    ))
}
