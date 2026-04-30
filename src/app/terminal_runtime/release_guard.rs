// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::modes::{apply_actions, release_to_child_actions, return_from_child_actions};
use crate::app::ReleaseReason;

pub(crate) struct TerminalReleaseGuard {
    reason: ReleaseReason,
    command: &'static str,
    restored: bool,
}

impl TerminalReleaseGuard {
    pub(crate) fn release(reason: ReleaseReason, command: &'static str) -> std::io::Result<Self> {
        let guard = Self { reason, command, restored: false };
        let mut stdout = std::io::stdout();
        if let Err(err) = apply_actions(&mut stdout, release_to_child_actions()) {
            drop(guard);
            return Err(err);
        }
        tracing::debug!(
            target: crate::logging::targets::APP_LIFECYCLE,
            event_name = "terminal_released_to_child",
            message = "terminal released to child process",
            outcome = "success",
            reason = ?reason,
            command,
        );
        Ok(guard)
    }

    pub(crate) fn restore(mut self) -> std::io::Result<()> {
        self.restore_inner()
    }

    fn restore_inner(&mut self) -> std::io::Result<()> {
        if self.restored {
            return Ok(());
        }

        let mut stdout = std::io::stdout();
        let result = apply_actions(&mut stdout, return_from_child_actions());
        match &result {
            Ok(()) => {
                tracing::debug!(
                    target: crate::logging::targets::APP_LIFECYCLE,
                    event_name = "terminal_returned_from_child",
                    message = "terminal restored from child process",
                    outcome = "success",
                    reason = ?self.reason,
                    command = self.command,
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: crate::logging::targets::APP_LIFECYCLE,
                    event_name = "terminal_return_from_child_failed",
                    message = "failed to restore terminal from child process",
                    outcome = "failure",
                    reason = ?self.reason,
                    command = self.command,
                    error_message = %err,
                );
            }
        }
        self.restored = result.is_ok();
        result
    }
}

impl Drop for TerminalReleaseGuard {
    fn drop(&mut self) {
        let _ = self.restore_inner();
    }
}
