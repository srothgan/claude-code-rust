// SPDX-License-Identifier: Apache-2.0

use crossterm::event::Event;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::mpsc;

/// One stoppable terminal reader. Joining on release guarantees that no
/// background poll can consume inherited stdin after a child starts.
pub(crate) struct TerminalInput {
    receiver: mpsc::Receiver<std::io::Result<Event>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl TerminalInput {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel(256);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                // Poll wakes immediately on input. The timeout only bounds
                // release latency when there is no input to wake the reader.
                let ready = crossterm::event::poll(Duration::from_millis(25));
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                let event = match ready {
                    Ok(false) => continue,
                    Ok(true) => crossterm::event::read(),
                    Err(err) => Err(err),
                };
                let failed = event.is_err();
                if sender.blocking_send(event).is_err() || failed {
                    break;
                }
            }
        });
        Self { receiver, stop, worker: Some(worker) }
    }

    pub(crate) async fn next(&mut self) -> Option<std::io::Result<Event>> {
        self.receiver.recv().await
    }
}

impl Drop for TerminalInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Unblock a sender if queued input filled the bounded channel.
        self.receiver.close();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!(
                target: crate::logging::targets::APP_LIFECYCLE,
                event_name = "terminal_input_worker_failed",
                message = "terminal input reader panicked during release",
                outcome = "failure",
            );
        }
    }
}
