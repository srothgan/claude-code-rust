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

/// Single owner of all chat layout state: scroll, per-message heights, and prefix sums.
///
/// Consolidates state previously scattered across `App` (scroll fields, prefix sums),
/// `ChatMessage` (`cached_visual_height`/`cached_visual_width`), and `BlockCache` (`wrapped_height`/`wrapped_width`).
/// Per-block heights remain on `BlockCache` via `set_height()` / `height_at()`, but
/// the viewport owns the validity width that governs whether those caches are considered
/// current. On resize, `on_frame()` zeroes message heights and clears prefix sums,
/// causing the next `update_visual_heights()` pass to re-measure every message
/// using ground-truth `Paragraph::line_count()`.
pub struct ChatViewport {
    // --- Scroll ---
    /// Rendered scroll offset (rounded from `scroll_pos`).
    pub scroll_offset: usize,
    /// Target scroll offset requested by user input or auto-scroll.
    pub scroll_target: usize,
    /// Smooth scroll position (fractional) for animation.
    pub scroll_pos: f32,
    /// Smoothed scrollbar thumb top row (fractional) for animation.
    pub scrollbar_thumb_top: f32,
    /// Smoothed scrollbar thumb height (fractional) for animation.
    pub scrollbar_thumb_size: f32,
    /// Whether to auto-scroll to bottom on new content.
    pub auto_scroll: bool,

    // --- Layout ---
    /// Current terminal width. Set by `on_frame()` each render cycle.
    pub width: u16,
    /// Monotonic layout generation for width/global layout-affecting changes.
    /// Tool-call measurement cache keys include this to avoid stale heights.
    pub layout_generation: u64,

    // --- Per-message heights ---
    /// Visual height (in terminal rows) of each message, indexed by message position.
    /// Zeroed on resize; rebuilt by `measure_message_height()`.
    pub message_heights: Vec<usize>,
    /// Width at which `message_heights` was last computed.
    pub message_heights_width: u16,
    /// Oldest message index whose cached height may be stale.
    pub dirty_from: Option<usize>,

    // --- Prefix sums ---
    /// Cumulative heights: `height_prefix_sums[i]` = sum of heights `0..=i`.
    /// Enables O(log n) binary search for first visible message and O(1) total height.
    pub height_prefix_sums: Vec<usize>,
    /// Width at which prefix sums were last computed.
    pub prefix_sums_width: u16,
}

impl ChatViewport {
    /// Create a new viewport with default scroll state (auto-scroll enabled).
    #[must_use]
    pub fn new() -> Self {
        Self {
            scroll_offset: 0,
            scroll_target: 0,
            scroll_pos: 0.0,
            scrollbar_thumb_top: 0.0,
            scrollbar_thumb_size: 0.0,
            auto_scroll: true,
            width: 0,
            layout_generation: 1,
            message_heights: Vec::new(),
            message_heights_width: 0,
            dirty_from: None,
            height_prefix_sums: Vec::new(),
            prefix_sums_width: 0,
        }
    }

    /// Called at top of each render frame. Detects width change and invalidates
    /// all cached heights so they get re-measured at the new width.
    pub fn on_frame(&mut self, width: u16) {
        if self.width != 0 && self.width != width {
            tracing::debug!(
                "RESIZE: width {} -> {}, scroll_target={}, auto_scroll={}",
                self.width,
                width,
                self.scroll_target,
                self.auto_scroll
            );
            self.handle_resize();
        }
        self.width = width;
    }

    /// Invalidate height caches on terminal resize.
    ///
    /// Setting `message_heights_width = 0` forces `update_visual_heights()`
    /// to re-measure every message at the new width using ground-truth
    /// `line_count()`. Old message heights are kept as approximations so
    /// `content_height` stays reasonable on the resize frame.
    fn handle_resize(&mut self) {
        self.message_heights_width = 0;
        self.prefix_sums_width = 0;
        self.layout_generation = self.layout_generation.wrapping_add(1);
    }

    /// Bump layout generation for non-width global layout-affecting changes.
    pub fn bump_layout_generation(&mut self) {
        self.layout_generation = self.layout_generation.wrapping_add(1);
    }

    // --- Per-message height ---

    /// Get the cached visual height for message `idx`. Returns 0 if not yet computed.
    #[must_use]
    pub fn message_height(&self, idx: usize) -> usize {
        self.message_heights.get(idx).copied().unwrap_or(0)
    }

    /// Set the visual height for message `idx`, growing the vec if needed.
    ///
    /// Does NOT update `message_heights_width` - the caller must call
    /// `mark_heights_valid()` after the full re-measurement pass completes.
    pub fn set_message_height(&mut self, idx: usize, h: usize) {
        if idx >= self.message_heights.len() {
            self.message_heights.resize(idx + 1, 0);
        }
        self.message_heights[idx] = h;
    }

    /// Mark all message heights as valid at the current width.
    /// Call after `update_visual_heights()` finishes re-measuring.
    pub fn mark_heights_valid(&mut self) {
        self.message_heights_width = self.width;
        self.dirty_from = None;
    }

    /// Mark cached heights dirty from `idx` onward.
    pub fn mark_message_dirty(&mut self, idx: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(idx, |oldest| oldest.min(idx)));
    }

    // --- Prefix sums ---

    /// Rebuild prefix sums from `message_heights`.
    /// O(1) fast path when width unchanged and only the last message changed (streaming).
    pub fn rebuild_prefix_sums(&mut self) {
        let n = self.message_heights.len();
        if self.prefix_sums_width == self.width && self.height_prefix_sums.len() == n && n > 0 {
            // Streaming fast path: only last message's height changed.
            let prev = if n >= 2 { self.height_prefix_sums[n - 2] } else { 0 };
            self.height_prefix_sums[n - 1] = prev + self.message_heights[n - 1];
            return;
        }
        // Full rebuild (resize or new messages added)
        self.height_prefix_sums.clear();
        self.height_prefix_sums.reserve(n);
        let mut acc = 0;
        for &h in &self.message_heights {
            acc += h;
            self.height_prefix_sums.push(acc);
        }
        self.prefix_sums_width = self.width;
    }

    /// Total height of all messages (O(1) via prefix sums).
    #[must_use]
    pub fn total_message_height(&self) -> usize {
        self.height_prefix_sums.last().copied().unwrap_or(0)
    }

    /// Cumulative height of messages `0..idx` (O(1) via prefix sums).
    #[must_use]
    pub fn cumulative_height_before(&self, idx: usize) -> usize {
        if idx == 0 { 0 } else { self.height_prefix_sums.get(idx - 1).copied().unwrap_or(0) }
    }

    /// Binary search for the first message whose cumulative range overlaps `scroll_offset`.
    #[must_use]
    pub fn find_first_visible(&self, scroll_offset: usize) -> usize {
        if self.height_prefix_sums.is_empty() {
            return 0;
        }
        self.height_prefix_sums
            .partition_point(|&h| h <= scroll_offset)
            .min(self.message_heights.len().saturating_sub(1))
    }

    // --- Scroll ---

    /// Scroll up by `lines`. Disables auto-scroll.
    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_target = self.scroll_target.saturating_sub(lines);
        self.auto_scroll = false;
    }

    /// Scroll down by `lines`. Auto-scroll re-engagement handled by render.
    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_target = self.scroll_target.saturating_add(lines);
    }

    /// Re-engage auto-scroll (stick to bottom).
    pub fn engage_auto_scroll(&mut self) {
        self.auto_scroll = true;
    }
}

impl Default for ChatViewport {
    fn default() -> Self {
        Self::new()
    }
}
