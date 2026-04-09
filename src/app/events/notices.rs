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
                app.viewport.engage_auto_scroll();
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
                app.viewport.engage_auto_scroll();
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
    app.viewport.engage_auto_scroll();
}

fn insert_standalone_notice(
    app: &mut App,
    dedup_key: NoticeDedupKey,
    stage: NoticeStage,
    severity: SystemSeverity,
    message: &str,
) {
    let msg_idx = app.messages.len();
    app.push_message_tracked(ChatMessage {
        role: MessageRole::System(Some(severity)),
        blocks: vec![MessageBlock::Notice(
            NoticeBlock::from_complete(severity, message).with_dedup_key(dedup_key.clone()),
        )],
        usage: None,
    });
    app.enforce_history_retention_tracked();
    app.turn_notice_refs.push(TurnNoticeRef {
        dedup_key,
        stage,
        location: TurnNoticeLocation::Standalone { msg_idx },
    });
    app.viewport.engage_auto_scroll();
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
