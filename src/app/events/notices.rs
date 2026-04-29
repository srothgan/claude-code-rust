// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::super::{
    App, ChatMessage, InvalidationLevel, MessageBlock, MessageRole, NoticeBlock, NoticeDedupKey,
    NoticeStage, SystemSeverity, TurnNoticeLocation, TurnNoticeRef,
};

pub(super) fn clear_turn_notice_tracking(app: &mut App) {
    app.clear_turn_notice_refs();
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
                insert_inline_notice(app, owner_idx, dedup_key, stage, severity, message);
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
    if let Some(owner_idx) = app.active_turn_assistant_idx() {
        insert_inline_notice(app, owner_idx, dedup_key, stage, severity, message);
    } else {
        insert_standalone_notice(app, dedup_key, stage, severity, message);
    }
}

fn insert_inline_notice(
    app: &mut App,
    owner_idx: usize,
    dedup_key: NoticeDedupKey,
    stage: NoticeStage,
    severity: SystemSeverity,
    message: &str,
) {
    let Some(owner) = app.messages.get_mut(owner_idx) else {
        insert_standalone_notice(app, dedup_key, stage, severity, message);
        return;
    };
    let block_idx = owner.blocks.len();
    owner.blocks.push(MessageBlock::Notice(
        NoticeBlock::from_complete(severity, message).with_dedup_key(dedup_key.clone()),
    ));
    app.sync_after_message_blocks_changed(owner_idx);
    app.invalidate_layout(InvalidationLevel::MessageChanged(owner_idx));
    app.turn_notice_refs.push(TurnNoticeRef {
        dedup_key,
        stage,
        location: TurnNoticeLocation::Inline { msg_idx: owner_idx, block_idx },
    });
    if let Some(MessageBlock::Notice(notice)) =
        app.messages.get(owner_idx).and_then(|message| message.blocks.get(block_idx))
    {
        let live_notice = crate::app::handoff::types::inline_notice_to_live(
            notice,
            crate::app::handoff::types::LiveUnitId(0),
        );
        crate::app::handoff::shadow::mirror_inline_notice_insert(
            &mut app.handoff_shadow,
            live_notice,
        );
        crate::app::handoff::shadow::sync_handoff_commit_queue(app);
    }
}

fn insert_standalone_notice(
    app: &mut App,
    dedup_key: NoticeDedupKey,
    stage: NoticeStage,
    severity: SystemSeverity,
    message: &str,
) {
    let msg_idx = app.messages.len();
    app.push_message_tracked(ChatMessage::new(
        MessageRole::System(Some(severity)),
        vec![MessageBlock::Notice(
            NoticeBlock::from_complete(severity, message).with_dedup_key(dedup_key.clone()),
        )],
        None,
    ));
    app.enforce_history_retention_tracked();
    app.turn_notice_refs.push(TurnNoticeRef {
        dedup_key,
        stage,
        location: TurnNoticeLocation::Standalone { msg_idx },
    });
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
    crate::app::handoff::shadow::mirror_inline_notice_update(
        &mut app.handoff_shadow,
        dedup_key,
        severity,
        message,
    );
    crate::app::handoff::shadow::sync_handoff_commit_queue(app);
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
    use crate::app::{App, ChatMessage, MessageRole, NoticeStage, SystemSeverity};

    #[test]
    fn inline_notice_insert_is_mirrored_into_shadow() {
        let mut app = App::test_default();
        app.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        let _ = crate::app::handoff::shadow::begin_local_assistant_turn(&mut app.handoff_shadow);

        upsert_turn_notice(
            &mut app,
            crate::app::NoticeDedupKey::ApiRetry,
            NoticeStage::Warning,
            SystemSeverity::Warning,
            "retrying",
        );

        crate::app::handoff::shadow::assert_shadow_matches_visible_active_turn(&app);
        let turn = app.handoff_shadow.active_turn.as_ref().expect("active turn");
        assert_eq!(turn.committed_entries.len(), 0);
        assert_eq!(turn.live.units.len(), 1);
    }

    #[test]
    fn inline_notice_update_mutates_shadow_notice() {
        let mut app = App::test_default();
        app.messages.push(ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        let _ = crate::app::handoff::shadow::begin_local_assistant_turn(&mut app.handoff_shadow);

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

        crate::app::handoff::shadow::assert_shadow_matches_visible_active_turn(&app);
    }
}
