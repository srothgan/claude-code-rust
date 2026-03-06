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

//! Timing-based paste burst detection for terminals that don't reliably
//! surface bracketed paste events (notably Windows Terminal with crossterm).
//!
//! When a user pastes text, the terminal may deliver each character as a
//! separate `Event::Key(Char(_))` at machine speed. This module detects
//! such rapid character streams and buffers them into a single paste payload,
//! which is then routed through [`super::App::queue_paste_text`] like a
//! normal `Event::Paste`.
//!
//! Design inspired by:
//! - Codex CLI (`paste_burst.rs`): hold-first strategy, platform-tuned
//!   intervals, enter suppression window, retro-capture heuristics.
//! - Gemini CLI (`KeypressContext.tsx`): fast-return buffering at 30ms.
//!
//! # State machine
//!
//! ```text
//! Idle ──(fast char)──> Pending ──(2nd fast char)──> Buffering ──(idle)──> Flush
//!                          │                                        │
//!                      (timeout)                             (enter suppression
//!                          v                                  window active)
//!                     Emit held char
//! ```

use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Platform-tuned timing constants
// ---------------------------------------------------------------------------

/// Maximum gap between consecutive characters to be considered part of the
/// same paste burst. Characters arriving faster than this are machine-speed
/// (paste), not human typing (~100-200ms per keystroke).
#[cfg(not(windows))]
const CHAR_INTERVAL: Duration = Duration::from_millis(8);

#[cfg(windows)]
const CHAR_INTERVAL: Duration = Duration::from_millis(30);

/// How long to wait after the last buffered character before flushing the
/// burst as a completed paste. Slightly longer on Windows where terminal
/// I/O adds latency between pasted characters.
#[cfg(not(windows))]
const IDLE_TIMEOUT: Duration = Duration::from_millis(8);

#[cfg(windows)]
const IDLE_TIMEOUT: Duration = Duration::from_millis(50);

/// After a burst is flushed, suppress Enter-as-submit for this duration.
/// Handles terminals that insert a small gap between the last pasted
/// character and a trailing newline.
const ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(100);

/// Minimum characters in a burst to classify it as paste (not fast typing).
const MIN_BURST_LEN: usize = 3;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Action the caller should take after feeding a character to the detector.
#[derive(Debug, PartialEq)]
pub enum CharAction {
    /// Character was consumed into the burst buffer. Do not insert it.
    Consumed,
    /// Character was consumed, and `n` previously inserted characters should
    /// be deleted from the input before continuing (retro-capture).
    RetroCapture(usize),
    /// Not a burst -- insert this character normally.
    Passthrough(char),
}

/// Action produced by [`PasteBurstDetector::tick`] when a timeout fires.
#[derive(Debug, PartialEq)]
pub enum FlushAction {
    /// The held character timed out without a follow-up. Emit as normal input.
    EmitChar(char),
    /// A burst completed. Emit the accumulated text as a paste payload.
    EmitPaste(String),
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum BurstState {
    /// No active burst.
    Idle,
    /// A single fast character is being held. If a second arrives within
    /// `CHAR_INTERVAL`, both move into `Buffering`. Otherwise the held
    /// character is emitted as a normal keystroke on the next `tick()`.
    Pending { held_char: char, received_at: Instant, retro_prefix: Vec<char> },
    /// Actively accumulating a rapid character stream.
    Buffering,
}

/// Detects rapid character input streams (paste events delivered as
/// individual key events) and buffers them into a single paste payload.
#[derive(Debug)]
pub struct PasteBurstDetector {
    state: BurstState,
    /// Accumulated characters during `Buffering` state.
    buffer: String,
    /// Timestamp of the last character fed to the detector.
    last_char_time: Option<Instant>,
    /// Last recently-inserted passthrough characters (chronological, newest at end).
    /// Used for retro-capturing leaked leading characters when a burst is confirmed.
    recent_passthrough: std::collections::VecDeque<(char, Instant)>,
    /// After a burst flush, suppress Enter-as-submit until this instant.
    enter_suppress_until: Option<Instant>,
}

impl PasteBurstDetector {
    pub fn new() -> Self {
        Self {
            state: BurstState::Idle,
            buffer: String::new(),
            last_char_time: None,
            recent_passthrough: std::collections::VecDeque::with_capacity(2),
            enter_suppress_until: None,
        }
    }

    /// Feed a printable character event. Returns whether the caller should
    /// insert it or whether the detector consumed it.
    pub fn on_char(&mut self, ch: char, now: Instant) -> CharAction {
        let is_fast = self
            .last_char_time
            .is_some_and(|last| now.saturating_duration_since(last) <= CHAR_INTERVAL);
        self.last_char_time = Some(now);

        match &self.state {
            BurstState::Idle => {
                if is_fast {
                    // A character arrived quickly after the previous one
                    // (which was already inserted as passthrough). Transition
                    // to Pending to hold this one and see if a burst follows.
                    self.state = BurstState::Pending {
                        held_char: ch,
                        received_at: now,
                        retro_prefix: self.collect_retro_prefix(now),
                    };
                    CharAction::Consumed
                } else {
                    // Normal typing speed -- pass through immediately.
                    self.push_recent_passthrough(ch, now);
                    CharAction::Passthrough(ch)
                }
            }
            BurstState::Pending { held_char, retro_prefix, .. } => {
                if is_fast {
                    // Second fast character confirms a burst is starting.
                    let held = *held_char;
                    let retro_len = retro_prefix.len();
                    self.buffer.clear();
                    for prefix in retro_prefix {
                        self.buffer.push(*prefix);
                    }
                    self.buffer.push(held);
                    self.buffer.push(ch);
                    self.state = BurstState::Buffering;
                    self.recent_passthrough.clear();
                    if retro_len > 0 {
                        CharAction::RetroCapture(retro_len)
                    } else {
                        CharAction::Consumed
                    }
                } else {
                    // Gap too long -- the held char was a false alarm.
                    // Emit both characters as normal typing by releasing the
                    // held char and returning to Idle for the current char.
                    let prev = *held_char;
                    self.state = BurstState::Idle;
                    self.push_recent_passthrough(prev, now);
                    self.push_recent_passthrough(ch, now);
                    CharAction::Passthrough(prev)
                }
            }
            BurstState::Buffering => {
                // Once a burst is confirmed, keep buffering until idle timeout.
                // This tolerates Windows scheduling jitter between pasted chars.
                self.buffer.push(ch);
                CharAction::Consumed
            }
        }
    }

    /// Feed an Enter key event. Returns `true` if Enter should be treated
    /// as a newline (inserted into the buffer) rather than a submit action.
    ///
    /// This covers two cases:
    /// 1. Enter arrives while actively buffering a burst (append newline).
    /// 2. Enter arrives within the post-burst suppression window.
    pub fn on_enter(&mut self, now: Instant) -> bool {
        match &self.state {
            BurstState::Buffering => {
                self.buffer.push('\n');
                self.last_char_time = Some(now);
                true
            }
            BurstState::Pending { held_char, .. } => {
                // Promote held char + Enter into a buffering burst.
                let held = *held_char;
                self.buffer.clear();
                self.buffer.push(held);
                self.buffer.push('\n');
                self.state = BurstState::Buffering;
                self.last_char_time = Some(now);
                true
            }
            BurstState::Idle => self.should_suppress_enter(now),
        }
    }

    /// Check for timeouts and return any pending action.
    /// Call once per drain cycle (after all events are processed).
    pub fn tick(&mut self, now: Instant) -> Option<FlushAction> {
        match &self.state {
            BurstState::Pending { held_char, received_at, .. } => {
                if now.saturating_duration_since(*received_at) > IDLE_TIMEOUT {
                    let ch = *held_char;
                    self.state = BurstState::Idle;
                    self.push_recent_passthrough(ch, now);
                    Some(FlushAction::EmitChar(ch))
                } else {
                    None
                }
            }
            BurstState::Buffering => {
                let idle = self
                    .last_char_time
                    .is_some_and(|last| now.saturating_duration_since(last) > IDLE_TIMEOUT);
                if idle {
                    let text = self.flush_buffer(now);
                    if text.is_empty() { None } else { Some(FlushAction::EmitPaste(text)) }
                } else {
                    None
                }
            }
            BurstState::Idle => None,
        }
    }

    /// Whether Enter should be suppressed (treated as newline, not submit).
    pub fn should_suppress_enter(&self, now: Instant) -> bool {
        self.enter_suppress_until.is_some_and(|until| now <= until)
    }

    /// Whether the detector is actively buffering characters.
    #[must_use]
    pub fn is_buffering(&self) -> bool {
        matches!(self.state, BurstState::Buffering | BurstState::Pending { .. })
    }

    /// Reset burst state on non-character key events (arrows, Esc, etc.).
    /// Prevents state from leaking across unrelated input.
    pub fn on_non_char_key(&mut self, now: Instant) {
        if matches!(self.state, BurstState::Buffering) {
            let _ = self.flush_buffer(now);
        } else if let BurstState::Pending { .. } = &self.state {
            // Drop the held char -- non-char input breaks any potential burst.
            self.state = BurstState::Idle;
        }
        self.last_char_time = None;
        self.recent_passthrough.clear();
    }

    /// Drain the buffer and transition to Idle. If the buffer meets the
    /// minimum burst length, it activates the enter suppression window.
    /// Returns the buffer contents (may be empty).
    fn flush_buffer(&mut self, now: Instant) -> String {
        let text = std::mem::take(&mut self.buffer);
        self.state = BurstState::Idle;
        self.recent_passthrough.clear();
        if text.chars().count() >= MIN_BURST_LEN {
            self.enter_suppress_until = Some(now + ENTER_SUPPRESS_WINDOW);
        }
        text
    }

    fn push_recent_passthrough(&mut self, ch: char, now: Instant) {
        self.recent_passthrough.push_back((ch, now));
        while self.recent_passthrough.len() > 2 {
            let _ = self.recent_passthrough.pop_front();
        }
    }

    fn collect_retro_prefix(&self, now: Instant) -> Vec<char> {
        let mut rev = Vec::with_capacity(self.recent_passthrough.len());
        let mut prev_time = now;
        for (ch, at) in self.recent_passthrough.iter().rev() {
            if prev_time.saturating_duration_since(*at) <= CHAR_INTERVAL {
                rev.push(*ch);
                prev_time = *at;
            } else {
                break;
            }
        }
        rev.reverse();
        rev
    }
}

impl Default for PasteBurstDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fast(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn after_idle(base: Instant) -> Instant {
        base + IDLE_TIMEOUT + Duration::from_millis(10)
    }

    #[test]
    fn single_char_passes_through() {
        let mut d = PasteBurstDetector::new();
        let now = Instant::now();
        let action = d.on_char('a', now);
        assert_eq!(action, CharAction::Passthrough('a'));
    }

    #[test]
    fn two_fast_chars_start_buffering() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        // First char passes through (no prior timing reference).
        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));

        // Second char within interval is held (Pending).
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);

        // Third char within interval promotes to Buffering.
        let t2 = fast(t1, 2);
        assert_eq!(d.on_char('c', t2), CharAction::RetroCapture(1));

        assert!(d.is_buffering());
    }

    #[test]
    fn burst_flushes_on_idle_timeout() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);
        let t2 = fast(t1, 2);
        assert_eq!(d.on_char('c', t2), CharAction::RetroCapture(1));
        let t3 = fast(t2, 2);
        assert_eq!(d.on_char('d', t3), CharAction::Consumed);

        // Not yet timed out.
        let t4 = fast(t3, 2);
        assert!(d.tick(t4).is_none());

        // After idle timeout.
        let t5 = after_idle(t3);
        let flush = d.tick(t5);
        assert_eq!(flush, Some(FlushAction::EmitPaste("abcd".to_owned())));
    }

    #[test]
    fn pending_char_emitted_on_timeout() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);

        // Timeout without a third char.
        let t2 = after_idle(t1);
        assert_eq!(d.tick(t2), Some(FlushAction::EmitChar('b')));
    }

    #[test]
    fn enter_suppressed_after_burst() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        assert_eq!(d.on_char('b', fast(t0, 2)), CharAction::Consumed);
        assert_eq!(d.on_char('c', fast(t0, 4)), CharAction::RetroCapture(1));
        for i in 3_u8..=4 {
            let t = fast(t0, u64::from(i) * 2);
            assert_eq!(d.on_char(char::from(b'b' + i - 1), t), CharAction::Consumed);
        }

        // Flush the burst.
        let t_flush = after_idle(t0);
        let flush = d.tick(t_flush);
        assert!(matches!(flush, Some(FlushAction::EmitPaste(_))));

        // Enter within suppression window is suppressed.
        let t_enter = fast(t_flush, 10);
        assert!(d.should_suppress_enter(t_enter));

        // Enter after suppression window is not suppressed.
        let t_late = fast(t_flush, 150);
        assert!(!d.should_suppress_enter(t_late));
    }

    #[test]
    fn enter_during_buffering_appends_newline() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);
        let t2 = fast(t1, 2);
        assert_eq!(d.on_char('c', t2), CharAction::RetroCapture(1));

        // Enter during buffering.
        let t3 = fast(t2, 2);
        assert!(d.on_enter(t3));
        assert!(d.is_buffering());

        // Continue buffering after Enter.
        let t4 = fast(t3, 2);
        assert_eq!(d.on_char('d', t4), CharAction::Consumed);

        // Flush.
        let t5 = after_idle(t4);
        let flush = d.tick(t5);
        assert_eq!(flush, Some(FlushAction::EmitPaste("abc\nd".to_owned())));
    }

    #[test]
    fn non_char_key_resets_state() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);

        // Non-char key resets.
        let t2 = fast(t1, 2);
        d.on_non_char_key(t2);
        assert!(!d.is_buffering());
    }

    #[test]
    fn slow_typing_never_triggers_burst() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        for i in 0_u8..10 {
            let t = fast(t0, u64::from(i) * 200); // 200ms apart = human typing.
            let ch = char::from(b'a' + (i % 26));
            assert_eq!(d.on_char(ch, t), CharAction::Passthrough(ch));
        }
        assert!(!d.is_buffering());
    }

    #[test]
    fn sub_threshold_burst_emits_chars_not_paste() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        // Only 2 fast chars (below MIN_BURST_LEN of 3).
        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed); // held in Pending

        // Third char is slow.
        let t2 = fast(t1, 200);
        // 'b' gets emitted as passthrough, 'c' becomes the new pending hold.
        assert_eq!(d.on_char('c', t2), CharAction::Passthrough('b'));
    }

    #[test]
    fn enter_in_pending_promotes_to_buffering() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);

        // Enter while Pending -> promotes to Buffering with held_char + newline.
        let t2 = fast(t1, 2);
        assert!(d.on_enter(t2));
        assert!(d.is_buffering());

        // Flush to verify buffer contents.
        let t3 = after_idle(t2);
        let flush = d.tick(t3);
        assert_eq!(flush, Some(FlushAction::EmitPaste("b\n".to_owned())));
    }

    #[test]
    fn retro_capture_first_char_on_burst_confirm() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 2);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);
        let t2 = fast(t1, 2);
        assert_eq!(d.on_char('c', t2), CharAction::RetroCapture(1));

        let t3 = after_idle(t2);
        assert_eq!(d.tick(t3), Some(FlushAction::EmitPaste("abc".to_owned())));
    }

    #[cfg(windows)]
    #[test]
    fn windows_slower_burst_still_detected() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 20);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);
        let t2 = fast(t1, 20);
        assert_eq!(d.on_char('c', t2), CharAction::RetroCapture(1));
        let t3 = fast(t2, 25);
        assert_eq!(d.on_char('d', t3), CharAction::Consumed);

        let t4 = fast(t3, 80);
        assert_eq!(d.tick(t4), Some(FlushAction::EmitPaste("abcd".to_owned())));
    }

    #[cfg(windows)]
    #[test]
    fn windows_buffering_gap_does_not_drop_text() {
        let mut d = PasteBurstDetector::new();
        let t0 = Instant::now();

        assert_eq!(d.on_char('a', t0), CharAction::Passthrough('a'));
        let t1 = fast(t0, 20);
        assert_eq!(d.on_char('b', t1), CharAction::Consumed);
        let t2 = fast(t1, 20);
        assert_eq!(d.on_char('c', t2), CharAction::RetroCapture(1));

        // Gap above CHAR_INTERVAL but below IDLE_TIMEOUT should stay in burst.
        let t3 = fast(t2, 40);
        assert_eq!(d.on_char('d', t3), CharAction::Consumed);

        let t4 = fast(t3, 80);
        assert_eq!(d.tick(t4), Some(FlushAction::EmitPaste("abcd".to_owned())));
    }
}
