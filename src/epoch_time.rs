// SPDX-License-Identifier: Apache-2.0

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Persisted Claude timestamps use seconds, or milliseconds for modern 13-digit values.
pub(crate) fn parse_value(value: &serde_json::Value) -> Option<SystemTime> {
    match value {
        serde_json::Value::Number(number) => number.as_i64().and_then(from_epoch),
        serde_json::Value::String(raw) => parse_str(raw),
        _ => None,
    }
}

pub(crate) fn parse_str(raw: &str) -> Option<SystemTime> {
    raw.trim().parse::<i64>().ok().and_then(from_epoch)
}

fn from_epoch(raw: i64) -> Option<SystemTime> {
    let raw = u64::try_from(raw).ok()?;
    let duration = if raw >= 1_000_000_000_000 {
        Duration::from_millis(raw)
    } else {
        Duration::from_secs(raw)
    };
    UNIX_EPOCH.checked_add(duration)
}
