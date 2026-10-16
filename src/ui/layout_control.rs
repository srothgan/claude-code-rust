// SPDX-License-Identifier: Apache-2.0

use std::cell::Cell;
use std::time::{Duration, Instant};

pub(crate) const LAYOUT_TIME_SLICE: Duration = Duration::from_millis(4);

pub(crate) fn replay_row_budget(height: u16) -> usize {
    usize::from(height).saturating_mul(4).clamp(32, 160)
}

/// Cancellation belongs to a layout attempt, including work nested inside one message.
/// The caller supplies the observation mechanism; layout never owns terminal input.
pub(crate) struct LayoutControl<'a> {
    interrupted: Cell<bool>,
    check: Option<&'a dyn Fn() -> bool>,
    deadline: Option<Instant>,
}

impl<'a> LayoutControl<'a> {
    pub(crate) const fn uninterrupted() -> Self {
        Self { interrupted: Cell::new(false), check: None, deadline: None }
    }

    pub(crate) fn new(check: &'a dyn Fn() -> bool) -> Self {
        Self::with_budget(check, LAYOUT_TIME_SLICE)
    }

    pub(crate) fn with_budget(check: &'a dyn Fn() -> bool, budget: Duration) -> Self {
        Self {
            interrupted: Cell::new(false),
            check: Some(check),
            deadline: Some(Instant::now() + budget),
        }
    }

    pub(crate) fn cancel(&self) {
        self.interrupted.set(true);
    }

    pub(crate) fn can_yield(&self) -> bool {
        self.deadline.is_some()
    }

    pub(crate) fn yield_due(&self) -> bool {
        self.deadline.is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub(crate) fn cancelled(&self) -> bool {
        if !self.interrupted.get() && self.check.is_some_and(|check| check()) {
            self.interrupted.set(true);
        }
        self.interrupted.get()
    }
}
