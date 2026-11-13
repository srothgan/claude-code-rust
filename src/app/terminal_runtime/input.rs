// SPDX-License-Identifier: Apache-2.0

use crossterm::event::Event;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// One stoppable terminal reader. Joining on release guarantees that no
/// background poll can consume inherited stdin after a child starts.
pub(crate) struct TerminalInput {
    receiver: mpsc::Receiver<std::io::Result<CapturedTerminalEvent>>,
    stop: Arc<AtomicBool>,
    reading_batch: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

pub(crate) struct CapturedTerminalEvent {
    pub(crate) event: Event,
    pub(crate) received_at: Instant,
}

impl TerminalInput {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel(256);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let reading_batch = Arc::new(AtomicBool::new(false));
        let worker_reading = Arc::clone(&reading_batch);
        let worker = std::thread::spawn(move || {
            let mut batch_received_at = None;
            while !worker_stop.load(Ordering::Acquire) {
                // Poll wakes immediately on input. The timeout only bounds
                // release latency when there is no input to wake the reader.
                // Preserve arrival cadence while the bounded send waits for
                // rendering. Already-ready console records belong to this read
                // batch; sampling each read would measure queue backpressure.
                let timeout = if batch_received_at.is_some() {
                    Duration::ZERO
                } else {
                    Duration::from_millis(25)
                };
                let ready = crossterm::event::poll(timeout);
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                let event = match ready {
                    Ok(false) => {
                        batch_received_at = None;
                        worker_reading.store(false, Ordering::Release);
                        continue;
                    }
                    Ok(true) => {
                        worker_reading.store(true, Ordering::Release);
                        crossterm::event::read()
                    }
                    Err(err) => Err(err),
                };
                let event = event.map(|event| CapturedTerminalEvent {
                    event,
                    received_at: *batch_received_at.get_or_insert_with(Instant::now),
                });
                let failed = event.is_err();
                if sender.blocking_send(event).is_err() || failed {
                    break;
                }
            }
            worker_reading.store(false, Ordering::Release);
        });
        Self { receiver, stop, reading_batch, worker: Some(worker) }
    }

    /// A bounded UI drain is not an input-idle boundary. The reader can be
    /// waiting for queue space while more native records are already ready.
    pub(crate) fn has_pending_input(&self) -> bool {
        self.reading_batch.load(Ordering::Acquire) || !self.receiver.is_empty()
    }

    pub(crate) async fn next(&mut self) -> Option<std::io::Result<CapturedTerminalEvent>> {
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
