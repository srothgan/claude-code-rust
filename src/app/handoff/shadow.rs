// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::commit::{apply_successful_commit, plan_handoff, prepare_for_turn_exit};
use super::projection::InlineOutputState;
use super::serialize::serialize_assistant_prefix;
use super::stabilizer::{append_text_chunk, insert_notice, insert_tool};
use super::types::{
    AssistantCommittedUnit, AssistantFormattingState, AssistantTurnId, LiveAssistantIndicator,
    LiveAssistantTurn, LiveAssistantUnit, LiveNoticeUnit, LiveToolUnit, LiveUnitId,
    SystemTranscriptEntry, TerminalMutationState, ToolTranscriptSnapshot, TranscriptEntry,
    UserTranscriptBlock, UserTranscriptEntry, WelcomeTranscriptEntry,
};
use crate::agent::model::ToolCallStatus;
use crate::app::{
    App, AppStatus, ChatMessage, MessageBlock, MessageRole, NoticeDedupKey, SystemSeverity,
    ToolCallInfo, WelcomeBlock,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandoffShadowState {
    pub next_turn_id: u64,
    pub active_turn: Option<ActiveAssistantShadowTurn>,
    pub last_finished_turn: Option<FinishedAssistantShadowTurn>,
    pub inline_output: InlineOutputState,
}

impl Default for HandoffShadowState {
    fn default() -> Self {
        Self {
            next_turn_id: 1,
            active_turn: None,
            last_finished_turn: None,
            inline_output: InlineOutputState::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveAssistantShadowTurn {
    pub committed_entries: Vec<TranscriptEntry>,
    pub live: LiveAssistantTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinishedAssistantShadowTurn {
    pub turn_id: AssistantTurnId,
    pub transcript_entries: Vec<TranscriptEntry>,
}

impl ActiveAssistantShadowTurn {
    fn new(turn_id: AssistantTurnId) -> Self {
        Self { committed_entries: Vec::new(), live: LiveAssistantTurn::new(turn_id) }
    }
}

#[must_use]
pub(crate) fn begin_local_assistant_turn(shadow: &mut HandoffShadowState) -> AssistantTurnId {
    let turn_id = allocate_turn_id(shadow);
    shadow.active_turn = Some(ActiveAssistantShadowTurn::new(turn_id));
    turn_id
}

pub(crate) fn ensure_active_turn(
    shadow: &mut HandoffShadowState,
) -> &mut ActiveAssistantShadowTurn {
    if shadow.active_turn.is_none() {
        let turn_id = allocate_turn_id(shadow);
        shadow.active_turn = Some(ActiveAssistantShadowTurn::new(turn_id));
    }
    match shadow.active_turn.as_mut() {
        Some(turn) => turn,
        None => unreachable!("active turn must exist after initialization"),
    }
}

pub(crate) fn clear_shadow_state(shadow: &mut HandoffShadowState) {
    *shadow = HandoffShadowState::default();
}

pub(crate) fn sync_live_indicator(
    shadow: &mut HandoffShadowState,
    indicator: Option<LiveAssistantIndicator>,
) {
    if let Some(turn) = shadow.active_turn.as_mut() {
        turn.live.set_live_indicator(indicator);
    }
}

#[must_use]
pub(crate) fn current_shadow_live_indicator(app: &App) -> Option<LiveAssistantIndicator> {
    app.handoff_shadow.active_turn.as_ref()?;
    if app.is_compacting {
        return Some(LiveAssistantIndicator::Compacting);
    }
    if matches!(app.status, AppStatus::Thinking) {
        return Some(LiveAssistantIndicator::Thinking);
    }
    if matches!(app.status, AppStatus::Running)
        && app
            .active_turn_assistant_idx()
            .and_then(|idx| app.messages.get(idx))
            .is_some_and(|msg| msg.blocks.is_empty())
    {
        return Some(LiveAssistantIndicator::Thinking);
    }
    None
}

pub(crate) fn sync_shadow_live_indicator(app: &mut App) {
    let indicator = current_shadow_live_indicator(app);
    sync_live_indicator(&mut app.handoff_shadow, indicator);
}

pub(crate) fn mirror_text_chunk(shadow: &mut HandoffShadowState, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    let turn = ensure_active_turn(shadow);
    append_text_chunk(&mut turn.live, chunk);
}

pub(crate) fn mirror_tool_snapshot(shadow: &mut HandoffShadowState, tool: LiveToolUnit) {
    let tool_call_id = tool.snapshot.tool_call_id.clone();
    let turn = ensure_active_turn(shadow);
    if let Some(existing) = turn.live.tool_mut_by_call_id(&tool_call_id) {
        let preserved_id = existing.id;
        *existing = tool;
        existing.id = preserved_id;
        return;
    }
    if committed_entries_contain_tool(turn, &tool_call_id) {
        tracing::warn!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "handoff_shadow_tool_update_ignored",
            message = "shadow tool update ignored because the tool was already committed",
            outcome = "ignored",
            tool_call_id = %tool_call_id,
        );
        debug_assert!(false, "shadow tool update must not mutate committed transcript state");
        return;
    }
    insert_tool(&mut turn.live, tool);
}

pub(crate) fn mirror_inline_notice_insert(shadow: &mut HandoffShadowState, notice: LiveNoticeUnit) {
    let dedup_key = notice.dedup_key.clone();
    let turn = ensure_active_turn(shadow);
    if let Some(key) = dedup_key.as_ref() {
        if let Some(existing) = turn.live.notice_mut_by_key(key) {
            let preserved_id = existing.id;
            *existing = notice;
            existing.id = preserved_id;
            return;
        }
        if committed_entries_contain_notice(turn, key) {
            tracing::warn!(
                target: crate::logging::targets::APP_SESSION,
                event_name = "handoff_shadow_notice_insert_ignored",
                message = "shadow inline notice insert ignored because the notice was already committed",
                outcome = "ignored",
                dedup_key = ?key,
            );
            debug_assert!(false, "shadow notice insert must not mutate committed transcript state");
            return;
        }
    }
    insert_notice(&mut turn.live, notice);
}

pub(crate) fn mirror_inline_notice_update(
    shadow: &mut HandoffShadowState,
    dedup_key: &NoticeDedupKey,
    severity: SystemSeverity,
    text: &str,
) {
    let turn = ensure_active_turn(shadow);
    if let Some(existing) = turn.live.notice_mut_by_key(dedup_key) {
        existing.severity = severity;
        text.clone_into(&mut existing.text);
        return;
    }
    if committed_entries_contain_notice(turn, dedup_key) {
        tracing::warn!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "handoff_shadow_notice_update_ignored",
            message = "shadow inline notice update ignored because the notice was already committed",
            outcome = "ignored",
            dedup_key = ?dedup_key,
        );
        debug_assert!(false, "shadow notice update must not mutate committed transcript state");
    }
}

pub(crate) fn mirror_tool_interaction_flags(
    shadow: &mut HandoffShadowState,
    tool_call_id: &str,
    pending_permission: bool,
    pending_question: bool,
) {
    let turn = ensure_active_turn(shadow);
    if let Some(existing) = turn.live.tool_mut_by_call_id(tool_call_id) {
        existing.pending_permission = pending_permission;
        existing.pending_question = pending_question;
        return;
    }
    if committed_entries_contain_tool(turn, tool_call_id) {
        tracing::warn!(
            target: crate::logging::targets::APP_SESSION,
            event_name = "handoff_shadow_tool_interaction_ignored",
            message = "shadow tool interaction update ignored because the tool was already committed",
            outcome = "ignored",
            tool_call_id = %tool_call_id,
        );
        debug_assert!(
            false,
            "shadow tool interaction update must not mutate committed transcript state"
        );
    }
}

pub(crate) fn mirror_turn_exit(
    shadow: &mut HandoffShadowState,
    final_status: ToolCallStatus,
) -> Vec<TranscriptEntry> {
    let Some(mut turn) = shadow.active_turn.take() else {
        return Vec::new();
    };

    prepare_for_turn_exit(&mut turn.live, final_status);
    let newly_committed_entries = commit_ready_prefix(&mut turn);
    debug_assert!(
        turn.live.units.is_empty(),
        "turn exit should leave no live units after prefix commit"
    );

    if turn.committed_entries.is_empty() && turn.live.units.is_empty() {
        return Vec::new();
    }

    shadow.last_finished_turn = Some(FinishedAssistantShadowTurn {
        turn_id: turn.live.turn_id,
        transcript_entries: turn.committed_entries.clone(),
    });
    newly_committed_entries
}

pub(crate) fn commit_ready_prefix(turn: &mut ActiveAssistantShadowTurn) -> Vec<TranscriptEntry> {
    let decision = plan_handoff(&turn.live);
    if decision.committed_prefix_len == 0 {
        return Vec::new();
    }
    turn.committed_entries.extend(decision.transcript_entries.iter().cloned());
    apply_successful_commit(&mut turn.live, &decision);
    decision.transcript_entries
}

#[must_use]
pub(crate) fn active_assistant_projection_anchor(app: &App) -> Option<(usize, AssistantTurnId)> {
    let msg_idx = app.active_turn_assistant_idx()?;
    let turn_id = app.handoff_shadow.active_turn.as_ref()?.live.turn_id;
    Some((msg_idx, turn_id))
}

fn active_assistant_needs_live_slot(turn: &ActiveAssistantShadowTurn) -> bool {
    !turn.live.units.is_empty() || turn.live.live_indicator.is_some()
}

fn sync_active_assistant_live_slot(app: &mut App, msg_idx: usize, turn_id: AssistantTurnId) {
    let needs_live_slot =
        app.handoff_shadow.active_turn.as_ref().is_some_and(active_assistant_needs_live_slot);
    if needs_live_slot {
        app.handoff_shadow.inline_output.record_assistant_live_slot(msg_idx, turn_id);
    } else {
        app.handoff_shadow.inline_output.remove_assistant_live_slot(msg_idx, turn_id);
    }
}

fn record_assistant_commits_for_anchor(
    app: &mut App,
    msg_idx: usize,
    turn_id: AssistantTurnId,
    committed_entries: Vec<TranscriptEntry>,
) {
    if committed_entries.is_empty() {
        return;
    }
    app.handoff_shadow.inline_output.record_assistant_committed_entries(
        msg_idx,
        turn_id,
        committed_entries,
    );
    app.mark_committed_output_changed();
}

pub(crate) fn record_assistant_turn_exit_projection(
    app: &mut App,
    anchor: Option<(usize, AssistantTurnId)>,
    committed_entries: Vec<TranscriptEntry>,
) {
    let Some((msg_idx, turn_id)) = anchor else {
        debug_assert!(
            committed_entries.is_empty(),
            "assistant commits require an active assistant projection anchor"
        );
        return;
    };

    record_assistant_commits_for_anchor(app, msg_idx, turn_id, committed_entries);
    app.handoff_shadow.inline_output.remove_assistant_live_slot(msg_idx, turn_id);
}

pub(crate) fn sync_handoff_commit_queue(app: &mut App) {
    let anchor = active_assistant_projection_anchor(app);
    let committed_entries =
        app.handoff_shadow.active_turn.as_mut().map(commit_ready_prefix).unwrap_or_default();
    if let Some((msg_idx, turn_id)) = anchor {
        record_assistant_commits_for_anchor(app, msg_idx, turn_id, committed_entries);
    } else {
        debug_assert!(
            committed_entries.is_empty(),
            "assistant commits require an active assistant projection anchor"
        );
    }
    sync_shadow_live_indicator(app);
    if let Some((msg_idx, turn_id)) = anchor {
        sync_active_assistant_live_slot(app, msg_idx, turn_id);
    }
}

#[must_use]
pub(crate) fn terminal_mutation_state(app: &App, tc: &ToolCallInfo) -> TerminalMutationState {
    if !tc.is_execute_tool() {
        return TerminalMutationState::None;
    }

    if let Some((msg_idx, block_idx)) = app.lookup_tool_call(&tc.id)
        && app
            .terminal_tool_calls
            .iter()
            .any(|entry| entry.msg_idx == msg_idx && entry.block_idx == block_idx)
    {
        return TerminalMutationState::Streaming;
    }

    if matches!(tc.status, ToolCallStatus::Pending | ToolCallStatus::InProgress) {
        return TerminalMutationState::AwaitingFinalSnapshot;
    }

    TerminalMutationState::Settled
}

#[must_use]
pub(crate) fn live_tool_unit_from_info(app: &App, tc: &ToolCallInfo) -> LiveToolUnit {
    LiveToolUnit {
        id: LiveUnitId(0),
        snapshot: ToolTranscriptSnapshot::from_tool_call_info(tc),
        pending_permission: tc.pending_permission.is_some(),
        pending_question: tc.pending_question.is_some(),
        terminal_mutation: terminal_mutation_state(app, tc),
    }
}

pub(crate) fn mirror_visible_tool_snapshot(app: &mut App, tool_call_id: &str) {
    let snapshot = {
        let Some((msg_idx, block_idx)) = app.lookup_tool_call(tool_call_id) else {
            return;
        };
        let Some(MessageBlock::ToolCall(tc)) =
            app.messages.get(msg_idx).and_then(|message| message.blocks.get(block_idx))
        else {
            return;
        };
        live_tool_unit_from_info(app, tc.as_ref())
    };

    mirror_tool_snapshot(&mut app.handoff_shadow, snapshot);
}

pub(crate) fn mirror_visible_live_tools(app: &mut App) {
    let tool_ids = app
        .handoff_shadow
        .active_turn
        .as_ref()
        .map(|turn| {
            turn.live
                .units
                .iter()
                .filter_map(|unit| match unit {
                    LiveAssistantUnit::Tool(tool) => Some(tool.snapshot.tool_call_id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for tool_id in tool_ids {
        mirror_visible_tool_snapshot(app, &tool_id);
    }
}

fn allocate_turn_id(shadow: &mut HandoffShadowState) -> AssistantTurnId {
    let turn_id = AssistantTurnId(shadow.next_turn_id);
    shadow.next_turn_id = shadow.next_turn_id.saturating_add(1);
    turn_id
}

fn committed_entries_contain_tool(turn: &ActiveAssistantShadowTurn, tool_call_id: &str) -> bool {
    turn.committed_entries.iter().any(|entry| match entry {
        TranscriptEntry::AssistantOpen(entry) | TranscriptEntry::AssistantContinue(entry) => {
            matches!(
                &entry.unit,
                AssistantCommittedUnit::Tool(tool)
                    if tool.snapshot.tool_call_id == tool_call_id
            )
        }
        TranscriptEntry::Welcome(_) | TranscriptEntry::User(_) | TranscriptEntry::System(_) => {
            false
        }
    })
}

fn committed_entries_contain_notice(
    _turn: &ActiveAssistantShadowTurn,
    _dedup_key: &NoticeDedupKey,
) -> bool {
    false
}

#[must_use]
pub(crate) fn transcript_entries_from_message(message: &ChatMessage) -> Vec<TranscriptEntry> {
    match &message.role {
        MessageRole::Welcome => transcript_entries_from_welcome_message(message),
        MessageRole::User => transcript_entries_from_user_message(message),
        MessageRole::System(severity) => transcript_entries_from_system_message(message, *severity),
        MessageRole::Assistant => transcript_entries_from_assistant_message(message),
    }
}

fn transcript_entries_from_welcome_message(message: &ChatMessage) -> Vec<TranscriptEntry> {
    let Some(MessageBlock::Welcome(WelcomeBlock {
        version,
        subscription,
        cwd,
        session_id,
        tip_seed,
        ..
    })) = message.blocks.first()
    else {
        return Vec::new();
    };

    let session_ready = !session_id.trim().is_empty() && session_id != "-";
    let subscription_ready = !subscription.trim().is_empty() && subscription != "-";

    // Temporary gate: commit the welcome only once the overview has the key
    // metadata we want to show in transcript history.
    //
    // TODO(inline-welcome): move welcome rendering into the live area so it
    // appears immediately and can progressively populate overview fields as
    // session/account data arrives. Remove this committed-output gate once the
    // live welcome path exists.
    if !session_ready || !subscription_ready {
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "inline_chat_welcome_deferred",
            message = "welcome transcript entry deferred until overview metadata is ready",
            outcome = "deferred",
            session_ready,
            subscription_ready,
            session_id = %session_id,
            subscription = %subscription,
        );
        return Vec::new();
    }

    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "inline_chat_welcome_ready",
        message = "welcome transcript entry became eligible for committed output",
        outcome = "ready",
        session_id = %session_id,
        subscription = %subscription,
    );

    vec![TranscriptEntry::Welcome(WelcomeTranscriptEntry {
        version: version.clone(),
        subscription: subscription.clone(),
        cwd: cwd.clone(),
        session_id: session_id.clone(),
        tip_seed: *tip_seed,
    })]
}

fn transcript_entries_from_user_message(message: &ChatMessage) -> Vec<TranscriptEntry> {
    let blocks = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(text) => Some(UserTranscriptBlock::Text(text.text.clone())),
            MessageBlock::ImageAttachment(image) => {
                Some(UserTranscriptBlock::ImageAttachment { count: image.count })
            }
            MessageBlock::Notice(_) | MessageBlock::ToolCall(_) | MessageBlock::Welcome(_) => None,
        })
        .collect::<Vec<_>>();

    if blocks.is_empty() {
        Vec::new()
    } else {
        vec![TranscriptEntry::User(UserTranscriptEntry { blocks })]
    }
}

fn transcript_entries_from_system_message(
    message: &ChatMessage,
    default_severity: Option<SystemSeverity>,
) -> Vec<TranscriptEntry> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(text) => Some(TranscriptEntry::System(SystemTranscriptEntry {
                severity: default_severity,
                text: text.text.clone(),
            })),
            MessageBlock::Notice(notice) => Some(TranscriptEntry::System(SystemTranscriptEntry {
                severity: Some(notice.severity),
                text: notice.text.text.clone(),
            })),
            MessageBlock::ToolCall(_)
            | MessageBlock::Welcome(_)
            | MessageBlock::ImageAttachment(_) => None,
        })
        .collect()
}

fn transcript_entries_from_assistant_message(message: &ChatMessage) -> Vec<TranscriptEntry> {
    let mut next_unit_id = 1_u64;
    let live_units = message
        .blocks
        .iter()
        .filter_map(|block| assistant_live_unit_from_block(block, &mut next_unit_id))
        .collect::<Vec<_>>();
    serialize_assistant_prefix(&live_units, &AssistantFormattingState::default())
}

fn assistant_live_unit_from_block(
    block: &MessageBlock,
    next_unit_id: &mut u64,
) -> Option<LiveAssistantUnit> {
    let mut alloc_id = || {
        let id = LiveUnitId(*next_unit_id);
        *next_unit_id = (*next_unit_id).saturating_add(1);
        id
    };

    match block {
        MessageBlock::Text(text) => {
            Some(LiveAssistantUnit::StableText(super::types::StableTextUnit {
                id: alloc_id(),
                text: text.text.clone(),
                trailing_spacing: text.trailing_spacing,
            }))
        }
        MessageBlock::Notice(notice) => Some(LiveAssistantUnit::Notice(LiveNoticeUnit {
            id: alloc_id(),
            dedup_key: notice.dedup_key.clone(),
            severity: notice.severity,
            text: notice.text.text.clone(),
            trailing_spacing: notice.text.trailing_spacing,
            mutability: super::types::NoticeMutability::Final,
        })),
        MessageBlock::ToolCall(tc) if !tc.hidden_unless_focused_interaction() => Some(
            LiveAssistantUnit::Tool(live_tool_unit_from_info_for_history(tc.as_ref(), alloc_id())),
        ),
        MessageBlock::ToolCall(_) | MessageBlock::Welcome(_) | MessageBlock::ImageAttachment(_) => {
            None
        }
    }
}

fn live_tool_unit_from_info_for_history(tc: &ToolCallInfo, id: LiveUnitId) -> LiveToolUnit {
    LiveToolUnit {
        id,
        snapshot: ToolTranscriptSnapshot::from_tool_call_info(tc),
        pending_permission: tc.pending_permission.is_some(),
        pending_question: tc.pending_question.is_some(),
        terminal_mutation: TerminalMutationState::Settled,
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectedAssistantUnit {
    Text {
        text: String,
        trailing_spacing: crate::app::TextBlockSpacing,
    },
    Notice {
        dedup_key: Option<NoticeDedupKey>,
        severity: SystemSeverity,
        text: String,
        trailing_spacing: crate::app::TextBlockSpacing,
    },
    Tool {
        snapshot: ToolTranscriptSnapshot,
    },
}

#[cfg(test)]
fn project_shadow_units(turn: &ActiveAssistantShadowTurn) -> Vec<ProjectedAssistantUnit> {
    let mut units = Vec::new();

    for entry in &turn.committed_entries {
        let assistant_entry = match entry {
            TranscriptEntry::AssistantOpen(entry) | TranscriptEntry::AssistantContinue(entry) => {
                entry
            }
            TranscriptEntry::Welcome(_) | TranscriptEntry::User(_) | TranscriptEntry::System(_) => {
                continue;
            }
        };

        units.push(match &assistant_entry.unit {
            AssistantCommittedUnit::Text(text) => ProjectedAssistantUnit::Text {
                text: text.text.clone(),
                trailing_spacing: text.trailing_spacing,
            },
            AssistantCommittedUnit::Notice(notice) => ProjectedAssistantUnit::Notice {
                dedup_key: None,
                severity: notice.severity,
                text: notice.text.clone(),
                trailing_spacing: notice.trailing_spacing,
            },
            AssistantCommittedUnit::Tool(tool) => {
                ProjectedAssistantUnit::Tool { snapshot: tool.snapshot.clone() }
            }
        });
    }

    for unit in &turn.live.units {
        units.push(match unit {
            LiveAssistantUnit::StableText(text) => ProjectedAssistantUnit::Text {
                text: text.text.clone(),
                trailing_spacing: text.trailing_spacing,
            },
            LiveAssistantUnit::MutableTextTail(text) => ProjectedAssistantUnit::Text {
                text: text.text.clone(),
                trailing_spacing: crate::app::TextBlockSpacing::None,
            },
            LiveAssistantUnit::Notice(notice) => ProjectedAssistantUnit::Notice {
                dedup_key: notice.dedup_key.clone(),
                severity: notice.severity,
                text: notice.text.clone(),
                trailing_spacing: notice.trailing_spacing,
            },
            LiveAssistantUnit::Tool(tool) => {
                ProjectedAssistantUnit::Tool { snapshot: tool.snapshot.clone() }
            }
        });
    }

    units
}

#[cfg(test)]
fn project_visible_units(app: &App) -> Vec<ProjectedAssistantUnit> {
    let Some(owner_idx) = app.active_turn_assistant_idx() else {
        return Vec::new();
    };
    let Some(message) = app.messages.get(owner_idx) else {
        return Vec::new();
    };

    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            MessageBlock::Text(text) => Some(ProjectedAssistantUnit::Text {
                text: text.text.clone(),
                trailing_spacing: text.trailing_spacing,
            }),
            MessageBlock::Notice(notice) => Some(ProjectedAssistantUnit::Notice {
                dedup_key: notice.dedup_key.clone(),
                severity: notice.severity,
                text: notice.text.text.clone(),
                trailing_spacing: notice.text.trailing_spacing,
            }),
            MessageBlock::ToolCall(tc) => Some(ProjectedAssistantUnit::Tool {
                snapshot: ToolTranscriptSnapshot::from_tool_call_info(tc.as_ref()),
            }),
            MessageBlock::Welcome(_) | MessageBlock::ImageAttachment(_) => None,
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn assert_shadow_matches_visible_active_turn(app: &App) {
    let shadow_units =
        app.handoff_shadow.active_turn.as_ref().map(project_shadow_units).unwrap_or_default();
    let visible_units = project_visible_units(app);
    assert_eq!(shadow_units, visible_units, "shadow units diverged from visible assistant state");

    let expected_indicator = current_shadow_live_indicator(app);
    let actual_indicator =
        app.handoff_shadow.active_turn.as_ref().and_then(|turn| turn.live.live_indicator);
    assert_eq!(actual_indicator, expected_indicator, "shadow indicator drifted from app state");

    if let Some(turn) = app.handoff_shadow.active_turn.as_ref() {
        for unit in &turn.live.units {
            let LiveAssistantUnit::Tool(tool) = unit else {
                continue;
            };
            let Some((msg_idx, block_idx)) = app.lookup_tool_call(&tool.snapshot.tool_call_id)
            else {
                panic!("visible tool lookup missing for {}", tool.snapshot.tool_call_id);
            };
            let Some(MessageBlock::ToolCall(visible)) =
                app.messages.get(msg_idx).and_then(|message| message.blocks.get(block_idx))
            else {
                panic!("visible tool block missing for {}", tool.snapshot.tool_call_id);
            };
            assert_eq!(tool.pending_permission, visible.pending_permission.is_some());
            assert_eq!(tool.pending_question, visible.pending_question.is_some());
            assert_eq!(tool.terminal_mutation, terminal_mutation_state(app, visible));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model;
    use crate::app::handoff::projection::{
        InlineOutputAnchor, InlineOutputItemKind, InlineOutputStatus,
    };
    use crate::app::handoff::types::NoticeMutability;
    use crate::app::{App, MessageBlock, MessageRole, NoticeBlock, TextBlock, TextBlockSpacing};

    fn app_with_active_assistant_turn() -> (App, AssistantTurnId) {
        let mut app = App::test_default();
        app.messages.push(crate::app::ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        let turn_id = begin_local_assistant_turn(&mut app.handoff_shadow);
        (app, turn_id)
    }

    fn tool_unit(
        status: model::ToolCallStatus,
        terminal_mutation: TerminalMutationState,
        pending_permission: bool,
        pending_question: bool,
    ) -> LiveToolUnit {
        LiveToolUnit {
            id: LiveUnitId(1),
            snapshot: ToolTranscriptSnapshot {
                tool_call_id: "tool-1".to_owned(),
                title: "Tool".to_owned(),
                sdk_tool_name: "Bash".to_owned(),
                status,
                hidden: false,
                raw_input: None,
                output_metadata: None,
                task_metadata: None,
                content: Vec::new(),
                terminal_command: None,
                terminal_output: None,
            },
            pending_permission,
            pending_question,
            terminal_mutation,
        }
    }

    fn assert_only_assistant_live_slot(app: &App, turn_id: AssistantTurnId) {
        let items = app.handoff_shadow.inline_output.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].anchor, InlineOutputAnchor::AssistantLive { msg_idx: 0, turn_id });
        assert!(matches!(items[0].kind, InlineOutputItemKind::AssistantLive { .. }));
        assert!(app.handoff_shadow.inline_output.pending_transcript_items().is_empty());
    }

    #[test]
    fn begin_local_turn_creates_active_turn() {
        let mut shadow = HandoffShadowState::default();

        let turn_id = begin_local_assistant_turn(&mut shadow);

        assert_eq!(turn_id, AssistantTurnId(1));
        assert_eq!(shadow.next_turn_id, 2);
        assert!(shadow.active_turn.is_some());
    }

    #[test]
    fn mirror_turn_exit_records_finished_turn_and_clears_active_turn() {
        let mut shadow = HandoffShadowState::default();
        mirror_text_chunk(&mut shadow, "hello");
        let turn = ensure_active_turn(&mut shadow);
        turn.live.units.push(LiveAssistantUnit::Notice(LiveNoticeUnit {
            id: LiveUnitId(2),
            dedup_key: None,
            severity: SystemSeverity::Info,
            text: "note".to_owned(),
            trailing_spacing: TextBlockSpacing::None,
            mutability: NoticeMutability::Upgradeable,
        }));

        let committed = mirror_turn_exit(&mut shadow, model::ToolCallStatus::Completed);

        assert!(shadow.active_turn.is_none());
        let finished = shadow.last_finished_turn.expect("finished turn");
        assert_eq!(finished.turn_id, AssistantTurnId(1));
        assert_eq!(finished.transcript_entries.len(), 2);
        assert_eq!(committed.len(), 2);
    }

    #[test]
    fn mirror_turn_exit_returns_only_final_delta_after_incremental_commit() {
        let mut shadow = HandoffShadowState::default();
        let _ = begin_local_assistant_turn(&mut shadow);
        mirror_text_chunk(&mut shadow, "I'll check the uncommitted changes.");
        mirror_tool_snapshot(
            &mut shadow,
            LiveToolUnit {
                id: LiveUnitId(2),
                snapshot: ToolTranscriptSnapshot {
                    tool_call_id: "tool-1".to_owned(),
                    title: "Terminal".to_owned(),
                    sdk_tool_name: "Bash".to_owned(),
                    status: model::ToolCallStatus::InProgress,
                    hidden: false,
                    raw_input: None,
                    output_metadata: None,
                    task_metadata: None,
                    content: Vec::new(),
                    terminal_command: None,
                    terminal_output: None,
                },
                pending_permission: false,
                pending_question: false,
                terminal_mutation: TerminalMutationState::Streaming,
            },
        );

        let first_delta = {
            let turn = shadow.active_turn.as_mut().expect("active turn");
            commit_ready_prefix(turn)
        };
        assert_eq!(first_delta.len(), 1);

        let final_delta = mirror_turn_exit(&mut shadow, model::ToolCallStatus::Completed);

        assert_eq!(final_delta.len(), 1);
        assert!(matches!(final_delta[0], TranscriptEntry::AssistantContinue(_)));

        let finished = shadow.last_finished_turn.expect("finished turn");
        assert_eq!(finished.transcript_entries.len(), 2);
        assert!(matches!(finished.transcript_entries[0], TranscriptEntry::AssistantOpen(_)));
        assert!(matches!(finished.transcript_entries[1], TranscriptEntry::AssistantContinue(_)));
    }

    #[test]
    fn sync_handoff_commit_queue_keeps_mutable_text_tail_live_without_commit() {
        let (mut app, turn_id) = app_with_active_assistant_turn();
        mirror_text_chunk(&mut app.handoff_shadow, "still streaming");

        sync_handoff_commit_queue(&mut app);

        assert_only_assistant_live_slot(&app, turn_id);
        let active_turn = app.handoff_shadow.active_turn.as_ref().expect("active turn");
        assert_eq!(active_turn.committed_entries.len(), 0);
    }

    #[test]
    fn sync_handoff_commit_queue_keeps_pending_interaction_tools_live() {
        for (pending_permission, pending_question) in [(true, false), (false, true)] {
            let (mut app, turn_id) = app_with_active_assistant_turn();
            let turn = app.handoff_shadow.active_turn.as_mut().expect("active turn");
            turn.live.units.push(LiveAssistantUnit::Tool(tool_unit(
                model::ToolCallStatus::Completed,
                TerminalMutationState::Settled,
                pending_permission,
                pending_question,
            )));

            sync_handoff_commit_queue(&mut app);

            assert_only_assistant_live_slot(&app, turn_id);
            let active_turn = app.handoff_shadow.active_turn.as_ref().expect("active turn");
            assert_eq!(active_turn.committed_entries.len(), 0);
        }
    }

    #[test]
    fn sync_handoff_commit_queue_keeps_unsettled_execute_tools_live() {
        for terminal_mutation in
            [TerminalMutationState::Streaming, TerminalMutationState::AwaitingFinalSnapshot]
        {
            let (mut app, turn_id) = app_with_active_assistant_turn();
            let turn = app.handoff_shadow.active_turn.as_mut().expect("active turn");
            turn.live.units.push(LiveAssistantUnit::Tool(tool_unit(
                model::ToolCallStatus::Completed,
                terminal_mutation,
                false,
                false,
            )));

            sync_handoff_commit_queue(&mut app);

            assert_only_assistant_live_slot(&app, turn_id);
            let active_turn = app.handoff_shadow.active_turn.as_ref().expect("active turn");
            assert_eq!(active_turn.committed_entries.len(), 0);
        }
    }

    #[test]
    fn sync_handoff_commit_queue_records_sealed_final_tool_commit_in_projection() {
        let (mut app, turn_id) = app_with_active_assistant_turn();
        let turn = app.handoff_shadow.active_turn.as_mut().expect("active turn");
        turn.live.units.push(LiveAssistantUnit::Tool(tool_unit(
            model::ToolCallStatus::Completed,
            TerminalMutationState::Settled,
            false,
            false,
        )));
        turn.live.sealed = true;

        sync_handoff_commit_queue(&mut app);

        let items = app.handoff_shadow.inline_output.items();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].anchor,
            InlineOutputAnchor::AssistantCommit { msg_idx: 0, turn_id, commit_idx: 0 }
        );
        assert!(matches!(
            &items[0].kind,
            InlineOutputItemKind::Transcript {
                entry: TranscriptEntry::AssistantOpen(_),
                status: InlineOutputStatus::PendingInsert,
            }
        ));
        let active_turn = app.handoff_shadow.active_turn.as_ref().expect("active turn");
        assert_eq!(active_turn.committed_entries.len(), 1);
        assert!(active_turn.live.units.is_empty());
    }

    #[test]
    fn assert_shadow_matches_visible_active_turn_accepts_matching_state() {
        let mut app = App::test_default();
        app.status = AppStatus::Thinking;
        app.messages.push(crate::app::ChatMessage::new(MessageRole::Assistant, Vec::new(), None));
        app.bind_active_turn_assistant(0);
        let _ = begin_local_assistant_turn(&mut app.handoff_shadow);
        sync_shadow_live_indicator(&mut app);

        if let Some(message) = app.messages.get_mut(0) {
            message.blocks.push(MessageBlock::Text(TextBlock::from_complete("hello")));
            message.blocks.push(MessageBlock::Notice(NoticeBlock::from_complete(
                SystemSeverity::Info,
                "note",
            )));
        }
        let turn = app.handoff_shadow.active_turn.as_mut().expect("active turn");
        turn.committed_entries.push(TranscriptEntry::AssistantOpen(
            super::super::types::AssistantTranscriptEntry {
                leading_blank_lines: 0,
                unit: AssistantCommittedUnit::Text(super::super::types::CommittedTextUnit {
                    text: "hello".to_owned(),
                    trailing_spacing: TextBlockSpacing::None,
                }),
            },
        ));
        turn.live.units.push(LiveAssistantUnit::Notice(LiveNoticeUnit {
            id: LiveUnitId(2),
            dedup_key: None,
            severity: SystemSeverity::Info,
            text: "note".to_owned(),
            trailing_spacing: TextBlockSpacing::None,
            mutability: NoticeMutability::Upgradeable,
        }));
        turn.live.set_live_indicator(Some(LiveAssistantIndicator::Thinking));

        assert_shadow_matches_visible_active_turn(&app);
    }

    #[test]
    fn live_tool_unit_from_info_tracks_pending_flags() {
        let mut app = App::test_default();
        let tool = ToolCallInfo {
            id: "tool-1".to_owned(),
            title: "Tool".to_owned(),
            sdk_tool_name: "Bash".to_owned(),
            raw_input: None,
            raw_input_bytes: 0,
            output_metadata: None,
            task_metadata: None,
            status: model::ToolCallStatus::InProgress,
            content: Vec::new(),
            hidden: false,
            terminal_id: Some("term-1".to_owned()),
            terminal_command: Some("echo hi".to_owned()),
            terminal_output: None,
            terminal_output_len: 0,
            terminal_bytes_seen: 0,
            terminal_snapshot_mode: crate::app::TerminalSnapshotMode::AppendOnly,
            render_epoch: 0,
            layout_epoch: 0,
            last_measured_width: 0,
            last_measured_height: 0,
            last_measured_layout_epoch: 0,
            last_measured_layout_generation: 0,
            cache: crate::app::BlockCache::default(),
            pending_permission: Some(crate::app::InlinePermission {
                options: Vec::new(),
                display: None,
                response_tx: tokio::sync::oneshot::channel().0,
                selected_index: 0,
                focused: false,
            }),
            pending_question: None,
        };
        app.messages.push(crate::app::ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(tool))],
            None,
        ));
        app.index_tool_call("tool-1".to_owned(), 0, 0);
        app.sync_terminal_tool_call("term-1".to_owned(), 0, 0);

        let MessageBlock::ToolCall(tool) = &app.messages[0].blocks[0] else {
            panic!("expected tool");
        };
        let live = live_tool_unit_from_info(&app, tool.as_ref());

        assert!(live.pending_permission);
        assert!(!live.pending_question);
        assert_eq!(live.terminal_mutation, TerminalMutationState::Streaming);
    }
}
