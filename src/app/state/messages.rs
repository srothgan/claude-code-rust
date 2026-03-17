// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use super::block_cache::BlockCache;
use super::tool_call_info::ToolCallInfo;
use super::types::{MessageUsage, RecentSessionInfo};

pub struct ChatMessage {
    pub role: MessageRole,
    pub blocks: Vec<MessageBlock>,
    pub usage: Option<MessageUsage>,
}

impl ChatMessage {
    #[must_use]
    pub fn welcome(model_name: &str, cwd: &str) -> Self {
        Self::welcome_with_recent(model_name, cwd, &[])
    }

    #[must_use]
    pub fn welcome_with_recent(
        model_name: &str,
        cwd: &str,
        recent_sessions: &[RecentSessionInfo],
    ) -> Self {
        Self {
            role: MessageRole::Welcome,
            blocks: vec![MessageBlock::Welcome(WelcomeBlock {
                model_name: model_name.to_owned(),
                cwd: cwd.to_owned(),
                recent_sessions: recent_sessions.to_vec(),
                cache: BlockCache::default(),
            })],
            usage: None,
        }
    }
}

/// Text holder for a single message block's markdown source.
///
/// Block splitting for streaming text is handled at the message construction
/// level. This type intentionally does no internal splitting.
#[derive(Default)]
pub struct IncrementalMarkdown {
    text: String,
}

impl IncrementalMarkdown {
    /// Create from existing full text (e.g. user messages, connection errors).
    /// Treats the entire text as one block source.
    #[must_use]
    pub fn from_complete(text: &str) -> Self {
        Self { text: text.to_owned() }
    }

    /// Append a streaming text chunk.
    pub fn append(&mut self, chunk: &str) {
        self.text.push_str(chunk);
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
    pub fn lines(
        &mut self,
        render_fn: &impl Fn(&str) -> Vec<ratatui::text::Line<'static>>,
    ) -> Vec<ratatui::text::Line<'static>> {
        render_fn(&self.text)
    }

    /// No-op: markdown render caching lives at `BlockCache` level.
    pub fn invalidate_renders(&mut self) {
        let _ = self.text.len();
    }

    /// No-op: markdown render caching lives at `BlockCache` level.
    pub fn ensure_rendered(
        &mut self,
        _render_fn: &impl Fn(&str) -> Vec<ratatui::text::Line<'static>>,
    ) {
        let _ = self.text.len();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

/// Ordered content block - text and tool calls interleaved as they arrive.
pub enum MessageBlock {
    Text(TextBlock),
    ToolCall(Box<ToolCallInfo>),
    Welcome(WelcomeBlock),
}

#[derive(Debug)]
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
    pub model_name: String,
    pub cwd: String,
    pub recent_sessions: Vec<RecentSessionInfo>,
    pub cache: BlockCache,
}
