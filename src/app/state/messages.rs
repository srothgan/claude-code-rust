// Claude Code Rust - A native Rust terminal interface for Claude Code
// Copyright (C) 2025  Simon Peter Rothgang
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

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

/// Ordered content block - text and tool calls interleaved as they arrive.
pub enum MessageBlock {
    Text(String, BlockCache, IncrementalMarkdown),
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
