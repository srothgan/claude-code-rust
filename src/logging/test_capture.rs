// SPDX-License-Identifier: Apache-2.0

//! Diagnostics capture for unit tests.
//!
//! tracing caches callsite interest process-wide. A subscriber scoped to one test thread
//! therefore loses every event whose callsite another test thread registered first, which
//! makes captures depend on test scheduling. One process-wide subscriber with a per-thread
//! sink keeps each capture independent of the tests running beside it.

use std::cell::RefCell;
use std::io;
use std::sync::Once;

thread_local! {
    static SINK: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

struct ThreadSink;

impl io::Write for ThreadSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        SINK.with(|sink| {
            if let Some(bytes) = sink.borrow_mut().as_mut() {
                bytes.extend_from_slice(buf);
            }
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `action` and returns the detailed diagnostics records it emitted on this thread,
/// in the production format at the `full` preset.
pub(crate) fn capture(action: impl FnOnce()) -> Vec<serde_json::Value> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let filter =
            tracing_subscriber::EnvFilter::new(crate::DiagnosticsPreset::Full.filter_directives());
        tracing::subscriber::set_global_default(super::detailed_subscriber(filter, || ThreadSink))
            .expect("unit tests install no other global subscriber");
    });
    SINK.with(|sink| *sink.borrow_mut() = Some(Vec::new()));
    action();
    let bytes = SINK.with(|sink| sink.borrow_mut().take()).expect("capture sink");
    String::from_utf8(bytes)
        .expect("utf8 diagnostics")
        .lines()
        .map(|line| serde_json::from_str(line).expect("diagnostic JSON"))
        .collect()
}
