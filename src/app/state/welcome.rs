// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

impl App {
    /// Ensure the synthetic welcome message exists at index 0.
    pub fn ensure_welcome_message(&mut self) {
        if self.transcript.messages.first().is_some_and(|m| matches!(m.role, MessageRole::Welcome))
        {
            return;
        }
        self.insert_message_tracked(0, self.build_welcome_message());
    }

    #[must_use]
    fn welcome_subscription_display(&self) -> &str {
        self.session_runtime
            .account_info
            .as_ref()
            .and_then(|account| account.subscription_type.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("-")
    }

    #[must_use]
    fn welcome_cwd_display(&self) -> &str {
        let cwd = self.cwd.trim();
        if cwd.is_empty() { "-" } else { cwd }
    }

    #[must_use]
    fn welcome_session_id_display(&self) -> String {
        self.session_runtime
            .session_id
            .as_ref()
            .map(std::string::ToString::to_string)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "-".to_owned())
    }

    #[must_use]
    pub(crate) fn build_welcome_message(&self) -> ChatMessage {
        let subscription = self.welcome_subscription_display();
        let session_id = self.welcome_session_id_display();
        ChatMessage::welcome(
            env!("CARGO_PKG_VERSION"),
            subscription,
            self.welcome_cwd_display(),
            &session_id,
        )
    }

    #[must_use]
    pub(crate) fn current_welcome_tip_seed(&self) -> Option<u64> {
        let first = self.transcript.messages.first()?;
        let MessageBlock::Welcome(welcome) = first.blocks.first()? else {
            return None;
        };
        Some(welcome.tip_seed)
    }

    pub(crate) fn apply_welcome_tip_seed(message: &mut ChatMessage, tip_seed: u64) {
        let Some(MessageBlock::Welcome(welcome)) = message.blocks.first_mut() else {
            return;
        };
        welcome.tip_seed = tip_seed;
    }

    /// Update the welcome message with the latest session/account snapshot.
    pub fn sync_welcome_snapshot(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        let subscription = self.welcome_subscription_display().to_owned();
        let cwd = self.welcome_cwd_display().to_owned();
        let session_id = self.welcome_session_id_display();
        let Some(first) = self.transcript.messages.first() else {
            return;
        };
        if !matches!(first.role, MessageRole::Welcome) {
            return;
        }
        let mut changed = false;
        {
            let Some(first) = self.transcript.messages.first_mut() else {
                return;
            };
            let Some(MessageBlock::Welcome(welcome)) = first.blocks.first_mut() else {
                return;
            };
            if welcome.version != version
                || welcome.subscription != subscription
                || welcome.cwd != cwd
                || welcome.session_id != session_id
            {
                version.clone_into(&mut welcome.version);
                welcome.subscription = subscription;
                welcome.cwd = cwd;
                welcome.session_id = session_id;
                welcome.cache.invalidate();
                changed = true;
            }
        }
        if changed {
            self.recompute_message_retained_bytes(0);
            self.invalidate_layout(InvalidationLevel::MessagesFrom(0));
        }
    }
}
