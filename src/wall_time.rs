// SPDX-License-Identifier: Apache-2.0

//! RFC 3339 is the wire format; Jiff timestamps own runtime wall-clock instants.

pub(crate) fn parse_rfc3339(value: &str) -> Option<jiff::Timestamp> {
    let value =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()?;
    jiff::Timestamp::from_nanosecond(value.unix_timestamp_nanos()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_timestamp_requires_rfc3339_and_preserves_offset_and_fraction() {
        assert_eq!(
            parse_rfc3339("2026-10-25T02:30:00.123456789+02:00").unwrap().to_string(),
            "2026-10-25T00:30:00.123456789Z"
        );
        for invalid in [
            "2026-10-25",
            "20261025T023000Z",
            "2026-10-25T02:30:00Z[Europe/Berlin]",
            "2026-02-30T00:00:00Z",
        ] {
            assert!(parse_rfc3339(invalid).is_none(), "{invalid}");
        }
    }
}
