// SPDX-License-Identifier: Apache-2.0

use std::cell::Cell;
use std::time::Instant;

#[derive(Clone, Copy)]
struct InputTimes {
    received_at: Instant,
    dispatched_at: Instant,
}

thread_local! {
    static EVENT_TIME: Cell<Option<InputTimes>> = const { Cell::new(None) };
}

/// Dispatch/timer time owns interaction activity and post-flush Enter suppression.
pub(super) fn now() -> Instant {
    EVENT_TIME.with(Cell::get).map_or_else(Instant::now, |time| time.dispatched_at)
}

/// Arrival time classifies character cadence independently of rendering delays.
pub(super) fn received_at() -> Instant {
    EVENT_TIME.with(Cell::get).map_or_else(now, |time| time.received_at)
}

/// Scope both measurements to synchronous dispatch; never hold across an await.
pub(super) struct EventTime {
    previous: Option<InputTimes>,
}

impl EventTime {
    pub(super) fn enter(received_at: Instant, dispatched_at: Instant) -> Self {
        let time = InputTimes { received_at, dispatched_at };
        Self { previous: EVENT_TIME.with(|current| current.replace(Some(time))) }
    }

    #[cfg(test)]
    pub(super) fn set(now: Instant) {
        EVENT_TIME.with(|time| time.set(Some(InputTimes { received_at: now, dispatched_at: now })));
    }
}

impl Drop for EventTime {
    fn drop(&mut self) {
        EVENT_TIME.with(|time| time.set(self.previous));
    }
}
