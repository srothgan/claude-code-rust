// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use std::time::{Duration, Instant};

const FAILED_TTL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BtwRequestState {
    Waiting,
    Active,
    Failed { reason: String, expires_at: Instant },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BtwRequest {
    pub(crate) id: String,
    pub(crate) question: String,
    pub(crate) state: BtwRequestState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BtwCapacityError;

#[derive(Debug, Default)]
pub(crate) struct BtwRequests {
    items: Vec<BtwRequest>,
}

impl BtwRequests {
    pub(crate) const CAPACITY: usize = 10;
    pub(crate) const MAX_DETAIL_ROWS: usize = 3;
    const MAX_FAILED_ROWS: usize = 10;

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub(crate) fn is_full(&self) -> bool {
        self.items
            .iter()
            .filter(|item| !matches!(item.state, BtwRequestState::Failed { .. }))
            .count()
            >= Self::CAPACITY
    }

    #[must_use]
    pub(crate) fn has_active(&self) -> bool {
        self.items.iter().any(|item| matches!(item.state, BtwRequestState::Active))
    }

    pub(crate) fn try_push(
        &mut self,
        id: String,
        question: String,
    ) -> Result<(), BtwCapacityError> {
        if self.is_full() {
            return Err(BtwCapacityError);
        }
        self.items.push(BtwRequest { id, question, state: BtwRequestState::Waiting });
        Ok(())
    }

    #[must_use]
    pub(crate) fn get(&self, id: &str) -> Option<&BtwRequest> {
        self.items.iter().find(|item| item.id == id)
    }

    pub(crate) fn get_active(&self, id: &str) -> Option<&BtwRequest> {
        self.get(id).filter(|item| matches!(item.state, BtwRequestState::Active))
    }

    pub(crate) fn complete(&mut self, id: &str) -> Option<BtwRequest> {
        let index = self
            .items
            .iter()
            .position(|item| item.id == id && matches!(item.state, BtwRequestState::Active))?;
        Some(self.items.remove(index))
    }

    pub(crate) fn fail(&mut self, id: &str, reason: String, now: Instant) -> bool {
        let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.id == id && matches!(item.state, BtwRequestState::Active))
        else {
            return false;
        };
        item.state = BtwRequestState::Failed { reason, expires_at: now + FAILED_TTL };
        // Failure presentation is bounded independently of unresolved admission capacity.
        if self
            .items
            .iter()
            .filter(|item| matches!(item.state, BtwRequestState::Failed { .. }))
            .count()
            > Self::MAX_FAILED_ROWS
            && let Some(index) = self
                .items
                .iter()
                .position(|item| matches!(item.state, BtwRequestState::Failed { .. }))
        {
            self.items.remove(index);
        }
        true
    }

    pub(crate) fn expire_failed(&mut self, now: Instant) -> bool {
        let before = self.items.len();
        self.items.retain(|item| {
            !matches!(item.state, BtwRequestState::Failed { expires_at, .. } if now >= expires_at)
        });
        before != self.items.len()
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    pub(crate) fn status_items(&self) -> Vec<&BtwRequest> {
        let mut items = Vec::with_capacity(self.items.len());
        items
            .extend(self.items.iter().filter(|item| matches!(item.state, BtwRequestState::Active)));
        items.extend(
            self.items
                .iter()
                .rev()
                .filter(|item| matches!(item.state, BtwRequestState::Failed { .. })),
        );
        items.extend(
            self.items.iter().filter(|item| matches!(item.state, BtwRequestState::Waiting)),
        );
        items
    }

    pub(crate) fn take_next(&mut self) -> Option<BtwRequest> {
        if self.has_active() {
            return None;
        }
        let next =
            self.items.iter_mut().find(|item| matches!(item.state, BtwRequestState::Waiting))?;
        next.state = BtwRequestState::Active;
        Some(next.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{BtwRequestState, BtwRequests, FAILED_TTL};
    use std::time::Instant;

    #[test]
    fn duplicate_questions_keep_distinct_fifo_identity() {
        let mut requests = BtwRequests::default();
        requests.try_push("a".to_owned(), "same".to_owned()).expect("first");
        requests.try_push("b".to_owned(), "same".to_owned()).expect("second");
        assert_eq!(requests.take_next().expect("dispatch first").id, "a");
        assert!(requests.take_next().is_none());

        assert!(matches!(requests.get("a").map(|item| &item.state), Some(BtwRequestState::Active)));
        assert!(matches!(
            requests.get("b").map(|item| &item.state),
            Some(BtwRequestState::Waiting)
        ));

        let completed = requests.complete("a").expect("known request");
        assert_eq!(completed.question, "same");
        assert_eq!(requests.take_next().expect("dispatch second").id, "b");
        assert!(matches!(requests.get("b").map(|item| &item.state), Some(BtwRequestState::Active)));
    }

    #[test]
    fn failures_free_capacity_but_remain_visible_until_their_deadline() {
        let now = Instant::now();
        let mut requests = BtwRequests::default();
        requests.try_push("a".to_owned(), "question".to_owned()).expect("insert");
        requests.take_next().expect("dispatch");
        assert!(requests.fail("a", "failed".to_owned(), now));
        assert_eq!(requests.len(), 1);
        for index in 0..BtwRequests::CAPACITY {
            requests
                .try_push(format!("next-{index}"), "question".to_owned())
                .expect("failure does not occupy admission capacity");
        }
        assert!(requests.is_full());
        let before_deadline = FAILED_TTL
            .checked_sub(std::time::Duration::from_millis(1))
            .expect("failure TTL exceeds one millisecond");
        assert!(!requests.expire_failed(now + before_deadline));
        assert!(requests.expire_failed(now + FAILED_TTL));
        assert_eq!(requests.len(), BtwRequests::CAPACITY);
    }

    #[test]
    fn status_priority_is_active_then_newest_failures_then_waiting() {
        let now = Instant::now();
        let mut requests = BtwRequests::default();
        requests.try_push("old-failure".to_owned(), "old".to_owned()).expect("old failure");
        requests.try_push("new-failure".to_owned(), "new".to_owned()).expect("new failure");
        requests.try_push("active".to_owned(), "active".to_owned()).expect("active");
        requests.try_push("waiting".to_owned(), "waiting".to_owned()).expect("waiting");
        requests.take_next().expect("dispatch old failure");
        assert!(requests.fail("old-failure", "failed".to_owned(), now));
        requests.take_next().expect("dispatch new failure");
        assert!(requests.fail("new-failure", "failed".to_owned(), now));
        requests.take_next().expect("dispatch active");

        assert_eq!(
            requests.status_items().iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["active", "new-failure", "old-failure", "waiting"]
        );
    }

    #[test]
    fn repeated_failures_have_an_independent_bounded_display_lifetime() {
        let mut requests = BtwRequests::default();
        let now = Instant::now();
        for index in 0..30 {
            let id = format!("failure-{index}");
            requests.try_push(id.clone(), "question".to_owned()).expect("free admission");
            requests.take_next().expect("dispatch");
            assert!(requests.fail(&id, "failed".to_owned(), now));
        }
        assert_eq!(requests.len(), BtwRequests::MAX_FAILED_ROWS);
        assert!(!requests.is_full());
        assert!(requests.get("failure-19").is_none());
        assert!(requests.get("failure-20").is_some());
        assert!(requests.expire_failed(now + FAILED_TTL));
        assert_eq!(requests.len(), 0);
    }
}
