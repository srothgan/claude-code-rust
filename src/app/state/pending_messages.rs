// SPDX-License-Identifier: Apache-2.0

use crate::app::clipboard_image::ImageAttachment;
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingUserMessageState {
    Sending,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUserMessage {
    pub uuid: String,
    pub(crate) timestamp: Option<crate::app::presentation::MessageTimestamp>,
    pub text: String,
    pub images: Vec<ImageAttachment>,
    pub state: PendingUserMessageState,
}

impl PendingUserMessage {
    #[must_use]
    pub fn sending(uuid: String, text: String, images: Vec<ImageAttachment>) -> Self {
        Self {
            timestamp: Some(crate::app::presentation::MessageTimestamp::observed()),
            uuid,
            text,
            images,
            state: PendingUserMessageState::Sending,
        }
    }

    #[must_use]
    pub fn first_line(&self) -> &str {
        self.text.lines().next().unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingUserMessageInsertError {
    AtCapacity(PendingUserMessage),
    DuplicateUuid(PendingUserMessage),
}

#[derive(Debug, Default)]
pub struct PendingUserMessages {
    items: VecDeque<PendingUserMessage>,
}

impl PendingUserMessages {
    pub(crate) fn set_sdk_timestamp(&mut self, uuid: &str, timestamp: &str) {
        if let Some(message) = self.items.iter_mut().find(|message| message.uuid == uuid) {
            crate::app::presentation::MessageTimestamp::apply_sdk(
                &mut message.timestamp,
                timestamp,
            );
        }
    }
    pub const CAPACITY: usize = 10;

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &PendingUserMessage> {
        self.items.iter()
    }

    pub fn try_push_sending(
        &mut self,
        message: PendingUserMessage,
    ) -> Result<(), PendingUserMessageInsertError> {
        if self.items.len() >= Self::CAPACITY {
            return Err(PendingUserMessageInsertError::AtCapacity(message));
        }
        if self.items.iter().any(|pending| pending.uuid == message.uuid) {
            return Err(PendingUserMessageInsertError::DuplicateUuid(message));
        }
        self.items.push_back(message);
        Ok(())
    }

    pub fn mark_queued(&mut self, uuid: &str) -> bool {
        let Some(message) = self.items.iter_mut().find(|pending| pending.uuid == uuid) else {
            return false;
        };
        if message.state == PendingUserMessageState::Queued {
            return false;
        }
        message.state = PendingUserMessageState::Queued;
        true
    }

    pub fn take_started_prefix(&mut self, representative_uuid: &str) -> Vec<PendingUserMessage> {
        let Some(last_idx) = self.items.iter().position(|item| item.uuid == representative_uuid)
        else {
            return Vec::new();
        };
        self.items.drain(..=last_idx).collect()
    }

    pub fn remove(&mut self, uuid: &str) -> Option<PendingUserMessage> {
        let idx = self.items.iter().position(|message| message.uuid == uuid)?;
        self.items.remove(idx)
    }

    pub fn drain(&mut self) -> Vec<PendingUserMessage> {
        self.items.drain(..).collect()
    }

    pub fn reconcile_interrupt_survivors(&mut self, still_queued: &[String]) -> usize {
        let mut matched = 0;
        for uuid in still_queued {
            if let Some(message) = self.items.iter_mut().find(|pending| pending.uuid == *uuid) {
                message.state = PendingUserMessageState::Queued;
                matched += 1;
            }
        }
        matched
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}
