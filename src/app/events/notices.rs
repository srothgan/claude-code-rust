// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::super::{
    App, ChatMessage, InvalidationLevel, MessageBlock, MessageRole, NoticeBlock, NoticeDedupKey,
    NoticeStage, SystemSeverity, TurnNoticeLocation, TurnNoticeRef,
};

#[derive(Clone)]
struct TurnNoticeTracking {
    dedup_key: NoticeDedupKey,
    stage: NoticeStage,
}

pub(super) fn clear_turn_notice_tracking(app: &mut App) {
    app.clear_turn_notice_refs();
}

pub(super) fn emit_system_notice(app: &mut App, severity: SystemSeverity, message: &str) {
    insert_notice(app, severity, message, None);
}

pub(super) fn upsert_turn_notice(
    app: &mut App,
    dedup_key: NoticeDedupKey,
    stage: NoticeStage,
    severity: SystemSeverity,
    message: &str,
) {
    prune_invalid_turn_notice_refs(app);
    let Some(existing_ref_idx) =
        app.turn_notice_refs.iter().position(|notice_ref| notice_ref.dedup_key == dedup_key)
    else {
        insert_new_notice(app, dedup_key, stage, severity, message);
        return;
    };

    let existing = app.turn_notice_refs[existing_ref_idx].clone();
    if stage < existing.stage {
        return;
    }

    match existing.location {
        TurnNoticeLocation::Inline { msg_idx, block_idx } => {
            if update_inline_notice(app, msg_idx, block_idx, &dedup_key, severity, message) {
                app.turn_notice_refs[existing_ref_idx].stage = stage;
                return;
            }
            app.turn_notice_refs.remove(existing_ref_idx);
            insert_new_notice(app, dedup_key, stage, severity, message);
        }
        TurnNoticeLocation::Standalone { msg_idx } => {
            if app.active_turn_assistant_idx().is_some()
                && remove_standalone_notice(app, msg_idx)
                && let Some(owner_idx) = app.active_turn_assistant_idx()
            {
                app.turn_notice_refs.remove(existing_ref_idx);
                insert_inline_notice(
                    app,
                    owner_idx,
                    severity,
                    message,
                    Some(TurnNoticeTracking { dedup_key, stage }),
                );
                return;
            }

            if update_standalone_notice(app, msg_idx, &dedup_key, severity, message) {
                app.turn_notice_refs[existing_ref_idx].stage = stage;
                return;
            }

            app.turn_notice_refs.remove(existing_ref_idx);
            insert_new_notice(app, dedup_key, stage, severity, message);
        }
    }
}

fn insert_new_notice(
    app: &mut App,
    dedup_key: NoticeDedupKey,
    stage: NoticeStage,
    severity: SystemSeverity,
    message: &str,
) {
    insert_notice(app, severity, message, Some(TurnNoticeTracking { dedup_key, stage }));
}

fn insert_notice(
    app: &mut App,
    severity: SystemSeverity,
    message: &str,
    tracking: Option<TurnNoticeTracking>,
) {
    if let Some(owner_idx) = app.active_turn_assistant_idx() {
        insert_inline_notice(app, owner_idx, severity, message, tracking);
    } else {
        insert_standalone_notice(app, severity, message, tracking);
    }
}

fn insert_inline_notice(
    app: &mut App,
    owner_idx: usize,
    severity: SystemSeverity,
    message: &str,
    tracking: Option<TurnNoticeTracking>,
) {
    let Some(owner) = app.messages.get_mut(owner_idx) else {
        insert_standalone_notice(app, severity, message, tracking);
        return;
    };
    let block_idx = owner.blocks.len();
    let dedup_key = tracking.as_ref().map(|entry| entry.dedup_key.clone());
    owner.blocks.push(MessageBlock::Notice(notice_block(severity, message, dedup_key)));
    app.sync_after_message_blocks_changed(owner_idx);
    app.invalidate_layout(InvalidationLevel::MessageChanged(owner_idx));
    if let Some(tracking) = tracking {
        app.turn_notice_refs.push(TurnNoticeRef {
            dedup_key: tracking.dedup_key,
            stage: tracking.stage,
            location: TurnNoticeLocation::Inline { msg_idx: owner_idx, block_idx },
        });
    }
}

fn insert_standalone_notice(
    app: &mut App,
    severity: SystemSeverity,
    message: &str,
    tracking: Option<TurnNoticeTracking>,
) {
    let msg_idx = app.messages.len();
    let dedup_key = tracking.as_ref().map(|entry| entry.dedup_key.clone());
    app.push_message_tracked(ChatMessage::new(
        MessageRole::System(Some(severity)),
        vec![MessageBlock::Notice(notice_block(severity, message, dedup_key))],
        None,
    ));
    app.enforce_history_retention_tracked();
    if let Some(tracking) = tracking {
        app.turn_notice_refs.push(TurnNoticeRef {
            dedup_key: tracking.dedup_key,
            stage: tracking.stage,
            location: TurnNoticeLocation::Standalone { msg_idx },
        });
    }
}

fn notice_block(
    severity: SystemSeverity,
    message: &str,
    dedup_key: Option<NoticeDedupKey>,
) -> NoticeBlock {
    let block = NoticeBlock::from_complete(severity, message);
    if let Some(dedup_key) = dedup_key { block.with_dedup_key(dedup_key) } else { block }
}

fn update_inline_notice(
    app: &mut App,
    msg_idx: usize,
    block_idx: usize,
    dedup_key: &NoticeDedupKey,
    severity: SystemSeverity,
    message: &str,
) -> bool {
    let Some(MessageBlock::Notice(notice)) =
        app.messages.get_mut(msg_idx).and_then(|msg| msg.blocks.get_mut(block_idx))
    else {
        return false;
    };
    if notice.dedup_key.as_ref() != Some(dedup_key) {
        return false;
    }
    notice.severity = severity;
    notice.replace_text(message);
    app.sync_render_cache_slot(msg_idx, block_idx);
    app.recompute_message_retained_bytes(msg_idx);
    app.invalidate_layout(InvalidationLevel::MessageChanged(msg_idx));
    true
}

fn update_standalone_notice(
    app: &mut App,
    msg_idx: usize,
    dedup_key: &NoticeDedupKey,
    severity: SystemSeverity,
    message: &str,
) -> bool {
    let Some(msg) = app.messages.get_mut(msg_idx) else {
        return false;
    };
    if !matches!(msg.role, MessageRole::System(_)) {
        return false;
    }
    let Some(MessageBlock::Notice(notice)) = msg.blocks.first_mut() else {
        return false;
    };
    if notice.dedup_key.as_ref() != Some(dedup_key) {
        return false;
    }
    msg.role = MessageRole::System(Some(severity));
    notice.severity = severity;
    notice.replace_text(message);
    app.sync_render_cache_slot(msg_idx, 0);
    app.recompute_message_retained_bytes(msg_idx);
    app.invalidate_layout(InvalidationLevel::MessageChanged(msg_idx));
    true
}

fn remove_standalone_notice(app: &mut App, msg_idx: usize) -> bool {
    let Some(msg) = app.messages.get(msg_idx) else {
        return false;
    };
    let has_notice = matches!(msg.role, MessageRole::System(_))
        && matches!(msg.blocks.as_slice(), [MessageBlock::Notice(_)]);
    if !has_notice {
        return false;
    }
    app.remove_message_tracked(msg_idx).is_some()
}

fn prune_invalid_turn_notice_refs(app: &mut App) {
    app.turn_notice_refs.retain(|notice_ref| match &notice_ref.location {
        TurnNoticeLocation::Inline { msg_idx, block_idx } => matches!(
            app.messages.get(*msg_idx).and_then(|msg| msg.blocks.get(*block_idx)),
            Some(MessageBlock::Notice(notice))
                if notice.dedup_key.as_ref() == Some(&notice_ref.dedup_key)
        ),
        TurnNoticeLocation::Standalone { msg_idx } => matches!(
            app.messages.get(*msg_idx),
            Some(ChatMessage {
                role: MessageRole::System(_),
                blocks,
                ..
            }) if matches!(
                blocks.as_slice(),
                [MessageBlock::Notice(notice)]
                    if notice.dedup_key.as_ref() == Some(&notice_ref.dedup_key)
            )
        ),
    });
}

#[cfg(test)]
mod tests {
    use super::{update_inline_notice, upsert_turn_notice};
    use crate::app::{App, ChatMessage, MessageBlock, MessageRole, NoticeStage, SystemSeverity};

    #[test]
    fn inline_notice_insert_updates_canonical_assistant_message() {
        let mut app = App::test_default();
        app.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);

        upsert_turn_notice(
            &mut app,
            crate::app::NoticeDedupKey::ApiRetry,
            NoticeStage::Warning,
            SystemSeverity::Warning,
            "retrying",
        );

        assert_eq!(app.messages[0].blocks.len(), 1);
        let Some(MessageBlock::Notice(notice)) = app.messages[0].blocks.first() else {
            panic!("expected notice block");
        };
        assert_eq!(notice.severity, SystemSeverity::Warning);
        assert_eq!(notice.text.text, "retrying");
    }

    #[test]
    fn inline_notice_update_mutates_canonical_notice() {
        let mut app = App::test_default();
        app.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);

        upsert_turn_notice(
            &mut app,
            crate::app::NoticeDedupKey::ApiRetry,
            NoticeStage::Warning,
            SystemSeverity::Warning,
            "retrying",
        );
        assert!(update_inline_notice(
            &mut app,
            0,
            0,
            &crate::app::NoticeDedupKey::ApiRetry,
            SystemSeverity::Error,
            "failed",
        ));

        let Some(MessageBlock::Notice(notice)) = app.messages[0].blocks.first() else {
            panic!("expected notice block");
        };
        assert_eq!(notice.severity, SystemSeverity::Error);
        assert_eq!(notice.text.text, "failed");
    }
}
