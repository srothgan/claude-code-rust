// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Simon Peter Rothgang

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
    pub text: String,
    pub images: Vec<ImageAttachment>,
    pub state: PendingUserMessageState,
}

impl PendingUserMessage {
    #[must_use]
    pub fn sending(uuid: String, text: String, images: Vec<ImageAttachment>) -> Self {
        Self { uuid, text, images, state: PendingUserMessageState::Sending }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn message(uuid: &str) -> PendingUserMessage {
        PendingUserMessage::sending(uuid.to_owned(), format!("message {uuid}"), Vec::new())
    }

    #[test]
    fn started_representative_drains_the_observed_coalesced_prefix() {
        let mut pending = PendingUserMessages::default();
        assert!(
            pending
                .try_push_sending(PendingUserMessage::sending(
                    "one".to_owned(),
                    "first line\ncontinued".to_owned(),
                    Vec::new(),
                ))
                .is_ok()
        );
        assert!(pending.try_push_sending(message("two")).is_ok());
        assert!(pending.try_push_sending(message("three")).is_ok());

        let started = pending.take_started_prefix("two");

        assert_eq!(
            started.iter().map(|item| item.uuid.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(started[0].text, "first line\ncontinued");
        assert_eq!(pending.iter().map(|item| item.uuid.as_str()).collect::<Vec<_>>(), ["three"]);
    }

    #[test]
    fn transitions_are_uuid_keyed_and_idempotent() {
        let mut pending = PendingUserMessages::default();
        assert!(pending.try_push_sending(message("one")).is_ok());
        assert!(matches!(
            pending.try_push_sending(message("one")),
            Err(PendingUserMessageInsertError::DuplicateUuid(message)) if message.uuid == "one"
        ));
        assert!(pending.mark_queued("one"));
        assert!(!pending.mark_queued("one"));
        assert!(!pending.mark_queued("unknown"));
        assert!(pending.take_started_prefix("unknown").is_empty());
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn capacity_rejection_returns_message_and_removing_an_item_reopens_a_slot() {
        let mut pending = PendingUserMessages::default();
        for index in 0..PendingUserMessages::CAPACITY {
            assert!(pending.try_push_sending(message(&index.to_string())).is_ok());
        }

        let overflow = match pending.try_push_sending(message("overflow")) {
            Err(PendingUserMessageInsertError::AtCapacity(message)) => message,
            other => panic!("expected capacity rejection, got {other:?}"),
        };
        assert_eq!(pending.len(), PendingUserMessages::CAPACITY);
        assert_eq!(overflow.uuid, "overflow");

        assert!(pending.remove("0").is_some());
        assert!(pending.try_push_sending(overflow).is_ok());
        assert_eq!(pending.len(), PendingUserMessages::CAPACITY);
    }
}
