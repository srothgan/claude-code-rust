// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::time::Duration;

#[test]
fn expiry_removes_only_the_due_failure_and_preserves_active_and_waiting_payloads() {
    let base = Instant::now();
    let mut queue = BtwRequests::default();
    for (id, question, reason, at) in [
        ("old", "OLD_QUESTION", "OLD_CAUSE", base),
        ("new", "NEW_QUESTION", "NEW_CAUSE", base + Duration::from_secs(1)),
    ] {
        queue.try_push(id.into(), question.into()).unwrap();
        assert_eq!(queue.take_next().unwrap().id, id);
        assert!(queue.fail(id, reason.into(), at));
    }
    queue.try_push("active".into(), "ACTIVE_PAYLOAD".into()).unwrap();
    queue.try_push("waiter".into(), "WAITING_PAYLOAD".into()).unwrap();
    assert_eq!(queue.take_next().unwrap().id, "active");
    assert!(!queue.expire_failed(base + FAILED_TTL.checked_sub(Duration::from_nanos(1)).unwrap()));
    let old = queue.get("old").unwrap();
    assert_eq!(old.question, "OLD_QUESTION");
    assert!(matches!(&old.state, BtwRequestState::Failed { reason, .. } if reason == "OLD_CAUSE"));
    assert!(queue.expire_failed(base + FAILED_TTL));
    assert!(queue.get("old").is_none());
    let newer = queue.get("new").unwrap();
    assert_eq!(newer.question, "NEW_QUESTION");
    assert!(matches!(&newer.state,BtwRequestState::Failed{reason,..} if reason=="NEW_CAUSE"));
    assert_eq!(
        queue.status_items().iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
        ["active", "new", "waiter"]
    );
    assert!(queue.expire_failed(base + FAILED_TTL + Duration::from_secs(1)));
    assert!(!queue.expire_failed(base + FAILED_TTL + Duration::from_secs(2)));
    assert_eq!(queue.complete("active").unwrap().question, "ACTIVE_PAYLOAD");
    assert_eq!(queue.take_next().unwrap().question, "WAITING_PAYLOAD");
    assert!(queue.take_next().is_none());
    assert_eq!(queue.complete("waiter").unwrap().id, "waiter");
    assert!(queue.status_items().is_empty());
}

#[test]
fn failure_retention_evicts_the_oldest_without_losing_waiters_or_future_capacity() {
    let mut queue = BtwRequests::default();
    let now = Instant::now();
    let fixtures: Vec<_> = (0..=BtwRequests::MAX_FAILED_ROWS)
        .map(|i| (format!("failure-{i}"), format!("question-{i}"), format!("cause-{i}")))
        .collect();
    for (id, question, cause) in &fixtures {
        queue.try_push(id.clone(), question.clone()).unwrap();
        assert_eq!(queue.take_next().unwrap().id, *id);
        if id == &fixtures.last().unwrap().0 {
            queue.try_push("waiter".into(), "KEPT_WAITER".into()).unwrap();
        }
        assert!(queue.fail(id, cause.clone(), now));
    }
    let failures: Vec<_> = queue
        .status_items()
        .into_iter()
        .filter_map(|item| match &item.state {
            BtwRequestState::Failed { reason, .. } => {
                Some((item.id.clone(), item.question.clone(), reason.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(failures, fixtures[1..].iter().rev().cloned().collect::<Vec<_>>());
    assert!(queue.get(&fixtures[0].0).is_none());
    assert_eq!(queue.take_next().unwrap().question, "KEPT_WAITER");
    queue.try_push("fresh".into(), "FRESH_PAYLOAD".into()).unwrap();
    assert!(queue.take_next().is_none());
    assert_eq!(queue.complete("waiter").unwrap().question, "KEPT_WAITER");
    assert_eq!(queue.take_next().unwrap().question, "FRESH_PAYLOAD");
    assert_eq!(queue.complete("fresh").unwrap().id, "fresh");
}
