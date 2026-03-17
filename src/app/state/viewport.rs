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

/// Describes the intent behind a layout invalidation.
///
/// All variants currently reduce to the same `dirty_from` watermark model --
/// the semantic distinction exists for documentation, tracing, and future
/// optimization (e.g. O(1) prefix-sum patch for `Single` at non-tail indices).
///
/// Do NOT add `Range(start, end)` unless the underlying data structures
/// support bounded invalidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidationLevel {
    /// One message's content changed (tool status, permission UI, terminal output).
    /// Only that message needs re-measurement.
    Single(usize),
    /// Messages from `start` onward may have changed (insert/remove/reindex).
    From(usize),
    /// Terminal width changed. Handled internally by `on_frame()`.
    /// Included for completeness; not dispatched through `App::invalidate_layout()`.
    Resize,
    /// Global layout change (e.g. tool collapse toggle).
    /// All messages dirty + `layout_generation` bumped.
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeRemeasureState {
    scroll_anchor_index: usize,
    scroll_anchor_offset: usize,
    priority_start: usize,
    priority_end: usize,
    next_above: Option<usize>,
    next_below: usize,
    prefer_above: bool,
}

impl ResizeRemeasureState {
    fn new(
        scroll_anchor_index: usize,
        scroll_anchor_offset: usize,
        priority_start: usize,
        priority_end: usize,
        message_count: usize,
    ) -> Self {
        let last_idx = message_count.saturating_sub(1);
        let scroll_anchor_index = scroll_anchor_index.min(last_idx);
        let priority_start = priority_start.min(last_idx);
        let priority_end = priority_end.min(last_idx).max(priority_start);
        Self {
            scroll_anchor_index,
            scroll_anchor_offset,
            priority_start,
            priority_end,
            next_above: priority_start.checked_sub(1),
            next_below: priority_end.saturating_add(1).min(message_count),
            prefer_above: false,
        }
    }

    fn from_scroll_anchor(
        scroll_anchor_index: usize,
        scroll_anchor_offset: usize,
        message_count: usize,
    ) -> Self {
        Self::new(
            scroll_anchor_index,
            scroll_anchor_offset,
            scroll_anchor_index,
            scroll_anchor_index,
            message_count,
        )
    }
}

/// Single owner of all chat layout state: scroll, per-message heights, and prefix sums.
///
/// Consolidates state previously scattered across `App` (scroll fields, prefix sums),
/// `ChatMessage` (`cached_visual_height`/`cached_visual_width`), and `BlockCache` (`wrapped_height`/`wrapped_width`).
/// Per-block heights remain on `BlockCache` via `set_height()` / `height_at()`, but
/// the viewport owns the validity width that governs whether those caches are considered
/// current. On resize, prior heights are retained as temporary estimates while a
/// resumable re-measurement pass converges back to exact heights at the new width.
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
    /// Retained across resize as an estimate until re-measured at the new width.
    pub message_heights: Vec<usize>,
    /// Width at which `message_heights` was last computed.
    /// Set only when every cached message height is exact at `width`.
    pub message_heights_width: u16,
    /// Per-message exactness marker used while resize re-measurement is in flight.
    pub measured_message_widths: Vec<u16>,
    /// Oldest message index whose cached height may be stale.
    pub dirty_from: Option<usize>,
    /// Resumable frontiers for progressive resize re-measurement.
    pub resize_remeasure: Option<ResizeRemeasureState>,

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
            measured_message_widths: Vec::new(),
            dirty_from: None,
            resize_remeasure: None,
            height_prefix_sums: Vec::new(),
            prefix_sums_width: 0,
        }
    }

    /// Called at top of each render frame. Detects width change and invalidates
    /// all cached heights so they get re-measured at the new width.
    ///
    /// Returns `true` if a resize was detected (width changed).
    pub fn on_frame(&mut self, width: u16) -> bool {
        let resized = self.width != 0 && self.width != width;
        if resized {
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
        resized
    }

    /// Invalidate height caches on terminal resize.
    ///
    /// Old message heights remain as approximations so the next frame can keep
    /// using a stable prefix-sum model while resize re-measurement converges.
    fn handle_resize(&mut self) {
        self.message_heights_width = 0;
        self.prefix_sums_width = 0;
        self.resize_remeasure = (!self.message_heights.is_empty()).then(|| {
            let (anchor_index, anchor_offset) = self.current_scroll_anchor();
            ResizeRemeasureState::from_scroll_anchor(
                anchor_index,
                anchor_offset,
                self.message_heights.len(),
            )
        });
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

    /// Ensure per-message height state matches the current message count.
    pub fn sync_message_count(&mut self, count: usize) {
        let heights_len_before = self.message_heights.len();
        if heights_len_before != count || self.height_prefix_sums.len() != count {
            self.prefix_sums_width = 0;
        }
        self.message_heights.resize(count, 0);
        self.measured_message_widths.resize(count, 0);
        self.height_prefix_sums.truncate(count);
        if count == 0 {
            self.dirty_from = None;
            self.resize_remeasure = None;
            return;
        }
        if let Some(state) = self.resize_remeasure
            && (state.scroll_anchor_index >= count
                || state.priority_start >= count
                || state.priority_end >= count
                || state.next_below > count)
        {
            self.resize_remeasure = Some(ResizeRemeasureState::new(
                state.scroll_anchor_index.min(count.saturating_sub(1)),
                state.scroll_anchor_offset,
                state.priority_start.min(count.saturating_sub(1)),
                state.priority_end.min(count.saturating_sub(1)),
                count,
            ));
        }
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

    /// Mark one message height as exact for the current viewport width.
    pub fn mark_message_height_measured(&mut self, idx: usize) {
        if idx >= self.measured_message_widths.len() {
            self.measured_message_widths.resize(idx + 1, 0);
        }
        self.measured_message_widths[idx] = self.width;
    }

    /// Return whether a message height is exact at the current width.
    #[must_use]
    pub fn message_height_is_current(&self, idx: usize) -> bool {
        if self.message_heights_width == self.width {
            return idx < self.message_heights.len();
        }
        self.measured_message_widths.get(idx).copied().unwrap_or(0) == self.width
    }

    /// Mark all message heights as valid at the current width.
    /// Call after `update_visual_heights()` finishes re-measuring.
    pub fn mark_heights_valid(&mut self) {
        self.message_heights_width = self.width;
        self.dirty_from = None;
        self.measured_message_widths.fill(self.width);
        self.resize_remeasure = None;
    }

    /// Mark cached heights dirty from `idx` onward.
    pub fn mark_message_dirty(&mut self, idx: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(idx, |oldest| oldest.min(idx)));
        if idx < self.measured_message_widths.len() {
            self.measured_message_widths[idx..].fill(0);
        }
    }

    /// Return whether progressive resize re-measurement is active.
    #[must_use]
    pub fn resize_remeasure_active(&self) -> bool {
        self.resize_remeasure.is_some()
    }

    /// Reset the outward expansion frontiers around the current visible window.
    pub fn ensure_resize_remeasure_anchor(
        &mut self,
        visible_start: usize,
        visible_end: usize,
        message_count: usize,
    ) {
        if message_count == 0 || self.resize_remeasure.is_none() {
            return;
        }
        let Some(state) = self.resize_remeasure else {
            return;
        };
        let next = ResizeRemeasureState::new(
            state.scroll_anchor_index,
            state.scroll_anchor_offset,
            visible_start,
            visible_end,
            message_count,
        );
        let needs_reanchor = self.resize_remeasure.is_some_and(|state| {
            state.priority_start != next.priority_start || state.priority_end != next.priority_end
        });
        if needs_reanchor {
            self.resize_remeasure = Some(next);
        }
    }

    /// Resume outward resize re-measurement from the current visible anchor.
    pub fn next_resize_remeasure_index(&mut self, message_count: usize) -> Option<usize> {
        let state = self.resize_remeasure.as_mut()?;
        let choose_above = match (state.next_above, state.next_below < message_count) {
            (Some(_), true) => {
                let choose = state.prefer_above;
                state.prefer_above = !state.prefer_above;
                choose
            }
            (Some(_), false) => true,
            (None, true) => false,
            (None, false) => {
                self.resize_remeasure = None;
                return None;
            }
        };
        if choose_above {
            let idx = state.next_above?;
            state.next_above = idx.checked_sub(1);
            Some(idx)
        } else {
            let idx = state.next_below;
            state.next_below = state.next_below.saturating_add(1);
            Some(idx)
        }
    }

    /// Return the preserved pre-resize scroll anchor.
    #[must_use]
    pub fn resize_scroll_anchor(&self) -> Option<(usize, usize)> {
        self.resize_remeasure.map(|state| (state.scroll_anchor_index, state.scroll_anchor_offset))
    }

    /// Derive the priority window from the preserved scroll anchor using current estimates.
    #[must_use]
    pub fn resize_anchor_window(&self, viewport_height: usize) -> Option<(usize, usize)> {
        let state = self.resize_remeasure?;
        if self.message_heights.is_empty() {
            return None;
        }
        let start = state.scroll_anchor_index.min(self.message_heights.len().saturating_sub(1));
        let mut end = start;
        let needed_rows = state.scroll_anchor_offset.saturating_add(viewport_height.max(1));
        let mut covered_rows = self.message_height(start);
        while end + 1 < self.message_heights.len() && covered_rows < needed_rows {
            end += 1;
            covered_rows = covered_rows.saturating_add(self.message_height(end));
        }
        Some((start, end))
    }

    /// Restore the absolute scroll position from a preserved message-local anchor.
    #[allow(clippy::cast_precision_loss)]
    pub fn restore_scroll_anchor(&mut self, anchor_index: usize, anchor_offset: usize) {
        if self.auto_scroll || self.message_heights.is_empty() {
            return;
        }
        let anchor_index = anchor_index.min(self.message_heights.len().saturating_sub(1));
        let anchor_height = self.message_height(anchor_index);
        let clamped_offset =
            if anchor_height == 0 { 0 } else { anchor_offset.min(anchor_height.saturating_sub(1)) };
        let scroll = self.cumulative_height_before(anchor_index).saturating_add(clamped_offset);
        self.scroll_target = scroll;
        self.scroll_pos = scroll as f32;
        self.scroll_offset = scroll;
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

    /// Binary search for the last message whose cumulative range overlaps the viewport.
    #[must_use]
    pub fn find_last_visible(&self, scroll_offset: usize, viewport_height: usize) -> usize {
        if self.height_prefix_sums.is_empty() {
            return 0;
        }
        let visible_end = scroll_offset.saturating_add(viewport_height);
        self.height_prefix_sums
            .partition_point(|&h| h < visible_end)
            .min(self.message_heights.len().saturating_sub(1))
    }

    fn current_scroll_anchor(&self) -> (usize, usize) {
        if self.message_heights.is_empty() {
            return (0, 0);
        }
        let first_visible = self.find_first_visible_in_estimates(self.scroll_offset);
        let offset_in_message = self
            .scroll_offset
            .saturating_sub(self.cumulative_height_before_in_estimates(first_visible));
        (first_visible, offset_in_message)
    }

    fn find_first_visible_in_estimates(&self, scroll_offset: usize) -> usize {
        let mut acc = 0usize;
        for (idx, &height) in self.message_heights.iter().enumerate() {
            acc = acc.saturating_add(height);
            if acc > scroll_offset {
                return idx;
            }
        }
        self.message_heights.len().saturating_sub(1)
    }

    fn cumulative_height_before_in_estimates(&self, idx: usize) -> usize {
        self.message_heights.iter().take(idx).copied().sum()
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
