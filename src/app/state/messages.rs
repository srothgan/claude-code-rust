// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::block_cache::BlockCache;
use super::tool_call_info::ToolCallInfo;
use super::types::MessageUsage;
use crate::agent::model;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TRANSCRIPT_ID: AtomicU64 = AtomicU64::new(1);

fn next_transcript_id() -> u64 {
    NEXT_TRANSCRIPT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChatMessageId(u64);

impl ChatMessageId {
    #[must_use]
    pub fn new() -> Self {
        Self(next_transcript_id())
    }
}

impl Default for ChatMessageId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageBlockId(u64);

impl MessageBlockId {
    #[must_use]
    pub fn new() -> Self {
        Self(next_transcript_id())
    }
}

impl Default for MessageBlockId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HistoryOutputId {
    Message(ChatMessageId),
    AssistantLabel(ChatMessageId),
    AssistantIndicator(ChatMessageId),
    Block(MessageBlockId),
    ToolCall(String),
}

pub struct ChatMessage {
    pub id: ChatMessageId,
    pub role: MessageRole,
    pub blocks: Vec<MessageBlock>,
    pub usage: Option<MessageUsage>,
}

impl ChatMessage {
    #[must_use]
    pub fn new(role: MessageRole, blocks: Vec<MessageBlock>, usage: Option<MessageUsage>) -> Self {
        Self { id: ChatMessageId::new(), role, blocks, usage }
    }

    #[must_use]
    pub fn welcome(version: &str, subscription: &str, cwd: &str, session_id: &str) -> Self {
        Self::new(
            MessageRole::Welcome,
            vec![MessageBlock::Welcome(WelcomeBlock {
                id: MessageBlockId::new(),
                version: version.to_owned(),
                subscription: subscription.to_owned(),
                cwd: cwd.to_owned(),
                session_id: session_id.to_owned(),
                tip_seed: random_welcome_tip_seed(),
                cache: BlockCache::default(),
            })],
            None,
        )
    }
}

#[must_use]
pub fn hash_text_block_content(text: &str, trailing_spacing: TextBlockSpacing) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    trailing_spacing.hash(&mut hasher);
    hasher.finish()
}

#[must_use]
pub fn hash_welcome_block_content(block: &WelcomeBlock) -> u64 {
    let mut hasher = DefaultHasher::new();
    block.version.hash(&mut hasher);
    block.subscription.hash(&mut hasher);
    block.cwd.hash(&mut hasher);
    block.session_id.hash(&mut hasher);
    block.tip_seed.hash(&mut hasher);
    hasher.finish()
}

fn random_welcome_tip_seed() -> u64 {
    let mut hasher = DefaultHasher::new();
    SystemTime::now().duration_since(UNIX_EPOCH).ok().hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextBlockSpacing {
    #[default]
    None,
    ParagraphBreak,
}

impl TextBlockSpacing {
    #[must_use]
    pub fn blank_lines(self) -> usize {
        match self {
            Self::None => 0,
            Self::ParagraphBreak => 1,
        }
    }
}

pub struct TextBlock {
    pub id: MessageBlockId,
    pub source_message_uuids: Vec<String>,
    pub text: String,
    pub cache: BlockCache,
    /// Explicit visual spacing after this block.
    ///
    /// This is used when streaming splits one logical assistant message into
    /// multiple cached blocks at paragraph boundaries. Rendering consumes this
    /// metadata directly so spacing, height measurement, and scroll skipping all
    /// agree without mutating source text.
    pub trailing_spacing: TextBlockSpacing,
}

impl TextBlock {
    #[must_use]
    pub fn new(text: String) -> Self {
        Self::new_with_id(MessageBlockId::new(), text)
    }

    #[must_use]
    pub fn new_with_id(id: MessageBlockId, text: String) -> Self {
        Self {
            id,
            source_message_uuids: Vec::new(),
            text,
            cache: BlockCache::default(),
            trailing_spacing: TextBlockSpacing::None,
        }
    }

    #[must_use]
    pub fn from_complete(text: &str) -> Self {
        Self::new(text.to_owned())
    }

    #[must_use]
    pub fn with_source_message_uuid(mut self, source_message_uuid: Option<&str>) -> Self {
        self.add_source_message_uuid(source_message_uuid);
        self
    }

    #[must_use]
    pub fn with_source_message_uuids(mut self, source_message_uuids: Vec<String>) -> Self {
        for source_message_uuid in source_message_uuids {
            self.add_source_message_uuid(Some(&source_message_uuid));
        }
        self
    }

    pub fn add_source_message_uuid(&mut self, source_message_uuid: Option<&str>) -> bool {
        let Some(source_message_uuid) =
            source_message_uuid.map(str::trim).filter(|uuid| !uuid.is_empty())
        else {
            return false;
        };
        if self.source_message_uuids.iter().any(|uuid| uuid == source_message_uuid) {
            return false;
        }
        self.source_message_uuids.push(source_message_uuid.to_owned());
        true
    }

    #[must_use]
    pub fn has_source_message_uuid(&self, source_message_uuid: &str) -> bool {
        self.source_message_uuids.iter().any(|uuid| uuid == source_message_uuid)
    }

    #[must_use]
    pub fn with_trailing_spacing(mut self, trailing_spacing: TextBlockSpacing) -> Self {
        self.trailing_spacing = trailing_spacing;
        self
    }

    #[must_use]
    pub fn trailing_blank_lines(&self) -> usize {
        self.trailing_spacing.blank_lines()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateLimitIncidentKey {
    pub rate_limit_type: Option<String>,
    pub limit_scope: Option<String>,
    pub resets_at_bucket: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NoticeDedupKey {
    RateLimit(RateLimitIncidentKey),
    ApiRetry,
    ActiveTurnSubmissionBlocked,
}

pub struct NoticeBlock {
    pub id: MessageBlockId,
    pub severity: SystemSeverity,
    pub text: TextBlock,
    pub dedup_key: Option<NoticeDedupKey>,
}

impl NoticeBlock {
    #[must_use]
    pub fn new(severity: SystemSeverity, text: String) -> Self {
        Self { id: MessageBlockId::new(), severity, text: TextBlock::new(text), dedup_key: None }
    }

    #[must_use]
    pub fn from_complete(severity: SystemSeverity, text: &str) -> Self {
        Self::new(severity, text.to_owned())
    }

    #[must_use]
    pub fn with_dedup_key(mut self, dedup_key: NoticeDedupKey) -> Self {
        self.dedup_key = Some(dedup_key);
        self
    }

    pub fn replace_text(&mut self, text: &str) {
        self.id = MessageBlockId::new();
        self.text = TextBlock::from_complete(text);
    }

    #[must_use]
    pub fn trailing_blank_lines(&self) -> usize {
        self.text.trailing_blank_lines()
    }
}

/// Ordered content block - text and tool calls interleaved as they arrive.
pub enum MessageBlock {
    Text(TextBlock),
    BtwExchange(BtwExchangeBlock),
    Notice(NoticeBlock),
    ToolCall(Box<ToolCallInfo>),
    Welcome(WelcomeBlock),
    /// Indicates N images were attached to this user message.
    ImageAttachment(ImageAttachmentBlock),
    /// Turn-level user dialog (e.g. `refusal_fallback_prompt`) rendered inline as
    /// a selectable chooser. Not anchored to a tool call.
    UserDialog(UserDialogBlock),
}

/// Presentation-only side-question exchange. It is never reconstructed as main conversation
/// history and uses one immutable render cache for the complete bordered card.
pub struct BtwExchangeBlock {
    pub id: MessageBlockId,
    pub question: String,
    pub answer: String,
    pub cache: BlockCache,
}

impl BtwExchangeBlock {
    #[must_use]
    pub fn new(question: String, answer: String) -> Self {
        Self { id: MessageBlockId::new(), question, answer, cache: BlockCache::default() }
    }
}

/// Inline chooser for a turn-level `request_user_dialog`. Carries the oneshot
/// responder back to the bridge; `response_tx` is taken once the user answers.
pub struct UserDialogBlock {
    pub id: MessageBlockId,
    pub request_id: String,
    pub payload: model::RefusalFallbackPayload,
    pub options: Vec<model::UserDialogOption>,
    pub selected_index: usize,
    /// Whether this dialog currently has keyboard focus (shows the selection
    /// arrow and accepts navigation/confirm input).
    pub focused: bool,
    /// The resolved choice or cancellation; None means the dialog is pending.
    pub outcome: Option<model::RequestUserDialogOutcome>,
    pub response_tx: Option<tokio::sync::oneshot::Sender<model::RequestUserDialogResponse>>,
    pub cache: BlockCache,
}

impl UserDialogBlock {
    #[must_use]
    pub fn new(
        request: model::RequestUserDialogRequest,
        response_tx: tokio::sync::oneshot::Sender<model::RequestUserDialogResponse>,
    ) -> Self {
        Self {
            id: MessageBlockId::new(),
            request_id: request.request_id,
            payload: request.payload,
            options: request.options,
            selected_index: 0,
            focused: false,
            outcome: None,
            response_tx: Some(response_tx),
            cache: BlockCache::default(),
        }
    }

    pub fn resolve(&mut self, outcome: model::RequestUserDialogOutcome) {
        self.outcome = Some(outcome);
        self.focused = false;
        self.cache.invalidate();
    }
}

/// Lightweight block for image attachment indicators. Carries a [`BlockCache`]
/// to satisfy the render-budget invariant that every [`MessageBlock`] variant
/// has a cache, even though the cached content is trivially small.
pub struct ImageAttachmentBlock {
    pub id: MessageBlockId,
    pub count: usize,
    pub cache: BlockCache,
}

impl ImageAttachmentBlock {
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self { id: MessageBlockId::new(), count, cache: BlockCache::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System(Option<SystemSeverity>),
    Welcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemSeverity {
    Info,
    Warning,
    Error,
}

pub struct WelcomeBlock {
    pub id: MessageBlockId,
    pub version: String,
    pub subscription: String,
    pub cwd: String,
    pub session_id: String,
    pub tip_seed: u64,
    pub cache: BlockCache,
}
