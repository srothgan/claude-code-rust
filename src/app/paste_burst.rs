// SPDX-License-Identifier: Apache-2.0

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
#[cfg(not(windows))]
const ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(100);

#[cfg(windows)]
const ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(250);

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
    /// A false alarm released a held character. Insert it before the current key, even if equal.
    ReleaseHeld(char),
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
            BurstState::Pending { held_char, received_at, retro_prefix } => {
                let within_pending_window =
                    now.saturating_duration_since(*received_at) <= IDLE_TIMEOUT;
                if is_fast || within_pending_window {
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
                    tracing::debug!(
                        target: crate::logging::targets::APP_PASTE,
                        event_name = "paste_burst_detected",
                        message = "paste burst detected",
                        outcome = "start",
                        retro_len,
                        buffer_chars = self.buffer.chars().count(),
                        within_pending_window,
                    );
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
                    CharAction::ReleaseHeld(prev)
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
    /// as a newline (routed through the paste buffer) rather than a submit action.
    ///
    /// This covers two cases:
    /// 1. Enter arrives while actively buffering a burst (append newline).
    /// 2. Enter arrives within the post-burst suppression window.
    pub fn on_enter(&mut self, now: Instant) -> bool {
        match &self.state {
            BurstState::Buffering => {
                self.buffer.push('\n');
                self.last_char_time = Some(now);
                tracing::debug!(
                    target: crate::logging::targets::APP_PASTE,
                    event_name = "paste_enter_buffered",
                    message = "enter routed into the active paste burst buffer",
                    outcome = "success",
                    reason = "buffering",
                    buffer_chars = self.buffer.chars().count(),
                );
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
                tracing::debug!(
                    target: crate::logging::targets::APP_PASTE,
                    event_name = "paste_enter_buffered",
                    message = "enter promoted a pending paste burst into buffering",
                    outcome = "success",
                    reason = "pending",
                    buffer_chars = self.buffer.chars().count(),
                );
                true
            }
            BurstState::Idle if self.should_suppress_enter(now) => {
                self.buffer.clear();
                self.buffer.push('\n');
                self.state = BurstState::Buffering;
                self.last_char_time = Some(now);
                tracing::debug!(
                    target: crate::logging::targets::APP_PASTE,
                    event_name = "paste_enter_buffered",
                    message = "enter suppressed and queued as a paste newline",
                    outcome = "success",
                    reason = "suppression_window",
                    buffer_chars = self.buffer.chars().count(),
                );
                true
            }
            BurstState::Idle => false,
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
                    if text.is_empty() {
                        None
                    } else {
                        tracing::debug!(
                            target: crate::logging::targets::APP_PASTE,
                            event_name = "paste_burst_flushed",
                            message = "paste burst flushed after idle timeout",
                            outcome = "success",
                            pasted_chars = text.chars().count(),
                        );
                        Some(FlushAction::EmitPaste(text))
                    }
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

    /// Whether accepted characters are still held outside the visible editor.
    #[must_use]
    pub fn has_held_input(&self) -> bool {
        matches!(self.state, BurstState::Buffering | BurstState::Pending { .. })
    }

    /// Finish buffered text before a non-character key breaks the burst.
    /// The caller must apply the returned input rather than discarding it.
    pub fn on_non_char_key(&mut self, now: Instant) -> Option<FlushAction> {
        let action = match self.state {
            BurstState::Buffering => Some(FlushAction::EmitPaste(self.flush_buffer(now))),
            BurstState::Pending { held_char, .. } => {
                self.state = BurstState::Idle;
                Some(FlushAction::EmitChar(held_char))
            }
            BurstState::Idle => None,
        };
        self.last_char_time = None;
        self.recent_passthrough.clear();
        action
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
    fn bursts_deliver_complete_unicode_source_once_and_allow_reuse() {
        let cases = [
            ("\u{e9}\u{301}\u{754c}Z", [0, 2, 4, 6]),
            #[cfg(windows)]
            ("abcd", [0, 20, 40, 65]),
            #[cfg(windows)]
            ("abcd", [0, 20, 40, 80]),
            #[cfg(windows)]
            ("abcd", [0, 20, 60, 80]),
        ];
        for (source, times) in cases {
            let mut detector = PasteBurstDetector::new();
            let start = Instant::now();
            for (i, ch) in source.chars().enumerate() {
                let expected = match i {
                    0 => CharAction::Passthrough(ch),
                    1 | 3 => CharAction::Consumed,
                    2 => CharAction::RetroCapture(1),
                    _ => unreachable!(),
                };
                assert_eq!(detector.on_char(ch, fast(start, times[i])), expected);
            }
            let end = fast(start, times[3]);
            assert_eq!(detector.tick(end + IDLE_TIMEOUT), None);
            let flushed = end + IDLE_TIMEOUT + Duration::from_nanos(1);
            assert_eq!(detector.tick(flushed), Some(FlushAction::EmitPaste(source.to_owned())));
            assert_eq!(detector.tick(flushed + IDLE_TIMEOUT), None);
            let later = flushed + Duration::from_secs(1);
            assert_eq!(detector.on_char('X', later), CharAction::Passthrough('X'));
            assert_eq!(detector.on_char('Y', fast(later, 2)), CharAction::Consumed);
            assert_eq!(detector.on_char('Z', fast(later, 4)), CharAction::RetroCapture(1));
            assert_eq!(
                detector.tick(after_idle(fast(later, 4))),
                Some(FlushAction::EmitPaste("XYZ".to_owned()))
            );
        }
    }

    #[test]
    fn ordinary_typing_and_expired_pending_input_never_replay() {
        let mut detector = PasteBurstDetector::new();
        let start = Instant::now();
        for (i, ch) in "a\u{754c}b".chars().enumerate() {
            let at = start + Duration::from_secs(u64::try_from(i).unwrap());
            assert_eq!(detector.on_char(ch, at), CharAction::Passthrough(ch));
            assert_eq!(detector.tick(after_idle(at)), None);
        }
        let later = start + Duration::from_secs(4);
        assert_eq!(detector.on_char('X', later), CharAction::Passthrough('X'));
        let pending = fast(later, 2);
        assert_eq!(detector.on_char('Y', pending), CharAction::Consumed);
        assert_eq!(detector.tick(pending + IDLE_TIMEOUT), None);
        let expired = pending + IDLE_TIMEOUT + Duration::from_nanos(1);
        assert_eq!(detector.tick(expired), Some(FlushAction::EmitChar('Y')));
        assert_eq!(detector.tick(after_idle(expired)), None);
        assert_eq!(
            detector.on_char('Z', expired + Duration::from_secs(1)),
            CharAction::Passthrough('Z')
        );
    }

    #[test]
    fn a_false_alarm_explicitly_releases_the_held_character_before_the_current_key() {
        for current in ['b', 'c'] {
            let mut detector = PasteBurstDetector::new();
            let start = Instant::now();
            assert_eq!(detector.on_char('a', start), CharAction::Passthrough('a'));
            assert_eq!(detector.on_char('b', fast(start, 2)), CharAction::Consumed);
            let late = start + Duration::from_secs(1);
            assert_eq!(detector.on_char(current, late), CharAction::ReleaseHeld('b'));
            assert_eq!(detector.tick(after_idle(late)), None);
            assert_eq!(detector.on_non_char_key(after_idle(late)), None);
        }
    }

    #[test]
    fn enter_suppression_delivers_newlines_only_through_its_original_deadline() {
        for (offset, suppressed) in [
            (Duration::from_millis(1), true),
            (ENTER_SUPPRESS_WINDOW, true),
            (ENTER_SUPPRESS_WINDOW + Duration::from_nanos(1), false),
        ] {
            let mut detector = PasteBurstDetector::new();
            let start = Instant::now();
            assert_eq!(detector.on_char('a', start), CharAction::Passthrough('a'));
            assert_eq!(detector.on_char('b', fast(start, 2)), CharAction::Consumed);
            assert_eq!(detector.on_char('c', fast(start, 4)), CharAction::RetroCapture(1));
            let flushed = after_idle(fast(start, 4));
            assert_eq!(detector.tick(flushed), Some(FlushAction::EmitPaste("abc".into())));
            assert_eq!(detector.on_enter(flushed + offset), suppressed);
            let next = after_idle(flushed + offset);
            assert_eq!(
                detector.tick(next),
                if suppressed { Some(FlushAction::EmitPaste("\n".into())) } else { None }
            );
            // A one-character newline cannot extend the original suppression deadline.
            let after_deadline =
                (flushed + ENTER_SUPPRESS_WINDOW + Duration::from_nanos(1)).max(next);
            assert!(!detector.on_enter(after_deadline));
            assert_eq!(detector.tick(after_deadline + Duration::from_secs(1)), None);
            assert_eq!(
                detector.on_char('Q', after_deadline + Duration::from_secs(1)),
                CharAction::Passthrough('Q')
            );
        }
    }

    #[test]
    fn non_character_boundaries_release_source_once_and_restart_ordinary_typing() {
        for (source, expected) in [
            ("", None),
            ("ab", Some(FlushAction::EmitChar('b'))),
            ("abc", Some(FlushAction::EmitPaste("abc".into()))),
        ] {
            let mut detector = PasteBurstDetector::new();
            let start = Instant::now();
            for (i, ch) in source.chars().enumerate() {
                let _ = detector.on_char(ch, fast(start, u64::try_from(i).unwrap()));
            }
            let boundary = fast(start, 4);
            assert_eq!(detector.on_non_char_key(boundary), expected);
            assert_eq!(detector.on_non_char_key(boundary), None);
            assert_eq!(detector.tick(boundary), None);
            assert_eq!(detector.on_char('X', fast(boundary, 1)), CharAction::Passthrough('X'));
            assert_eq!(detector.tick(after_idle(boundary)), None);
        }
    }

    #[test]
    fn enter_preserves_pending_and_confirmed_source_with_followup_text() {
        for (source, expected) in [("ab", "b\nZ"), ("abc", "abc\nZ")] {
            let mut detector = PasteBurstDetector::new();
            let start = Instant::now();
            assert_eq!(detector.on_char('a', start), CharAction::Passthrough('a'));
            assert_eq!(detector.on_char('b', fast(start, 2)), CharAction::Consumed);
            if source == "abc" {
                assert_eq!(detector.on_char('c', fast(start, 4)), CharAction::RetroCapture(1));
            }
            assert!(detector.on_enter(fast(start, 6)));
            assert_eq!(detector.on_char('Z', fast(start, 7)), CharAction::Consumed);
            let end = after_idle(fast(start, 7));
            assert_eq!(detector.tick(end), Some(FlushAction::EmitPaste(expected.into())));
            assert_eq!(detector.tick(after_idle(end)), None);
        }
    }
}
