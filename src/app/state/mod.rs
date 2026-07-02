// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

pub mod block_cache;
pub mod cache_metrics;
pub mod chat_render;
mod history_retention;
pub mod messages;
mod render_budget;
pub mod tool_call_info;
pub mod types;

mod app;
mod autocomplete;
mod focus_runtime;
mod git_runtime;
mod paste;
mod repaint;
mod session_identity;
mod startup;
mod tool_tracking;
mod transcript;
mod turn;
mod turn_notices;
mod welcome;

// Re-export all public types so external `use crate::app::state::X` paths still work.
pub use app::App;
pub use autocomplete::AutocompleteKind;
pub use block_cache::BlockCache;
pub use cache_metrics::CacheMetrics;
pub use chat_render::{
    ChatRenderState, ComposerRenderState, LiveRegionRenderState, TerminalSize, TerminalSizeChange,
};
pub(crate) use messages::MarkdownRenderKey;
pub use messages::{
    ChatMessage, ChatMessageId, HistoryOutputId, ImageAttachmentBlock, IncrementalMarkdown,
    MessageBlock, MessageBlockId, MessageRole, NoticeBlock, NoticeDedupKey, RateLimitIncidentKey,
    SystemSeverity, TextBlock, TextBlockSpacing, UserDialogBlock, WelcomeBlock,
    hash_text_block_content, hash_welcome_block_content,
};
pub use paste::PasteState;
pub use repaint::LayoutInvalidation as InvalidationLevel;
pub use repaint::{ChatRenderTraceState, LayoutInvalidation};
pub use startup::StartupState;
pub use tool_call_info::{
    InlinePermission, InlineQuestion, SubagentPermissionContext, TerminalSnapshotMode,
    ToolCallInfo, is_execute_tool_name,
};
pub use tool_tracking::TerminalToolCallRef;
pub use transcript::Transcript;
pub use turn::TurnState;
pub use turn_notices::{NoticeStage, TurnNoticeLocation, TurnNoticeRef};
pub use types::{
    AppStatus, CancelOrigin, ExtraUsage, HistoryRetentionPolicy, HistoryRetentionStats, LoginHint,
    McpState, MessageUsage, ModeInfo, ModeState, PasteSessionState, PendingCommandAck,
    RecentSessionInfo, RenderCacheBudget, SelectionPoint, SessionPickerState, SessionUsageState,
    ToolCallScope, UpdateNoticeState, UsageSnapshot, UsageSourceKind, UsageSourceMode, UsageState,
    UsageWindow,
};

#[allow(unused_imports)]
mod prelude {
    pub(super) use super::app::App;
    pub(super) use super::autocomplete::AutocompleteKind;
    pub(super) use super::cache_metrics;
    pub(super) use super::chat_render::ChatRenderState;
    pub(super) use super::messages::{
        ChatMessage, MessageBlock, MessageRole, NoticeDedupKey, WelcomeBlock,
    };
    pub(super) use super::paste::PasteState;
    pub(super) use super::render_budget;
    pub(super) use super::repaint::LayoutInvalidation as InvalidationLevel;
    pub(super) use super::repaint::{ChatRenderTraceState, LayoutInvalidation};
    pub(super) use super::startup::StartupState;
    pub(super) use super::tool_call_info::ToolCallInfo;
    pub(super) use super::tool_tracking::TerminalToolCallRef;
    pub(super) use super::transcript::Transcript;
    pub(super) use super::turn::TurnState;
    pub(super) use super::turn_notices::{NoticeStage, TurnNoticeLocation, TurnNoticeRef};
    pub(super) use super::types::{
        AppStatus, CancelOrigin, HistoryRetentionPolicy, HistoryRetentionStats, LoginHint,
        McpState, ModeState, PasteSessionState, PendingCommandAck, RecentSessionInfo,
        RenderCacheBudget, SelectionPoint, SessionPickerState, SessionUsageState, ToolCallScope,
        UpdateNoticeState, UsageState,
    };
    pub(super) use super::{BlockCache, CacheMetrics};
    pub(super) use crate::agent::events::ClientEvent;
    pub(super) use crate::agent::model;
    pub(super) use crate::app::config::ConfigState;
    pub(super) use crate::app::file_index;
    pub(super) use crate::app::focus::{FocusContext, FocusManager, FocusOwner, FocusTarget};
    pub(super) use crate::app::git_context::GitContextState;
    pub(super) use crate::app::inline_interactions::{
        clear_inline_interaction_focus, focus_next_inline_interaction,
    };
    pub(super) use crate::app::input::{
        InputSnapshot, InputState, parse_paste_placeholder_before_cursor,
    };
    pub(super) use crate::app::keymap::ResolvedKeymap;
    pub(super) use crate::app::mention;
    pub(super) use crate::app::notify;
    pub(super) use crate::app::paste_burst;
    pub(super) use crate::app::plugins::PluginsState;
    pub(super) use crate::app::slash;
    pub(super) use crate::app::subagent;
    pub(super) use crate::app::trust::TrustState;
    pub(super) use crate::app::view::SurfaceMode;
    pub(super) use crate::app::{
        ChatPurgeReplayOptions, SurfaceDirtyState, TerminalLifecycleState,
    };
    pub(super) use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
    pub(super) use std::path::{Path, PathBuf};
    pub(super) use std::rc::Rc;
    pub(super) use std::sync::mpsc as std_mpsc;
    pub(super) use std::time::Instant;
    pub(super) use tokio::sync::mpsc;
}

#[cfg(test)]
mod tests;
