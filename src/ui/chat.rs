// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::cache_metrics;
use crate::app::{App, AppStatus, MessageBlock, MessageRole, SelectionKind, SelectionState};
use crate::ui::message::{self, SpinnerState};
use crate::ui::theme;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use std::time::Instant;

/// Minimum number of messages to render above/below the visible range as a margin.
/// Heights are now exact (block-level wrapped heights), so no safety margin is needed.
const CULLING_MARGIN: usize = 0;
const SCROLLBAR_MIN_THUMB_HEIGHT: usize = 1;
const SCROLLBAR_TOP_EASE: f32 = 0.35;
const SCROLLBAR_SIZE_EASE: f32 = 0.2;
const SCROLLBAR_EASE_EPSILON: f32 = 0.01;
const OVERSCROLL_CLAMP_EASE: f32 = 0.2;

#[derive(Clone, Copy, Default)]
struct HeightUpdateStats {
    measured_msgs: usize,
    measured_lines: usize,
    reused_msgs: usize,
}

#[derive(Clone, Copy, Default)]
struct ResizeMeasureBudget {
    remaining_msgs: usize,
    remaining_lines: usize,
}

impl ResizeMeasureBudget {
    fn new(viewport_height: usize) -> Self {
        let viewport_floor = viewport_height.max(12);
        Self {
            remaining_msgs: viewport_floor,
            remaining_lines: viewport_floor.saturating_mul(8).max(256),
        }
    }

    fn exhausted(self) -> bool {
        self.remaining_msgs == 0 || self.remaining_lines == 0
    }

    fn consume(&mut self, wrapped_lines: usize) {
        self.remaining_msgs = self.remaining_msgs.saturating_sub(1);
        self.remaining_lines = self.remaining_lines.saturating_sub(wrapped_lines.max(1));
    }
}

#[derive(Clone, Copy, Default)]
struct CulledRenderStats {
    local_scroll: usize,
    first_visible: usize,
    render_start: usize,
    rendered_msgs: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScrollbarGeometry {
    thumb_top: usize,
    thumb_size: usize,
}
/// Build a `SpinnerState` for a specific message index.
fn msg_spinner(
    base: SpinnerState,
    index: usize,
    msg_count: usize,
    is_thinking: bool,
    show_subagent_thinking: bool,
    msg: &crate::app::ChatMessage,
) -> SpinnerState {
    let is_last = index + 1 == msg_count;
    let is_assistant = matches!(msg.role, MessageRole::Assistant);
    let mid_turn = is_last && is_thinking && is_assistant && !msg.blocks.is_empty();
    let subagent = is_last
        && is_assistant
        && base.is_active
        && show_subagent_thinking
        && !msg.blocks.is_empty();
    SpinnerState {
        is_last_message: is_last,
        is_thinking_mid_turn: mid_turn,
        is_subagent_thinking: subagent,
        ..base
    }
}

/// Ensure every message has an up-to-date height in the viewport at the given width.
/// The last message is always recomputed while streaming (content changes each frame).
///
/// Height is ground truth: each message is rendered into a scratch buffer via
/// `render_message()` and measured with `Paragraph::line_count(width)`. This uses
/// the exact same wrapping algorithm as the actual render path, so heights can
/// never drift from reality.
///
/// Iterates in reverse so we can break early: once we hit a message whose height
/// is already valid at this width, all earlier messages are also valid (content
/// only changes at the tail during streaming). This turns the common case from
/// O(n) to O(1).
fn update_visual_heights(
    app: &mut App,
    base: SpinnerState,
    is_thinking: bool,
    show_subagent_thinking: bool,
    width: u16,
    viewport_height: usize,
) -> HeightUpdateStats {
    let _t =
        app.perf.as_ref().map(|p| p.start_with("chat::update_heights", "msgs", app.messages.len()));
    app.viewport.sync_message_count(app.messages.len());

    if app.viewport.resize_remeasure_active() {
        return update_visual_heights_resize(
            app,
            base,
            is_thinking,
            show_subagent_thinking,
            width,
            viewport_height,
        );
    }

    update_visual_heights_steady(app, base, is_thinking, show_subagent_thinking, width)
}

fn update_visual_heights_steady(
    app: &mut App,
    base: SpinnerState,
    is_thinking: bool,
    show_subagent_thinking: bool,
    width: u16,
) -> HeightUpdateStats {
    let msg_count = app.messages.len();
    let is_streaming = matches!(app.status, AppStatus::Thinking | AppStatus::Running);
    let width_valid = app.viewport.message_heights_width == width;
    let dirty_from = app.viewport.dirty_from.filter(|&idx| idx < msg_count);
    let mut stats = HeightUpdateStats::default();
    for i in (0..msg_count).rev() {
        let is_last = i + 1 == msg_count;
        let is_dirty = dirty_from.is_some_and(|idx| i >= idx);
        if !is_dirty
            && width_valid
            && app.viewport.message_height(i) > 0
            && !(is_last && is_streaming)
        {
            stats.reused_msgs = i + 1;
            break;
        }
        measure_message_height_at(
            app,
            base,
            is_thinking,
            show_subagent_thinking,
            width,
            i,
            &mut stats,
        );
    }
    app.viewport.mark_heights_valid();
    stats
}

fn update_visual_heights_resize(
    app: &mut App,
    base: SpinnerState,
    is_thinking: bool,
    show_subagent_thinking: bool,
    width: u16,
    viewport_height: usize,
) -> HeightUpdateStats {
    let msg_count = app.messages.len();
    let is_streaming = matches!(app.status, AppStatus::Thinking | AppStatus::Running);
    let mut stats = HeightUpdateStats::default();

    if msg_count == 0 {
        app.viewport.mark_heights_valid();
        return stats;
    }

    let (visible_start, visible_end) =
        app.viewport.resize_anchor_window(viewport_height).unwrap_or((0, 0));
    app.viewport.ensure_resize_remeasure_anchor(visible_start, visible_end, msg_count);

    for i in visible_start..=visible_end {
        let is_last = i + 1 == msg_count;
        if !needs_height_measure(app, i, is_last, is_streaming) {
            stats.reused_msgs += 1;
            continue;
        }
        measure_message_height_at(
            app,
            base,
            is_thinking,
            show_subagent_thinking,
            width,
            i,
            &mut stats,
        );
    }

    if is_streaming {
        let last = msg_count.saturating_sub(1);
        if needs_height_measure(app, last, true, true) {
            measure_message_height_at(
                app,
                base,
                is_thinking,
                show_subagent_thinking,
                width,
                last,
                &mut stats,
            );
        }
    }

    let mut budget = ResizeMeasureBudget::new(viewport_height);
    while !budget.exhausted() {
        let Some(i) = app.viewport.next_resize_remeasure_index(msg_count) else {
            break;
        };
        if (visible_start..=visible_end).contains(&i) {
            continue;
        }
        let is_last = i + 1 == msg_count;
        if !needs_height_measure(app, i, is_last, is_streaming) {
            stats.reused_msgs += 1;
            continue;
        }
        let measured_lines_before = stats.measured_lines;
        measure_message_height_at(
            app,
            base,
            is_thinking,
            show_subagent_thinking,
            width,
            i,
            &mut stats,
        );
        budget.consume(stats.measured_lines.saturating_sub(measured_lines_before));
    }

    if !app.viewport.resize_remeasure_active() {
        app.viewport.mark_heights_valid();
    }
    stats
}

fn needs_height_measure(app: &App, idx: usize, is_last: bool, is_streaming: bool) -> bool {
    (is_last && is_streaming) || !app.viewport.message_height_is_current(idx)
}

fn measure_message_height_at(
    app: &mut App,
    base: SpinnerState,
    is_thinking: bool,
    show_subagent_thinking: bool,
    width: u16,
    idx: usize,
    stats: &mut HeightUpdateStats,
) {
    let msg_count = app.messages.len();
    let sp =
        msg_spinner(base, idx, msg_count, is_thinking, show_subagent_thinking, &app.messages[idx]);
    let (h, rendered_lines) =
        measure_message_height(&mut app.messages[idx], &sp, width, app.viewport.layout_generation);
    stats.measured_msgs += 1;
    stats.measured_lines += rendered_lines;
    app.viewport.set_message_height(idx, h);
    app.viewport.mark_message_height_measured(idx);
    if idx + 1 < msg_count {
        app.viewport.prefix_sums_width = 0;
    }
}

/// Measure message height using ground truth: render the message into a scratch
/// buffer and call `Paragraph::line_count(width)`.
///
/// This uses the exact same code path as actual rendering (`render_message()`),
/// so heights can never diverge from what appears on screen. The scratch vec is
/// temporary and discarded after measurement. Block-level caches are still
/// populated as a side effect (via `render_text_cached` / `render_tool_call_cached`),
/// so completed blocks remain O(1) on subsequent calls.
fn measure_message_height(
    msg: &mut crate::app::ChatMessage,
    spinner: &SpinnerState,
    width: u16,
    layout_generation: u64,
) -> (usize, usize) {
    let _t = crate::perf::start_with("chat::measure_msg", "blocks", msg.blocks.len());
    let (h, wrapped_lines) =
        message::measure_message_height_cached(msg, spinner, width, layout_generation);
    crate::perf::mark_with("chat::measure_msg_wrapped_lines", "lines", wrapped_lines);
    (h, wrapped_lines)
}

/// Long content: smooth scroll + viewport culling.
#[allow(
    clippy::cast_possible_truncation,
    clippy::too_many_arguments,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn render_scrolled(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    base: SpinnerState,
    is_thinking: bool,
    show_subagent_thinking: bool,
    width: u16,
    content_height: usize,
    viewport_height: usize,
) {
    let _t = app.perf.as_ref().map(|p| p.start("chat::render_scrolled"));
    let vp = &mut app.viewport;
    let reduced_motion = app.config.prefers_reduced_motion_effective();
    let max_scroll = content_height.saturating_sub(viewport_height);
    if vp.auto_scroll {
        vp.scroll_target = max_scroll;
        // Auto-scroll should stay pinned to the latest content without easing lag.
        vp.scroll_pos = vp.scroll_target as f32;
    }
    vp.scroll_target = vp.scroll_target.min(max_scroll);

    if !vp.auto_scroll {
        let target = vp.scroll_target as f32;
        let delta = target - vp.scroll_pos;
        if reduced_motion || delta.abs() < 0.01 {
            vp.scroll_pos = target;
        } else {
            vp.scroll_pos += delta * 0.3;
        }
    }
    vp.scroll_offset = vp.scroll_pos.round() as usize;
    clamp_scroll_to_content(vp, max_scroll, reduced_motion);

    let scroll_offset = vp.scroll_offset;
    crate::perf::mark_with("chat::max_scroll", "rows", max_scroll);
    crate::perf::mark_with("chat::scroll_offset", "rows", scroll_offset);

    let mut all_lines = Vec::new();
    let render_stats = {
        let _t = app
            .perf
            .as_ref()
            .map(|p| p.start_with("chat::render_msgs", "msgs", app.messages.len()));
        render_culled_messages(
            app,
            base,
            is_thinking,
            show_subagent_thinking,
            width,
            scroll_offset,
            viewport_height,
            &mut all_lines,
        )
    };
    crate::perf::mark_with("chat::render_scrolled_lines", "lines", all_lines.len());
    crate::perf::mark_with("chat::render_scrolled_msgs", "msgs", render_stats.rendered_msgs);
    crate::perf::mark_with(
        "chat::render_scrolled_first_visible",
        "idx",
        render_stats.first_visible,
    );
    crate::perf::mark_with("chat::render_scrolled_start", "idx", render_stats.render_start);

    let paragraph = {
        let _t = app
            .perf
            .as_ref()
            .map(|p| p.start_with("chat::paragraph_build", "lines", all_lines.len()));
        Paragraph::new(Text::from(all_lines)).wrap(Wrap { trim: false })
    };

    app.rendered_chat_area = area;
    if app.selection.is_some_and(|s| s.dragging) {
        let _t = app.perf.as_ref().map(|p| p.start("chat::selection_capture"));
        app.rendered_chat_lines =
            render_lines_from_paragraph(&paragraph, area, render_stats.local_scroll);
    }
    {
        let _t = app
            .perf
            .as_ref()
            .map(|p| p.start_with("chat::render_widget", "scroll", render_stats.local_scroll));
        frame.render_widget(paragraph.scroll((render_stats.local_scroll as u16, 0)), area);
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn clamp_scroll_to_content(
    viewport: &mut crate::app::ChatViewport,
    max_scroll: usize,
    reduced_motion: bool,
) {
    viewport.scroll_target = viewport.scroll_target.min(max_scroll);

    // Shrinks can leave the smoothed scroll position beyond new content end.
    // Ease it back toward the valid bound while keeping rendered offset clamped.
    let max_scroll_f = max_scroll as f32;
    if viewport.scroll_pos > max_scroll_f {
        if reduced_motion {
            viewport.scroll_pos = max_scroll_f;
        } else {
            let overshoot = viewport.scroll_pos - max_scroll_f;
            viewport.scroll_pos = max_scroll_f + overshoot * OVERSCROLL_CLAMP_EASE;
            if (viewport.scroll_pos - max_scroll_f).abs() < SCROLLBAR_EASE_EPSILON {
                viewport.scroll_pos = max_scroll_f;
            }
        }
    }

    viewport.scroll_offset = (viewport.scroll_pos.round() as usize).min(max_scroll);
    if viewport.scroll_offset >= max_scroll {
        viewport.auto_scroll = true;
    }
}

/// Compute overlay scrollbar geometry for a single-column track.
///
/// Returns None when content fits in the viewport.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn compute_scrollbar_geometry(
    content_height: usize,
    viewport_height: usize,
    scroll_pos: f32,
) -> Option<ScrollbarGeometry> {
    if viewport_height == 0 || content_height <= viewport_height {
        return None;
    }
    let max_scroll = content_height.saturating_sub(viewport_height) as f32;
    let thumb_size = viewport_height
        .saturating_mul(viewport_height)
        .checked_div(content_height)
        .unwrap_or(0)
        .max(SCROLLBAR_MIN_THUMB_HEIGHT)
        .min(viewport_height);
    let track_space = viewport_height.saturating_sub(thumb_size) as f32;
    let thumb_top = if max_scroll <= f32::EPSILON || track_space <= 0.0 {
        0
    } else {
        ((scroll_pos.clamp(0.0, max_scroll) / max_scroll) * track_space).round() as usize
    };
    Some(ScrollbarGeometry { thumb_top, thumb_size })
}

fn ease_value(current: &mut f32, target: f32, factor: f32) {
    let delta = target - *current;
    if delta.abs() < SCROLLBAR_EASE_EPSILON {
        *current = target;
    } else {
        *current += delta * factor;
    }
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn smooth_scrollbar_geometry(
    viewport: &mut crate::app::ChatViewport,
    target: ScrollbarGeometry,
    viewport_height: usize,
    reduced_motion: bool,
) -> ScrollbarGeometry {
    let target_top = target.thumb_top as f32;
    let target_size = target.thumb_size as f32;

    if reduced_motion || viewport.scrollbar_thumb_size <= 0.0 {
        viewport.scrollbar_thumb_top = target_top;
        viewport.scrollbar_thumb_size = target_size;
    } else {
        ease_value(&mut viewport.scrollbar_thumb_top, target_top, SCROLLBAR_TOP_EASE);
        ease_value(&mut viewport.scrollbar_thumb_size, target_size, SCROLLBAR_SIZE_EASE);
    }

    let mut thumb_size = viewport.scrollbar_thumb_size.round() as usize;
    thumb_size = thumb_size.max(SCROLLBAR_MIN_THUMB_HEIGHT).min(viewport_height);
    let max_top = viewport_height.saturating_sub(thumb_size);
    let thumb_top = viewport.scrollbar_thumb_top.round().clamp(0.0, max_top as f32) as usize;

    ScrollbarGeometry { thumb_top, thumb_size }
}
#[allow(clippy::cast_possible_truncation)]
fn render_scrollbar_overlay(
    frame: &mut Frame,
    viewport: &mut crate::app::ChatViewport,
    reduced_motion: bool,
    area: Rect,
    content_height: usize,
    viewport_height: usize,
) {
    let Some(target) =
        compute_scrollbar_geometry(content_height, viewport_height, viewport.scroll_pos)
    else {
        viewport.scrollbar_thumb_top = 0.0;
        viewport.scrollbar_thumb_size = 0.0;
        return;
    };
    if area.width == 0 || area.height == 0 {
        return;
    }
    let geometry = smooth_scrollbar_geometry(viewport, target, viewport_height, reduced_motion);
    let rail_style = Style::default().add_modifier(Modifier::DIM);
    let thumb_style = Style::default().fg(theme::ROLE_ASSISTANT);
    let rail_x = area.right().saturating_sub(1);
    let buf = frame.buffer_mut();
    for row in 0..area.height as usize {
        let y = area.y.saturating_add(row as u16);
        if let Some(cell) = buf.cell_mut((rail_x, y)) {
            cell.set_symbol("\u{2595}");
            cell.set_style(rail_style);
        }
    }
    let thumb_top = geometry.thumb_top.min(area.height.saturating_sub(1) as usize);
    let thumb_end = thumb_top.saturating_add(geometry.thumb_size).min(area.height as usize);
    for row in thumb_top..thumb_end {
        let y = area.y.saturating_add(row as u16);
        if let Some(cell) = buf.cell_mut((rail_x, y)) {
            cell.set_symbol("\u{2590}");
            cell.set_style(thumb_style);
        }
    }
}
/// Render only the visible message range into out (viewport culling).
/// Returns the local scroll offset to pass to `Paragraph::scroll()`.
#[allow(clippy::cast_possible_truncation, clippy::too_many_arguments)]
fn render_culled_messages(
    app: &mut App,
    base: SpinnerState,
    is_thinking: bool,
    show_subagent_thinking: bool,
    width: u16,
    scroll: usize,
    viewport_height: usize,
    out: &mut Vec<Line<'static>>,
) -> CulledRenderStats {
    let msg_count = app.messages.len();

    // O(log n) binary search via prefix sums to find first visible message.
    let first_visible = app.viewport.find_first_visible(scroll);

    // Apply margin: render a few extra messages above/below for safety
    let render_start = first_visible.saturating_sub(CULLING_MARGIN);

    // O(1) cumulative height lookup via prefix sums
    let height_before_start = app.viewport.cumulative_height_before(render_start);

    // Render messages from render_start onward, stopping when we have enough
    let lines_needed = (scroll - height_before_start) + viewport_height + 100;
    crate::perf::mark_with("chat::cull_lines_needed", "lines", lines_needed);
    let mut rendered_msgs = 0usize;
    let mut local_scroll = scroll.saturating_sub(height_before_start);
    let mut consume_skip_in_messages = true;
    for i in render_start..msg_count {
        let sp =
            msg_spinner(base, i, msg_count, is_thinking, show_subagent_thinking, &app.messages[i]);
        let before = out.len();
        if local_scroll > 0 && consume_skip_in_messages {
            let rem = message::render_message_from_offset(
                &mut app.messages[i],
                &sp,
                width,
                app.viewport.layout_generation,
                local_scroll,
                out,
            );
            // If we rendered part of this message and still have remaining rows,
            // the remainder is intra-block and must be applied once via
            // `Paragraph::scroll()`, not consumed again by later messages.
            if rem > 0 && out.len() > before {
                consume_skip_in_messages = false;
            }
            local_scroll = rem;
        } else {
            message::render_message(&mut app.messages[i], &sp, width, out);
        }
        if out.len() > before {
            rendered_msgs += 1;
        }
        if out.len() > lines_needed {
            break;
        }
    }

    CulledRenderStats { local_scroll, first_visible, render_start, rendered_msgs }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss)]
pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let _t = app.perf.as_ref().map(|p| p.start("chat::render"));
    crate::perf::mark_with("chat::message_count", "msgs", app.messages.len());
    let is_thinking = matches!(app.status, AppStatus::Thinking);
    let show_subagent_thinking = app.should_show_subagent_thinking(Instant::now());
    let width = area.width;
    let viewport_height = area.height as usize;

    let base_spinner = SpinnerState {
        frame: app.spinner_frame,
        is_active: matches!(app.status, AppStatus::Thinking | AppStatus::Running),
        is_last_message: false,
        is_thinking_mid_turn: false,
        is_subagent_thinking: false,
        is_compacting: app.is_compacting,
    };

    // Detect width change and invalidate layout caches
    {
        let _t = app.perf.as_ref().map(|p| p.start("chat::on_frame"));
        if app.viewport.on_frame(width) {
            app.cache_metrics.record_resize();
        }
    }
    let resize_anchor = app.viewport.resize_scroll_anchor();

    // Update per-message visual heights
    let height_stats = update_visual_heights(
        app,
        base_spinner,
        is_thinking,
        show_subagent_thinking,
        width,
        viewport_height,
    );
    crate::perf::mark_with(
        "chat::update_heights_measured_msgs",
        "msgs",
        height_stats.measured_msgs,
    );
    crate::perf::mark_with("chat::update_heights_reused_msgs", "msgs", height_stats.reused_msgs);
    crate::perf::mark_with(
        "chat::update_heights_measured_lines",
        "lines",
        height_stats.measured_lines,
    );

    // Rebuild prefix sums (O(1) fast path when only last message changed)
    {
        let _t = app.perf.as_ref().map(|p| p.start("chat::prefix_sums"));
        app.viewport.rebuild_prefix_sums();
    }
    if let Some((anchor_idx, anchor_offset)) = resize_anchor {
        app.viewport.restore_scroll_anchor(anchor_idx, anchor_offset);
    }

    // O(1) via prefix sums instead of O(n) sum every frame
    let content_height: usize = app.viewport.total_message_height();
    crate::perf::mark_with("chat::content_height", "rows", content_height);
    crate::perf::mark_with("chat::viewport_height", "rows", viewport_height);
    crate::perf::mark_with(
        "chat::content_overflow_rows",
        "rows",
        content_height.saturating_sub(viewport_height),
    );

    tracing::trace!(
        "RENDER: width={}, content_height={}, viewport_height={}, scroll_target={}, auto_scroll={}",
        width,
        content_height,
        viewport_height,
        app.viewport.scroll_target,
        app.viewport.auto_scroll
    );

    if content_height <= viewport_height {
        crate::perf::mark_with("chat::path_short", "active", 1);
    } else {
        crate::perf::mark_with("chat::path_scrolled", "active", 1);
    }

    render_scrolled(
        frame,
        area,
        app,
        base_spinner,
        is_thinking,
        show_subagent_thinking,
        width,
        content_height,
        viewport_height,
    );

    if let Some(sel) = app.selection
        && sel.kind == SelectionKind::Chat
    {
        frame.render_widget(SelectionOverlay { selection: sel }, app.rendered_chat_area);
    }

    render_scrollbar_overlay(
        frame,
        &mut app.viewport,
        app.config.prefers_reduced_motion_effective(),
        area,
        content_height,
        viewport_height,
    );

    enforce_and_emit_cache_metrics(app);
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn enforce_and_emit_cache_metrics(app: &mut App) {
    let budget_stats = app.enforce_render_cache_budget();
    crate::perf::mark_with("cache::bytes_before", "bytes", budget_stats.total_before_bytes);
    crate::perf::mark_with("cache::bytes_after", "bytes", budget_stats.total_after_bytes);
    crate::perf::mark_with("cache::protected_bytes", "bytes", budget_stats.protected_bytes);
    crate::perf::mark_with("cache::evicted_bytes", "bytes", budget_stats.evicted_bytes);
    crate::perf::mark_with("cache::evicted_blocks", "count", budget_stats.evicted_blocks);

    // -- Accumulate and conditionally log render cache metrics --
    let should_log =
        app.cache_metrics.record_render_enforcement(&budget_stats, &app.render_cache_budget);

    let render_utilization_pct = if app.render_cache_budget.max_bytes > 0 {
        (app.render_cache_budget.last_total_bytes as f32 / app.render_cache_budget.max_bytes as f32)
            * 100.0
    } else {
        0.0
    };
    let history_utilization_pct = if app.history_retention.max_bytes > 0 {
        (app.history_retention_stats.total_after_bytes as f32
            / app.history_retention.max_bytes as f32)
            * 100.0
    } else {
        0.0
    };

    if let Some(warn_kind) = app.cache_metrics.check_warn_condition(
        render_utilization_pct,
        history_utilization_pct,
        budget_stats.evicted_blocks,
    ) {
        cache_metrics::emit_cache_warning(&warn_kind);
    }

    if should_log {
        let entry_count = count_populated_cache_slots(&app.messages);
        let snap = cache_metrics::build_snapshot(
            &app.render_cache_budget,
            &app.history_retention_stats,
            app.history_retention,
            &app.cache_metrics,
            &app.viewport,
            entry_count,
            budget_stats.evicted_blocks,
            0,
            budget_stats.protected_bytes,
        );
        cache_metrics::emit_render_metrics(&snap);

        crate::perf::mark_with("cache::entry_count", "count", entry_count);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        crate::perf::mark_with(
            "cache::utilization_pct_x10",
            "pct",
            (snap.render_utilization_pct * 10.0) as usize,
        );
        crate::perf::mark_with("cache::peak_bytes", "bytes", snap.render_peak_bytes);
    }
}

/// Count cache slots with non-zero cached bytes across all message blocks.
///
/// Only called on log cadence (~every 60 frames), not per-frame.
fn count_populated_cache_slots(messages: &[crate::app::ChatMessage]) -> usize {
    messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter(|block| match block {
            MessageBlock::Text(block) => block.cache.cached_bytes() > 0,
            MessageBlock::Welcome(w) => w.cache.cached_bytes() > 0,
            MessageBlock::ToolCall(tc) => tc.cache.cached_bytes() > 0,
        })
        .count()
}

struct SelectionOverlay {
    selection: SelectionState,
}

impl Widget for SelectionOverlay {
    #[allow(clippy::cast_possible_truncation)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (start, end) =
            crate::app::normalize_selection(self.selection.start, self.selection.end);
        for row in start.row..=end.row {
            let y = area.y.saturating_add(row as u16);
            if y >= area.bottom() {
                break;
            }
            let row_start = if row == start.row { start.col } else { 0 };
            let row_end = if row == end.row { end.col } else { area.width as usize };
            for col in row_start..row_end {
                let x = area.x.saturating_add(col as u16);
                if x >= area.right() {
                    break;
                }
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
                }
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn render_lines_from_paragraph(
    paragraph: &Paragraph,
    area: Rect,
    scroll_offset: usize,
) -> Vec<String> {
    let mut buf = Buffer::empty(area);
    let widget = paragraph.clone().scroll((scroll_offset as u16, 0));
    widget.render(area, &mut buf);
    let mut lines = Vec::with_capacity(area.height as usize);
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            if let Some(cell) = buf.cell((area.x + x, area.y + y)) {
                line.push_str(cell.symbol());
            }
        }
        lines.push(line.trim_end().to_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{
        SCROLLBAR_MIN_THUMB_HEIGHT, ScrollbarGeometry, clamp_scroll_to_content,
        compute_scrollbar_geometry, smooth_scrollbar_geometry, update_visual_heights,
    };
    use crate::app::{
        App, AppStatus, ChatMessage, ChatViewport, InvalidationLevel, MessageBlock, MessageRole,
        TextBlock,
    };
    use crate::ui::message::SpinnerState;

    fn assistant_text_message(text: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::Assistant,
            blocks: vec![MessageBlock::Text(TextBlock::from_complete(text))],
            usage: None,
        }
    }

    #[test]
    fn scrollbar_hidden_when_content_fits() {
        assert_eq!(compute_scrollbar_geometry(10, 10, 0.0), None);
        assert_eq!(compute_scrollbar_geometry(8, 10, 0.0), None);
    }
    #[test]
    fn scrollbar_thumb_positions_are_stable() {
        assert_eq!(
            compute_scrollbar_geometry(50, 10, 0.0),
            Some(ScrollbarGeometry { thumb_top: 0, thumb_size: 2 })
        );
        assert_eq!(
            compute_scrollbar_geometry(50, 10, 20.0),
            Some(ScrollbarGeometry { thumb_top: 4, thumb_size: 2 })
        );
        assert_eq!(
            compute_scrollbar_geometry(50, 10, 40.0),
            Some(ScrollbarGeometry { thumb_top: 8, thumb_size: 2 })
        );
    }
    #[test]
    fn scrollbar_scroll_offset_is_clamped() {
        assert_eq!(
            compute_scrollbar_geometry(50, 10, 999.0),
            Some(ScrollbarGeometry { thumb_top: 8, thumb_size: 2 })
        );
    }
    #[test]
    fn scrollbar_handles_small_overflow() {
        assert_eq!(
            compute_scrollbar_geometry(11, 10, 1.0),
            Some(ScrollbarGeometry { thumb_top: 1, thumb_size: 9 })
        );
    }
    #[test]
    fn scrollbar_respects_min_thumb_height() {
        assert_eq!(
            compute_scrollbar_geometry(10_000, 10, 0.0),
            Some(ScrollbarGeometry { thumb_top: 0, thumb_size: SCROLLBAR_MIN_THUMB_HEIGHT })
        );
    }

    #[test]
    fn update_visual_heights_remeasures_dirty_non_tail_message() {
        let mut app = App::test_default();
        app.status = AppStatus::Ready;
        app.messages =
            vec![assistant_text_message("short"), assistant_text_message("tail stays unchanged")];

        app.viewport.on_frame(12);
        let spinner = SpinnerState {
            frame: 0,
            is_active: false,
            is_last_message: false,
            is_thinking_mid_turn: false,
            is_subagent_thinking: false,
            is_compacting: false,
        };

        update_visual_heights(&mut app, spinner, false, false, 12, 8);
        let base_h = app.viewport.message_height(0);
        assert!(base_h > 0);

        if let Some(MessageBlock::Text(block)) =
            app.messages.get_mut(0).and_then(|m| m.blocks.get_mut(0))
        {
            let extra = " this now wraps across multiple lines";
            block.text.push_str(extra);
            block.markdown.append(extra);
            block.cache.invalidate();
        }
        app.invalidate_layout(InvalidationLevel::From(0));

        update_visual_heights(&mut app, spinner, false, false, 12, 8);
        assert!(
            app.viewport.message_height(0) > base_h,
            "dirty non-tail message should be remeasured"
        );
    }

    #[allow(clippy::cast_precision_loss)]
    #[test]
    fn resize_remeasure_updates_visible_window_before_far_messages() {
        let mut app = App::test_default();
        let text = "This message should wrap after resize and stay expensive enough to measure. "
            .repeat(6);
        app.messages = (0..32).map(|_| assistant_text_message(&text)).collect();

        let spinner = SpinnerState {
            frame: 0,
            is_active: false,
            is_last_message: false,
            is_thinking_mid_turn: false,
            is_subagent_thinking: false,
            is_compacting: false,
        };

        app.viewport.on_frame(48);
        update_visual_heights(&mut app, spinner, false, false, 48, 12);
        app.viewport.rebuild_prefix_sums();
        let per_message_height = app.viewport.message_height(0);
        assert!(per_message_height > 0);

        let visible_rows = per_message_height * 2;
        app.viewport.scroll_offset = per_message_height * 15;
        app.viewport.scroll_target = app.viewport.scroll_offset;
        app.viewport.scroll_pos = app.viewport.scroll_offset as f32;

        assert!(app.viewport.on_frame(18));
        update_visual_heights(&mut app, spinner, false, false, 18, visible_rows);

        assert_eq!(app.viewport.message_heights_width, 0);
        assert!(app.viewport.resize_remeasure_active());
        assert!(app.viewport.message_height_is_current(15));
        assert!(app.viewport.message_height_is_current(16));
        assert!(!app.viewport.message_height_is_current(0));
    }

    #[allow(clippy::cast_precision_loss)]
    #[test]
    fn resize_remeasure_converges_over_multiple_frames() {
        let mut app = App::test_default();
        let text = "This message should wrap after resize and stay expensive enough to measure. "
            .repeat(6);
        app.messages = (0..40).map(|_| assistant_text_message(&text)).collect();

        let spinner = SpinnerState {
            frame: 0,
            is_active: false,
            is_last_message: false,
            is_thinking_mid_turn: false,
            is_subagent_thinking: false,
            is_compacting: false,
        };

        app.viewport.on_frame(48);
        update_visual_heights(&mut app, spinner, false, false, 48, 12);
        app.viewport.rebuild_prefix_sums();
        let per_message_height = app.viewport.message_height(0);
        app.viewport.scroll_offset = per_message_height * 12;
        app.viewport.scroll_target = app.viewport.scroll_offset;
        app.viewport.scroll_pos = app.viewport.scroll_offset as f32;

        assert!(app.viewport.on_frame(18));
        for _ in 0..8 {
            update_visual_heights(&mut app, spinner, false, false, 18, per_message_height * 2);
            app.viewport.rebuild_prefix_sums();
            if !app.viewport.resize_remeasure_active() {
                break;
            }
        }

        assert_eq!(app.viewport.message_heights_width, 18);
        assert!(!app.viewport.resize_remeasure_active());
        assert!(app.viewport.message_height_is_current(0));
        assert!(app.viewport.message_height_is_current(39));
    }

    #[allow(clippy::cast_precision_loss)]
    #[test]
    fn resize_remeasure_does_not_repeat_dirty_suffix_after_measuring_it() {
        let mut app = App::test_default();
        let text = "This message should wrap after resize and stay expensive enough to measure. "
            .repeat(6);
        app.messages = (0..8).map(|_| assistant_text_message(&text)).collect();

        let spinner = SpinnerState {
            frame: 0,
            is_active: false,
            is_last_message: false,
            is_thinking_mid_turn: false,
            is_subagent_thinking: false,
            is_compacting: false,
        };

        app.viewport.on_frame(48);
        update_visual_heights(&mut app, spinner, false, false, 48, 12);
        app.viewport.rebuild_prefix_sums();
        let per_message_height = app.viewport.message_height(0);
        app.viewport.scroll_offset = per_message_height * 2;
        app.viewport.scroll_target = app.viewport.scroll_offset;
        app.viewport.scroll_pos = app.viewport.scroll_offset as f32;

        assert!(app.viewport.on_frame(18));
        app.invalidate_layout(InvalidationLevel::From(0));

        let first =
            update_visual_heights(&mut app, spinner, false, false, 18, per_message_height * 2);
        app.viewport.rebuild_prefix_sums();
        let second =
            update_visual_heights(&mut app, spinner, false, false, 18, per_message_height * 2);

        assert!(first.measured_msgs >= app.messages.len());
        assert_eq!(second.measured_msgs, 0);
        assert_eq!(app.viewport.message_heights_width, 18);
    }

    #[test]
    fn clamp_scroll_to_content_snaps_overscroll_after_shrink() {
        let mut viewport = ChatViewport::new();
        viewport.auto_scroll = false;
        viewport.scroll_target = 120;
        viewport.scroll_pos = 120.0;
        viewport.scroll_offset = 120;

        clamp_scroll_to_content(&mut viewport, 40, false);

        assert!(viewport.auto_scroll);
        assert_eq!(viewport.scroll_target, 40);
        assert!(viewport.scroll_pos > 40.0);
        assert!(viewport.scroll_pos < 120.0);
        assert_eq!(viewport.scroll_offset, 40);
    }

    #[test]
    fn clamp_scroll_to_content_preserves_in_range_scroll() {
        let mut viewport = ChatViewport::new();
        viewport.auto_scroll = false;
        viewport.scroll_target = 20;
        viewport.scroll_pos = 20.0;
        viewport.scroll_offset = 20;

        clamp_scroll_to_content(&mut viewport, 40, false);

        assert!(!viewport.auto_scroll);
        assert_eq!(viewport.scroll_target, 20);
        assert!((viewport.scroll_pos - 20.0).abs() < f32::EPSILON);
        assert_eq!(viewport.scroll_offset, 20);
    }

    #[test]
    fn clamp_scroll_to_content_settles_to_max_over_frames() {
        let mut viewport = ChatViewport::new();
        viewport.auto_scroll = false;
        viewport.scroll_target = 120;
        viewport.scroll_pos = 120.0;
        viewport.scroll_offset = 120;

        for _ in 0..12 {
            clamp_scroll_to_content(&mut viewport, 40, false);
        }

        assert_eq!(viewport.scroll_target, 40);
        assert_eq!(viewport.scroll_offset, 40);
        assert!(viewport.scroll_pos >= 40.0);
        assert!(viewport.scroll_pos < 40.1);
    }

    #[test]
    fn clamp_scroll_to_content_snaps_overscroll_when_reduced_motion_enabled() {
        let mut viewport = ChatViewport::new();
        viewport.auto_scroll = false;
        viewport.scroll_target = 120;
        viewport.scroll_pos = 120.0;
        viewport.scroll_offset = 120;

        clamp_scroll_to_content(&mut viewport, 40, true);

        assert!(viewport.auto_scroll);
        assert_eq!(viewport.scroll_target, 40);
        assert!((viewport.scroll_pos - 40.0).abs() < f32::EPSILON);
        assert_eq!(viewport.scroll_offset, 40);
    }

    #[test]
    fn smooth_scrollbar_geometry_snaps_when_reduced_motion_enabled() {
        let mut viewport = ChatViewport::new();
        viewport.scrollbar_thumb_top = 2.0;
        viewport.scrollbar_thumb_size = 3.0;

        let geometry = smooth_scrollbar_geometry(
            &mut viewport,
            ScrollbarGeometry { thumb_top: 9, thumb_size: 5 },
            20,
            true,
        );

        assert_eq!(geometry, ScrollbarGeometry { thumb_top: 9, thumb_size: 5 });
        assert!((viewport.scrollbar_thumb_top - 9.0).abs() < f32::EPSILON);
        assert!((viewport.scrollbar_thumb_size - 5.0).abs() < f32::EPSILON);
    }
}
