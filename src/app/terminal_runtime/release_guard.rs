// SPDX-License-Identifier: Apache-2.0

use super::modes::{TerminalModes, release_to_child_actions, return_from_child_actions};
use crate::app::ReleaseReason;

pub(crate) struct TerminalReleaseGuard {
    reason: ReleaseReason,
    command: &'static str,
    restored: bool,
    modes: TerminalModes,
}

impl TerminalReleaseGuard {
    pub(crate) fn release(reason: ReleaseReason, command: &'static str) -> std::io::Result<Self> {
        let guard = Self {
            reason,
            command,
            restored: false,
            modes: TerminalModes::new(super::chat_terminal::NativeTerminalIo),
        };
        if let Err(err) = guard.modes.apply(release_to_child_actions()) {
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

        let result = self.modes.apply(return_from_child_actions());
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
