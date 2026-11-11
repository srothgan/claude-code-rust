// SPDX-License-Identifier: Apache-2.0

use crate::app::{
    App, AppStatus, ChatMessage, MessageBlock, SurfaceMode, TerminalLifecycleState, WelcomeBlock,
};
use crate::ui::host_tips::HOST_TIPS;
use crate::ui::theme;
use crate::ui::wrap::{
    StyledChunk, display_width, join_column_lines, wrap_styled_chunks,
    wrap_styled_chunks_with_hanging_prefix,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::{Duration, Instant};

/// One pose of the welcome crab: four rows, each as wide as the rest pose's,
/// so the overview text beside it never shifts.
pub(crate) type FerrisFrame = [&'static str; 4];

const SHELL: &str = r"    _~^~^~_     ";
const EYES: &str = r"\) /  o o  \ (/ ";
const EYES_LEFT: &str = r"\) / o o   \ (/ ";
const EYES_RIGHT: &str = r"\) /   o o \ (/ ";
const EYES_BLINK: &str = r"\) /  - -  \ (/ ";
const MOUTH: &str = r"  '_   -   _'   ";
const SMILE: &str = r"  '_   u   _'   ";
const LEGS_A: &str = r"  / '-----' \   ";
const LEGS_B: &str = r"  \ '-----' /   ";
const WAVE_SHELL: &str = r"\)  _~^~^~_  (/ ";
const WAVE_EYES: &str = r" \ /  o o  \ /  ";
const WAVE_BLINK: &str = r" \ /  - -  \ /  ";

pub(crate) const FERRIS_REST: FerrisFrame = [SHELL, EYES, MOUTH, LEGS_A];
const FERRIS_REST_BLINK: FerrisFrame = [SHELL, EYES_BLINK, MOUTH, LEGS_A];
const FERRIS_WAVE: FerrisFrame = [WAVE_SHELL, WAVE_EYES, SMILE, LEGS_A];
const FERRIS_WAVE_BLINK: FerrisFrame = [WAVE_SHELL, WAVE_BLINK, SMILE, LEGS_A];

/// Scuttles in place and looks around while the session connects.
const CONNECTING_FRAMES: [FerrisFrame; 4] = [
    [SHELL, EYES_LEFT, MOUTH, LEGS_A],
    [SHELL, EYES, MOUTH, LEGS_B],
    [SHELL, EYES_RIGHT, MOUTH, LEGS_A],
    [SHELL, EYES, MOUTH, LEGS_B],
];
/// Spinner frames per connecting pose, like `PULSE_FRAME_DIVISOR` in
/// `tab_title.rs`. The spinner advances once per two 16 ms loop ticks (about
/// 32 ms), so a pose lasts about 192 ms.
const CONNECTING_FRAME_DIVISOR: usize = 6;

/// Played once, after the first session of the process connects. It ends on
/// the rest pose, so the last frame drawn inside the window is already final.
const HELLO_FRAMES: [FerrisFrame; 5] =
    [FERRIS_WAVE, FERRIS_WAVE_BLINK, FERRIS_WAVE, FERRIS_REST_BLINK, FERRIS_REST];
const HELLO_BEAT: Duration = Duration::from_millis(240);
const HELLO_WINDOW: Duration = Duration::from_millis(1_200);

/// What the welcome crab is doing at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FerrisPhase {
    Rest,
    Connecting { spinner_frame: usize },
    Hello { elapsed: Duration },
}

/// The crab's phase, derived from app state and the clock; the only stored
/// input is the instant of the first connection. `width` is the width the
/// welcome is rendered at.
pub(crate) fn ferris_phase(
    app: &App,
    welcome: &WelcomeBlock,
    width: u16,
    now: Instant,
) -> FerrisPhase {
    if !motion_allowed(app) {
        return FerrisPhase::Rest;
    }
    // A welcome whose values are loading is never commit-ready, so the loop
    // can never be frozen into scrollback mid-step.
    if app.status == AppStatus::Connecting && !welcome_values_ready(welcome) {
        return FerrisPhase::Connecting { spinner_frame: app.spinner_frame };
    }
    hello_beat(app, width, now).map_or(FerrisPhase::Rest, |elapsed| FerrisPhase::Hello { elapsed })
}

/// Whether the frame loop must keep redrawing for the welcome hello: while
/// its window holds the welcome back from scrollback and a chat surface shows
/// it. The draw after the window closes is the one that commits the welcome,
/// so the loop redraws through the window even before the motion preference
/// is known and the wave cannot start yet. The connecting loop needs nothing
/// here: the Connecting status already redraws.
pub(crate) fn ferris_hello_live(app: &App, now: Instant) -> bool {
    let chat_visible = matches!(
        app.terminal_lifecycle,
        TerminalLifecycleState::Running(SurfaceMode::Chat) | TerminalLifecycleState::Bootstrapping
    );
    chat_visible && hello_window(app, app.chat_render.terminal_size().width, now).is_some()
}

/// Whether the welcome rows may be committed to scrollback, where they can
/// never be redrawn. They are held back while the hello window is open, so a
/// committed welcome is always final and always at rest.
pub(crate) fn welcome_commit_ready(
    app: &App,
    welcome: &WelcomeBlock,
    width: u16,
    now: Instant,
) -> bool {
    welcome_values_ready(welcome) && hello_window(app, width, now).is_none()
}

/// The time into the first-connect hello while it holds the welcome back:
/// the crab is visible at `width`, the welcome is the whole transcript (a
/// prompt, a notice or resumed history snaps the crab to rest) and the window
/// since the first connection has not closed. Later connects of the process
/// never reopen it.
fn hello_window(app: &App, width: u16, now: Instant) -> Option<Duration> {
    if !motion_allowed(app)
        || !ferris_column_visible(usize::from(width))
        || !welcome_is_whole_transcript(app)
    {
        return None;
    }
    let elapsed = now.saturating_duration_since(app.startup.first_connected_at()?);
    (elapsed < HELLO_WINDOW).then_some(elapsed)
}

/// The time into the hello while the crab waves. The preference arrives with
/// the settings snapshot, which is requested only after the first connect, so
/// the wave starts partway through its window if the snapshot is late and is
/// skipped if it comes after the window.
fn hello_beat(app: &App, width: u16, now: Instant) -> Option<Duration> {
    hello_window(app, width, now).filter(|_| app.config.snapshot.is_some())
}

/// Whether the transcript holds nothing but the welcome.
pub(crate) fn welcome_is_whole_transcript(app: &App) -> bool {
    matches!(app.transcript.messages.as_slice(), [message] if welcome_block(message).is_some())
}

fn motion_allowed(app: &App) -> bool {
    !app.config.prefers_reduced_motion_effective()
        && !app.shutdown_requested()
        && app.show_session_overview
}

/// The pose for a phase.
pub(crate) fn ferris_frame(phase: FerrisPhase) -> &'static FerrisFrame {
    match phase {
        FerrisPhase::Rest => &FERRIS_REST,
        FerrisPhase::Connecting { spinner_frame } => {
            let pose = spinner_frame / CONNECTING_FRAME_DIVISOR;
            CONNECTING_FRAMES.get(pose % CONNECTING_FRAMES.len()).unwrap_or(&FERRIS_REST)
        }
        // Past the last beat the hello holds its final pose, which is rest.
        FerrisPhase::Hello { elapsed } => {
            HELLO_FRAMES.get(beat_index(elapsed, HELLO_BEAT)).unwrap_or(&FERRIS_REST)
        }
    }
}

fn beat_index(elapsed: Duration, interval: Duration) -> usize {
    let beats = elapsed.as_millis() / interval.as_millis().max(1);
    usize::try_from(beats).unwrap_or(usize::MAX)
}

pub(crate) fn welcome_block(message: &ChatMessage) -> Option<&WelcomeBlock> {
    message.blocks.iter().find_map(|block| match block {
        MessageBlock::Welcome(welcome) => Some(welcome),
        MessageBlock::Text(_)
        | MessageBlock::BtwExchange(_)
        | MessageBlock::Notice(_)
        | MessageBlock::ToolCall(_)
        | MessageBlock::ToolResult { .. }
        | MessageBlock::ImageAttachment(_)
        | MessageBlock::UserDialog(_) => None,
    })
}

fn welcome_values_ready(welcome: &WelcomeBlock) -> bool {
    !welcome_value_missing(&welcome.subscription) && !welcome_value_missing(&welcome.session_id)
}

const FERRIS_LEFT_PADDING: &str = "  ";
const FERRIS_TEXT_GAP: usize = 2;
const MIN_INLINE_FIELD_VALUE_WIDTH: usize = 8;
const WELCOME_FIELD_LABELS: &[&str] = &["Version", "Subscription", "Cwd", "Session ID", "Tips"];

pub(crate) fn overview_lines(
    block: &WelcomeBlock,
    loading_status: Option<&str>,
    ferris: &FerrisFrame,
    width: u16,
) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if width == 0 {
        return vec![Line::default()];
    }

    let loading = loading_status.unwrap_or("Loading");
    let subscription_missing = welcome_value_missing(&block.subscription);
    let session_missing = welcome_value_missing(&block.session_id);
    let subscription_value =
        if subscription_missing { loading.to_owned() } else { block.subscription.clone() };
    let session_value = if session_missing { loading.to_owned() } else { block.session_id.clone() };
    let subscription_style = if subscription_missing {
        Style::default().fg(theme::DIM)
    } else {
        Style::default().fg(theme::RUST_ORANGE).add_modifier(Modifier::BOLD)
    };

    let ferris_width = ferris_column_width();
    let overview_width = width.saturating_sub(ferris_width.saturating_add(FERRIS_TEXT_GAP));
    let side_by_side = ferris_column_visible(width);
    let text_width = if side_by_side { overview_width } else { width };
    let text_rows = overview_text_rows(
        block,
        &subscription_value,
        subscription_style,
        &session_value,
        text_width,
    );
    let mut lines = if side_by_side {
        join_column_lines(ferris_rows(ferris), text_rows, ferris_width, FERRIS_TEXT_GAP)
    } else {
        text_rows
    };
    lines.push(Line::default());
    lines
}

fn overview_text_rows(
    block: &WelcomeBlock,
    subscription_value: &str,
    subscription_style: Style,
    session_value: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme::DIM);
    let mut rows = Vec::new();
    rows.extend(welcome_field_lines("Version", &block.version, dim, width));
    rows.extend(welcome_field_lines("Subscription", subscription_value, subscription_style, width));
    rows.extend(welcome_field_lines("Cwd", &block.cwd, dim, width));
    rows.extend(welcome_field_lines("Session ID", session_value, dim, width));
    rows.push(Line::default());
    rows.extend(welcome_field_lines("Tips", selected_tip(block), dim, width));
    rows
}

fn welcome_field_lines(
    label: &str,
    value: &str,
    value_style: Style,
    width: usize,
) -> Vec<Line<'static>> {
    let prefix =
        vec![StyledChunk { text: format!("{label}: "), style: Style::default().fg(theme::DIM) }];
    let body = vec![StyledChunk { text: value.to_owned(), style: value_style }];
    let prefix_width = prefix.iter().map(|chunk| display_width(&chunk.text)).sum::<usize>();
    if width.saturating_sub(prefix_width) < MIN_INLINE_FIELD_VALUE_WIDTH {
        let mut rows = wrap_styled_chunks(&prefix, width);
        rows.extend(wrap_styled_chunks(&body, width));
        return rows;
    }

    wrap_styled_chunks_with_hanging_prefix(&prefix, &body, width, Style::default())
}

fn ferris_column_width() -> usize {
    display_width(FERRIS_LEFT_PADDING)
        .saturating_add(FERRIS_REST.iter().map(|line| display_width(line)).max().unwrap_or(0))
}

/// Whether a row `width` columns wide shows the crab beside the overview; a
/// narrower one shows the overview text alone.
fn ferris_column_visible(width: usize) -> bool {
    let overview_offset = ferris_column_width().saturating_add(FERRIS_TEXT_GAP);
    width.saturating_sub(overview_offset) >= minimum_overview_width()
}

fn ferris_rows(ferris: &FerrisFrame) -> Vec<Line<'static>> {
    ferris
        .iter()
        .map(|art| {
            Line::from(Span::styled(
                format!("{FERRIS_LEFT_PADDING}{art}"),
                Style::default().fg(theme::RUST_ORANGE),
            ))
        })
        .collect()
}

fn minimum_overview_width() -> usize {
    WELCOME_FIELD_LABELS
        .iter()
        .map(|label| display_width(&format!("{label}: ")))
        .max()
        .unwrap_or(0)
        .saturating_add(MIN_INLINE_FIELD_VALUE_WIDTH)
}

fn welcome_value_missing(value: &str) -> bool {
    value.trim().is_empty() || value == "-"
}

pub(crate) fn selected_tip(block: &WelcomeBlock) -> &'static str {
    let Some(first_tip) = HOST_TIPS.first().copied() else {
        return "Enter sends, Shift+Enter inserts a newline, and Ctrl+C clears or quits";
    };
    let len_u64 = u64::try_from(HOST_TIPS.len()).unwrap_or(1);
    let idx_u64 = block.tip_seed % len_u64;
    let idx = usize::try_from(idx_u64).unwrap_or(0);
    HOST_TIPS.get(idx).copied().unwrap_or(first_tip)
}

#[cfg(test)]
mod tests {
    use super::{FERRIS_REST, HOST_TIPS, Line, overview_lines};
    use crate::app::{ChatMessage, MessageBlock};
    use crate::ui::wrap::line_display_width;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn field_region(rows: &[String], label: &str) -> (usize, Vec<String>) {
        let labels = ["Version:", "Subscription:", "Cwd:", "Session ID:", "Tips:"];
        let start = rows.iter().position(|row| row.contains(label)).expect("field label");
        let column = rows[start].find(label).unwrap();
        let end = rows[start + 1..]
            .iter()
            .position(|row| labels.iter().any(|next| row.contains(next)))
            .map_or(rows.len(), |offset| start + 1 + offset);
        let mut region = vec![rows[start][column + label.len()..].to_owned()];
        region.extend(
            rows[start + 1..end].iter().map(|row| row.get(column..).unwrap_or_default().to_owned()),
        );
        (column, region)
    }

    fn assert_field_source(region: &[String], expected: &str) {
        let expected = expected.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut remaining = expected.as_str();
        for piece in region.iter().map(|row| row.trim()).filter(|row| !row.is_empty()) {
            remaining = remaining.strip_prefix(' ').unwrap_or(remaining);
            assert!(
                remaining.starts_with(piece),
                "field changed source text: {region:?}, expected {expected:?}"
            );
            remaining = &remaining[piece.len()..];
        }
        assert!(remaining.is_empty(), "field lost source text: {region:?}, missing {remaining:?}");
    }

    #[test]
    fn overview_fields_keep_their_own_resolved_or_loading_values() {
        for (subscription, session, cause, expected_subscription, expected_session) in [
            (
                "Enterprise fixture",
                "session-saffron",
                Some("WAIT_PROFILE"),
                "Enterprise fixture",
                "session-saffron",
            ),
            ("-", "session-saffron", Some("WAIT_PROFILE"), "WAIT_PROFILE", "session-saffron"),
            (
                "Enterprise fixture",
                "  ",
                Some("WAIT_SESSION"),
                "Enterprise fixture",
                "WAIT_SESSION",
            ),
            ("", "-", None, "Loading", "Loading"),
        ] {
            let message = ChatMessage::welcome("8.7.6", subscription, "/workspace/cedar", session);
            let MessageBlock::Welcome(block) = &message.blocks[0] else { panic!("welcome block") };
            let rows = overview_lines(block, cause, &FERRIS_REST, 120)
                .iter()
                .map(line_text)
                .collect::<Vec<_>>();
            for (label, expected) in [
                ("Version:", "8.7.6"),
                ("Subscription:", expected_subscription),
                ("Cwd:", "/workspace/cedar"),
                ("Session ID:", expected_session),
            ] {
                let (_, region) = field_region(&rows, label);
                if cause.is_none() && expected == "Loading" {
                    assert!(region.iter().any(|row| row.contains("Loading")), "{label}: {rows:?}");
                } else {
                    assert_field_source(&region, expected);
                }
            }
        }
    }

    #[test]
    fn welcome_fields_preserve_unicode_content_and_relative_wrapping() {
        let cwd =
            "/workspace/雪 cedar/e\u{301}clair wide directory/amber cobalt saffron/final-file";
        let session = "session-orchid-782";
        let mut message = ChatMessage::welcome("8.7.6", "Enterprise fixture", cwd, session);
        let MessageBlock::Welcome(block) = &mut message.blocks[0] else { panic!("welcome block") };
        let (index, tip) = HOST_TIPS.iter().enumerate().max_by_key(|(_, tip)| tip.len()).unwrap();
        block.tip_seed = u64::try_from(index).unwrap();
        for width in [100, 46, 24, 14] {
            let lines = overview_lines(block, None, &FERRIS_REST, width);
            assert!(lines.iter().all(|line| line_display_width(line) <= usize::from(width)));
            let rows = lines.iter().map(line_text).collect::<Vec<_>>();
            for (label, expected) in [("Cwd:", cwd), ("Session ID:", session), ("Tips:", *tip)] {
                let (_, region) = field_region(&rows, label);
                assert_field_source(&region, expected);
                if !region[0].trim().is_empty() {
                    let value_start = label.len() + region[0].len() - region[0].trim_start().len();
                    for continuation in region.iter().skip(1).filter(|row| !row.trim().is_empty()) {
                        let indent = continuation.len() - continuation.trim_start().len();
                        assert_eq!(
                            indent, value_start,
                            "continuation escaped its field: {region:?}"
                        );
                    }
                }
            }
        }
    }
}
