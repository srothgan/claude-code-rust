// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::block_cache::BlockCache;
use super::tool_call_info::ToolCallInfo;
use super::types::MessageUsage;
use ratatui::style::Color;
use ratatui::text::Line;
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ChatMessage {
    pub role: MessageRole,
    pub blocks: Vec<MessageBlock>,
    pub usage: Option<MessageUsage>,
    pub render_cache: MessageRenderCache,
}

impl ChatMessage {
    #[must_use]
    pub fn new(role: MessageRole, blocks: Vec<MessageBlock>, usage: Option<MessageUsage>) -> Self {
        Self { role, blocks, usage, render_cache: MessageRenderCache::default() }
    }

    #[must_use]
    pub fn welcome(version: &str, subscription: &str, cwd: &str, session_id: &str) -> Self {
        Self::new(
            MessageRole::Welcome,
            vec![MessageBlock::Welcome(WelcomeBlock {
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

    pub fn invalidate_render_cache(&mut self) {
        self.render_cache.invalidate();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRenderCacheKey {
    pub width: u16,
    pub layout_generation: u64,
    pub tools_collapsed: bool,
    pub include_trailing_separator: bool,
    pub render_signature: MessageRenderSignature,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRenderSignature {
    pub role: MessageRole,
    pub show_empty_thinking: bool,
    pub show_thinking: bool,
    pub show_compacting: bool,
    pub assistant_frame: Option<usize>,
    pub blocks: Vec<MessageBlockRenderSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageBlockRenderSignature {
    Text {
        text_hash: u64,
        trailing_spacing: TextBlockSpacing,
    },
    Notice {
        severity: SystemSeverity,
        text_hash: u64,
        trailing_spacing: TextBlockSpacing,
    },
    ToolCall {
        render_epoch: u64,
        layout_epoch: u64,
        hidden: bool,
        status: crate::agent::model::ToolCallStatus,
        sdk_tool_name: String,
        pending_permission: bool,
        pending_question: bool,
        frame: Option<usize>,
    },
    Welcome {
        content_hash: u64,
    },
    ImageAttachment {
        count: usize,
    },
}

#[derive(Default)]
pub struct MessageRenderCache {
    key: Option<MessageRenderCacheKey>,
    segments: Vec<CachedMessageSegment>,
    cached_bytes: usize,
    height: usize,
    wrapped_lines: usize,
    last_access_tick: Cell<u64>,
}

#[derive(Clone)]
pub enum CachedMessageSegment {
    Blank,
    Lines { lines: Vec<Line<'static>>, height: usize },
}

impl MessageRenderCache {
    fn touch(&self) {
        self.last_access_tick.set(super::block_cache::next_cache_access_tick());
    }

    #[must_use]
    pub fn matches(&self, key: &MessageRenderCacheKey) -> bool {
        self.key.as_ref() == Some(key)
    }

    #[must_use]
    pub fn segments(&self) -> &[CachedMessageSegment] {
        self.touch();
        &self.segments
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.touch();
        self.height
    }

    #[must_use]
    pub fn wrapped_lines(&self) -> usize {
        self.touch();
        self.wrapped_lines
    }

    #[must_use]
    pub fn cached_bytes(&self) -> usize {
        self.cached_bytes
    }

    #[must_use]
    pub fn last_access_tick(&self) -> u64 {
        self.last_access_tick.get()
    }

    pub fn store(
        &mut self,
        key: MessageRenderCacheKey,
        segments: Vec<CachedMessageSegment>,
        height: usize,
        wrapped_lines: usize,
    ) {
        let cached_bytes = segments.iter().map(CachedMessageSegment::cached_bytes).sum();
        self.key = Some(key);
        self.segments = segments;
        self.cached_bytes = cached_bytes;
        self.height = height;
        self.wrapped_lines = wrapped_lines;
        self.touch();
    }

    pub fn invalidate(&mut self) {
        self.key = None;
        self.segments.clear();
        self.cached_bytes = 0;
        self.height = 0;
        self.wrapped_lines = 0;
    }

    pub fn evict_cached_render(&mut self) -> usize {
        let removed = self.cached_bytes;
        if removed == 0 {
            return 0;
        }
        self.invalidate();
        removed
    }
}

impl CachedMessageSegment {
    #[must_use]
    fn cached_bytes(&self) -> usize {
        match self {
            Self::Blank => 1,
            Self::Lines { lines, .. } => lines.iter().map(line_utf8_bytes).sum(),
        }
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

fn line_utf8_bytes(line: &Line<'static>) -> usize {
    let span_bytes =
        line.spans.iter().fold(0usize, |acc, span| acc.saturating_add(span.content.len()));
    span_bytes.saturating_add(1)
}

/// Text holder for a single message block's markdown source.
///
/// Block splitting for streaming text is handled at the message construction
/// level. Within a block, this type keeps stable paragraph-sized prefixes cached
/// so only the active tail needs to be re-rendered while streaming continues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MarkdownRenderKey {
    pub width: u16,
    pub bg: Option<Color>,
    pub preserve_newlines: bool,
}

#[derive(Default)]
struct MarkdownChunk {
    range: Range<usize>,
    rendered: Option<Vec<Line<'static>>>,
    render_key: Option<MarkdownRenderKey>,
    dirty: bool,
}

impl MarkdownChunk {
    fn new(range: Range<usize>) -> Self {
        Self { range, rendered: None, render_key: None, dirty: true }
    }
}

#[derive(Default)]
pub struct IncrementalMarkdown {
    text: String,
    chunks: Vec<MarkdownChunk>,
}

impl IncrementalMarkdown {
    /// Create from existing full text (e.g. user messages, connection errors).
    /// Treats the entire text as one block source.
    #[must_use]
    pub fn from_complete(text: &str) -> Self {
        let mut markdown = Self::default();
        markdown.append(text);
        markdown
    }

    /// Append a streaming text chunk.
    pub fn append(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        self.text.push_str(chunk);
        if let Some(last) = self.chunks.last_mut() {
            last.range.end = self.text.len();
            last.dirty = true;
            last.rendered = None;
            last.render_key = None;
        } else {
            self.chunks.push(MarkdownChunk::new(0..self.text.len()));
        }
        self.split_tail_chunks();
    }

    /// Get the full source text.
    #[must_use]
    pub fn full_text(&self) -> String {
        self.text.clone()
    }

    /// Allocated capacity of the internal text buffer in bytes.
    #[must_use]
    pub fn text_capacity(&self) -> usize {
        self.text.capacity()
    }

    /// Render this block source via the provided markdown renderer.
    /// `render_fn` converts a markdown source string into `Vec<Line>`.
    pub(crate) fn lines(
        &mut self,
        render_key: MarkdownRenderKey,
        render_fn: &impl Fn(&str) -> Vec<Line<'static>>,
    ) -> Vec<Line<'static>> {
        self.ensure_rendered(render_key, render_fn);

        let mut rendered = Vec::new();
        for chunk in &self.chunks {
            if let Some(lines) = &chunk.rendered {
                rendered.extend(lines.iter().cloned());
            }
        }
        rendered
    }

    pub fn invalidate_renders(&mut self) {
        for chunk in &mut self.chunks {
            chunk.dirty = true;
            chunk.rendered = None;
            chunk.render_key = None;
        }
    }

    pub(crate) fn ensure_rendered(
        &mut self,
        render_key: MarkdownRenderKey,
        render_fn: &impl Fn(&str) -> Vec<Line<'static>>,
    ) {
        for idx in 0..self.chunks.len() {
            let needs_render = {
                let chunk = &self.chunks[idx];
                chunk.dirty || chunk.rendered.is_none() || chunk.render_key != Some(render_key)
            };
            if !needs_render {
                continue;
            }

            let range = self.chunks[idx].range.clone();
            let rendered = render_fn(&self.text[range]);
            let chunk = &mut self.chunks[idx];
            chunk.rendered = Some(rendered);
            chunk.render_key = Some(render_key);
            chunk.dirty = false;
        }
    }

    fn split_tail_chunks(&mut self) {
        loop {
            let Some(last_idx) = self.chunks.len().checked_sub(1) else {
                break;
            };
            let range = self.chunks[last_idx].range.clone();
            let Some(split_at_rel) = find_first_stable_split(&self.text[range.clone()]) else {
                break;
            };
            let split_at = range.start + split_at_rel;
            if split_at <= range.start || split_at >= range.end {
                break;
            }

            self.chunks[last_idx] = MarkdownChunk::new(range.start..split_at);
            self.chunks.push(MarkdownChunk::new(split_at..range.end));
        }
    }
}

fn find_first_stable_split(text: &str) -> Option<usize> {
    let mut in_fenced_code = false;
    let mut saw_nonblank = false;
    let mut blank_run_end = None;
    let mut offset = 0usize;

    for line in text.split_inclusive('\n') {
        offset += line.len();
        let trimmed = line.trim_end_matches('\n').trim();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if is_fence {
            in_fenced_code = !in_fenced_code;
        }

        let is_blank = trimmed.is_empty();
        if !in_fenced_code && is_blank {
            if saw_nonblank {
                blank_run_end = Some(offset);
            }
            continue;
        }

        if let Some(boundary) = blank_run_end.take()
            && boundary < text.len()
        {
            return Some(boundary);
        }

        if !is_blank {
            saw_nonblank = true;
        }
    }

    None
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
    pub text: String,
    pub cache: BlockCache,
    pub markdown: IncrementalMarkdown,
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
        Self {
            markdown: IncrementalMarkdown::from_complete(&text),
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
    pub resets_at_bucket: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NoticeDedupKey {
    RateLimit(RateLimitIncidentKey),
    ApiRetry,
}

pub struct NoticeBlock {
    pub severity: SystemSeverity,
    pub text: TextBlock,
    pub dedup_key: Option<NoticeDedupKey>,
}

impl NoticeBlock {
    #[must_use]
    pub fn new(severity: SystemSeverity, text: String) -> Self {
        Self { severity, text: TextBlock::new(text), dedup_key: None }
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
    Notice(NoticeBlock),
    ToolCall(Box<ToolCallInfo>),
    Welcome(WelcomeBlock),
    /// Indicates N images were attached to this user message.
    ImageAttachment(ImageAttachmentBlock),
}

/// Lightweight block for image attachment indicators. Carries a [`BlockCache`]
/// to satisfy the render-budget invariant that every [`MessageBlock`] variant
/// has a cache, even though the cached content is trivially small.
pub struct ImageAttachmentBlock {
    pub count: usize,
    pub cache: BlockCache,
}

impl ImageAttachmentBlock {
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self { count, cache: BlockCache::default() }
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
    pub version: String,
    pub subscription: String,
    pub cwd: String,
    pub session_id: String,
    pub tip_seed: u64,
    pub cache: BlockCache,
}
