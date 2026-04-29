// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Cache observability: accumulation, snapshots, and rate-limited structured logging.
//!
//! `CacheMetrics` lives on `App` and accumulates cross-cutting counters (enforcement
//! counts, watermarks, rate-limit cooldown state). `CacheMetricsSnapshot` is a
//! computed on-demand view that pulls from `HistoryRetentionStats` and `CacheMetrics`.
//!
//! All structured tracing uses `target: "cache"` so it can be enabled via
//! `--log-filter "cache=debug"` without affecting other log targets.

use super::types::{HistoryRetentionPolicy, HistoryRetentionStats};

// ---------------------------------------------------------------------------
// Rate-limit constants
// ---------------------------------------------------------------------------

/// Emit history-retention debug log every N enforcement calls.
const HISTORY_LOG_INTERVAL: u64 = 10;

// ---------------------------------------------------------------------------
// Persistent accumulator (lives on App)
// ---------------------------------------------------------------------------

/// Cross-cutting cache metrics accumulated over the session lifetime.
///
/// Updated by `record_history_enforcement` after each retention pass.
/// Rate-limit cooldown state is internal and not meaningful to external consumers.
#[derive(Debug, Clone, Copy)]
pub struct CacheMetrics {
    pub enforcement_count: u64,
    pub peak_bytes: usize,

    // -- Rate-limit cooldown (private) --
    log_countdown: u64,
}

impl Default for CacheMetrics {
    fn default() -> Self {
        Self {
            enforcement_count: 0,
            peak_bytes: 0,
            // Fire on the very first call (countdown starts at 1 so it
            // decrements to 0 and triggers immediately).
            log_countdown: 1,
        }
    }
}

impl CacheMetrics {
    /// Record one history-retention enforcement pass.
    ///
    /// Returns `true` when a debug-level log should be emitted (every
    /// `HISTORY_LOG_INTERVAL` calls).
    pub fn record_history_enforcement(
        &mut self,
        stats: &HistoryRetentionStats,
        _policy: HistoryRetentionPolicy,
    ) -> bool {
        self.enforcement_count += 1;
        if stats.total_before_bytes > self.peak_bytes {
            self.peak_bytes = stats.total_before_bytes;
        }

        self.log_countdown -= 1;
        if self.log_countdown == 0 {
            self.log_countdown = HISTORY_LOG_INTERVAL;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// On-demand snapshot (not stored)
// ---------------------------------------------------------------------------

/// Point-in-time view of all cache subsystems, computed on demand.
#[derive(Debug, Clone, Copy)]
pub struct CacheMetricsSnapshot {
    pub bytes: usize,
    pub max_bytes: usize,
    pub utilization_pct: f32,
    pub dropped_messages_this_pass: usize,
    pub total_dropped_messages: usize,
    pub total_dropped_bytes: usize,
    pub enforcement_count: u64,
    pub peak_bytes: usize,
}

/// Build a snapshot from all cache subsystem state.
///
/// Only called on log cadence (not every frame), so the cost of collecting
/// fields is negligible.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn build_snapshot(
    retention_stats: &HistoryRetentionStats,
    retention_policy: HistoryRetentionPolicy,
    metrics: &CacheMetrics,
    dropped_this_pass: usize,
) -> CacheMetricsSnapshot {
    let history_util = if retention_policy.max_bytes > 0 {
        (retention_stats.total_after_bytes as f32 / retention_policy.max_bytes as f32) * 100.0
    } else {
        0.0
    };

    CacheMetricsSnapshot {
        bytes: retention_stats.total_after_bytes,
        max_bytes: retention_policy.max_bytes,
        utilization_pct: history_util,
        dropped_messages_this_pass: dropped_this_pass,
        total_dropped_messages: retention_stats.total_dropped_messages,
        total_dropped_bytes: retention_stats.total_dropped_bytes,
        enforcement_count: metrics.enforcement_count,
        peak_bytes: metrics.peak_bytes,
    }
}

// ---------------------------------------------------------------------------
// Structured tracing emitters
// ---------------------------------------------------------------------------

/// Emit a debug-level structured log summarizing history retention state.
pub fn emit_history_metrics(snap: &CacheMetricsSnapshot) {
    tracing::debug!(
        target: crate::logging::targets::APP_CACHE,
        event_name = "history_retention_metrics",
        message = "history retention metrics emitted",
        outcome = "success",
        history_bytes = snap.bytes,
        history_max = snap.max_bytes,
        history_util_pct = format_args!("{:.1}", snap.utilization_pct),
        history_dropped_pass = snap.dropped_messages_this_pass,
        history_dropped_total = snap.total_dropped_messages,
        history_dropped_bytes_total = snap.total_dropped_bytes,
        history_peak = snap.peak_bytes,
        history_enforcements = snap.enforcement_count,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_history_stats(before_bytes: usize, dropped: usize) -> HistoryRetentionStats {
        HistoryRetentionStats {
            total_before_bytes: before_bytes,
            total_after_bytes: before_bytes,
            dropped_messages: dropped,
            dropped_bytes: 0,
            total_dropped_messages: dropped,
            total_dropped_bytes: 0,
        }
    }

    #[test]
    fn history_log_is_rate_limited_after_initial_fire() {
        let mut m = CacheMetrics::default();
        let stats = make_history_stats(2000, 0);
        let policy = HistoryRetentionPolicy::default();

        assert!(m.record_history_enforcement(&stats, policy));
        assert!(!m.record_history_enforcement(&stats, policy));

        let mut fired_again = false;
        for _ in 0..HISTORY_LOG_INTERVAL {
            if m.record_history_enforcement(&stats, policy) {
                fired_again = true;
                break;
            }
        }

        assert!(fired_again, "history log should fire again after some delay");
        assert!(!m.record_history_enforcement(&stats, policy));
        assert!(m.enforcement_count >= 3);
    }

    #[test]
    fn history_peak_bytes_tracks_maximum() {
        let mut m = CacheMetrics::default();
        let policy = HistoryRetentionPolicy::default();

        m.record_history_enforcement(&make_history_stats(10_000, 0), policy);
        assert_eq!(m.peak_bytes, 10_000);

        m.record_history_enforcement(&make_history_stats(5_000, 0), policy);
        assert_eq!(m.peak_bytes, 10_000);
    }

    #[test]
    fn snapshot_utilization_computed_correctly() {
        let retention_stats =
            HistoryRetentionStats { total_after_bytes: 750, ..Default::default() };
        let policy = HistoryRetentionPolicy { max_bytes: 1000 };
        let metrics = CacheMetrics::default();
        let snap = build_snapshot(&retention_stats, policy, &metrics, 1);

        assert!((snap.utilization_pct - 75.0).abs() < 0.01);
        assert_eq!(snap.dropped_messages_this_pass, 1);
    }

    #[test]
    fn snapshot_zero_budget_no_panic() {
        let policy = HistoryRetentionPolicy { max_bytes: 0 };
        let metrics = CacheMetrics::default();
        let snap = build_snapshot(&HistoryRetentionStats::default(), policy, &metrics, 0);
        assert!(snap.utilization_pct.abs() < f32::EPSILON);
    }
}
