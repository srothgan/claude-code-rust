// SPDX-License-Identifier: Apache-2.0

use std::ops::Range;

pub const DEFAULT_CACHE_SPLIT_SOFT_LIMIT_BYTES: usize = 1536;
pub const DEFAULT_CACHE_SPLIT_HARD_LIMIT_BYTES: usize = 4096;
pub const DEFAULT_TOOL_PREVIEW_LIMIT_BYTES: usize = 2048;

#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSplitPolicy {
    pub soft_limit_bytes: usize,
    pub hard_limit_bytes: usize,
    pub preview_limit_bytes: usize,
}

impl Default for CacheSplitPolicy {
    fn default() -> Self {
        Self {
            soft_limit_bytes: DEFAULT_CACHE_SPLIT_SOFT_LIMIT_BYTES,
            hard_limit_bytes: DEFAULT_CACHE_SPLIT_HARD_LIMIT_BYTES,
            preview_limit_bytes: DEFAULT_TOOL_PREVIEW_LIMIT_BYTES,
        }
    }
}

#[must_use]
pub fn default_cache_split_policy() -> &'static CacheSplitPolicy {
    static POLICY: CacheSplitPolicy = CacheSplitPolicy {
        soft_limit_bytes: DEFAULT_CACHE_SPLIT_SOFT_LIMIT_BYTES,
        hard_limit_bytes: DEFAULT_CACHE_SPLIT_HARD_LIMIT_BYTES,
        preview_limit_bytes: DEFAULT_TOOL_PREVIEW_LIMIT_BYTES,
    };
    &POLICY
}

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSplitKind {
    Generic,
    ParagraphBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSplitDecision {
    pub split_at: usize,
    pub kind: TextSplitKind,
}

#[must_use]
pub fn find_text_split(text: &str, policy: CacheSplitPolicy) -> Option<TextSplitDecision> {
    let bytes = text.as_bytes();
    let protected_ranges = crate::markdown::table_and_code_ranges(text);
    let mut i = 0usize;

    let mut soft_newline = None;
    let mut soft_sentence = None;
    let mut hard_newline = None;
    let mut hard_sentence = None;
    let mut post_hard_newline = None;
    let mut post_hard_sentence = None;

    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
            let split_at = i + 2;
            if split_inside_markdown_block(split_at, &protected_ranges) {
                i += 1;
                continue;
            }
            if split_at < bytes.len() {
                return Some(TextSplitDecision {
                    split_at,
                    kind: TextSplitKind::ParagraphBoundary,
                });
            }
            return None;
        }

        if bytes[i] == b'\n' && !split_inside_markdown_block(i + 1, &protected_ranges) {
            track_text_split_candidate(
                i + 1,
                &policy,
                &mut soft_newline,
                &mut hard_newline,
                &mut post_hard_newline,
            );
        }

        if is_sentence_boundary(bytes, i) && !split_inside_markdown_block(i + 1, &protected_ranges)
        {
            track_text_split_candidate(
                i + 1,
                &policy,
                &mut soft_sentence,
                &mut hard_sentence,
                &mut post_hard_sentence,
            );
        }
        i += 1;
    }

    if bytes.len() >= policy.soft_limit_bytes
        && let Some(split_at) = pick_text_split_candidate(soft_newline, soft_sentence)
        && split_at < bytes.len()
    {
        return Some(TextSplitDecision { split_at, kind: TextSplitKind::Generic });
    }

    if bytes.len() >= policy.hard_limit_bytes
        && let Some(split_at) =
            hard_newline.or(post_hard_newline).or(hard_sentence).or(post_hard_sentence)
        && split_at < bytes.len()
    {
        return Some(TextSplitDecision { split_at, kind: TextSplitKind::Generic });
    }

    None
}

fn track_text_split_candidate(
    split_at: usize,
    policy: &CacheSplitPolicy,
    soft_slot: &mut Option<usize>,
    hard_slot: &mut Option<usize>,
    post_hard_slot: &mut Option<usize>,
) {
    if split_at <= policy.soft_limit_bytes {
        *soft_slot = Some(split_at);
    }
    if split_at <= policy.hard_limit_bytes {
        *hard_slot = Some(split_at);
    } else if post_hard_slot.is_none() {
        *post_hard_slot = Some(split_at);
    }
}

fn pick_text_split_candidate(newline: Option<usize>, sentence: Option<usize>) -> Option<usize> {
    newline.or(sentence)
}

fn is_sentence_boundary(bytes: &[u8], i: usize) -> bool {
    matches!(bytes[i], b'.' | b'!' | b'?')
        && (i + 1 == bytes.len() || matches!(bytes[i + 1], b' ' | b'\t' | b'\r' | b'\n'))
}

fn split_inside_markdown_block(split_at: usize, ranges: &[Range<usize>]) -> bool {
    ranges.iter().any(|range| split_at > range.start && split_at < range.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_source_split(
        source: &str,
        policy: CacheSplitPolicy,
        expected: Option<(&str, &str, TextSplitKind)>,
    ) {
        match expected {
            None => assert_eq!(find_text_split(source, policy), None, "protected source: {source}"),
            Some((prefix, suffix, kind)) => {
                assert_eq!(source, format!("{prefix}{suffix}"));
                let decision = find_text_split(source, policy).expect("eligible source boundary");
                assert_eq!(decision.kind, kind);
                assert_eq!(decision.split_at, prefix.len());
                assert!(source.is_char_boundary(decision.split_at));
                assert!(!prefix.is_empty() && !suffix.is_empty());
                assert_eq!(&source[..decision.split_at], prefix);
                assert_eq!(&source[decision.split_at..], suffix);
            }
        }
    }

    #[test]
    fn split_preserves_table_interiors_and_releases_independent_source_boundaries() {
        let small = CacheSplitPolicy {
            soft_limit_bytes: 12,
            hard_limit_bytes: 24,
            ..CacheSplitPolicy::default()
        };
        let default = *default_cache_split_policy();
        // A single long cell supplies just enough source to cross the deployed hard limit.
        let long = format!(
            "| Hassle | Report | Ref |\n| --- | --- | --- |\n| Slow startup | {} | reference |\n",
            "x".repeat(default.hard_limit_bytes)
        );
        let empty_url = format!(
            "| Name | Empty | URL |\n| --- | --- | --- |\n| Row |  | https://example.com/{} |\n",
            "x".repeat(default.hard_limit_bytes)
        );
        for (table, policy) in [
            ("Name | Value\n- | -\nfirst | one\nsecond | two\n", small),
            ("Name | Value\n--- | ---\nfirst\nsecond\nthird\n", small),
            ("| Hassle | Report |\n| --- | --- |\n| Slow startup | Delayed echo. |\n", small),
            (long.as_str(), default),
            (empty_url.as_str(), default),
        ] {
            assert!(table.len() > policy.hard_limit_bytes);
            assert_source_split(table, policy, None);
            let prefix = format!("{table}\n");
            assert_source_split(&prefix, policy, None); // No empty remainder at a final separator.
            let suffix = "Next paragraph.";
            assert_source_split(
                &format!("{prefix}{suffix}"),
                policy,
                Some((&prefix, suffix, TextSplitKind::ParagraphBoundary)),
            );
        }
        let prefix = "One | Two\n";
        let suffix = "--- | --- | ---\nfirst | second | third\nlast | row | here";
        let policy = CacheSplitPolicy {
            soft_limit_bytes: prefix.len(),
            hard_limit_bytes: prefix.len() + suffix.len(),
            ..small
        };
        assert_source_split(
            &format!("{prefix}{suffix}"),
            policy,
            Some((prefix, suffix, TextSplitKind::Generic)),
        );
    }

    #[test]
    fn split_keeps_table_like_code_in_its_parsed_fence() {
        let policy = CacheSplitPolicy {
            soft_limit_bytes: 12,
            hard_limit_bytes: 24,
            ..CacheSplitPolicy::default()
        };
        for (open, close) in [
            ("~~~md\nName | Value\n- | -\n\nfirst | second\n", "~~~"),
            ("````md\n```\nName | Value\n- | -\n\nfirst | second\n", "````"),
        ] {
            assert!(open.len() > policy.hard_limit_bytes);
            assert_source_split(open, policy, None);
            assert_source_split(&format!("{open}still inside"), policy, None);
            let closed = format!("{open}{close}");
            assert_source_split(&closed, policy, None);
            let prefix = format!("{closed}\n\n");
            assert_source_split(&prefix, policy, None);
            let suffix = "Following paragraph.";
            assert_source_split(
                &format!("{prefix}{suffix}"),
                policy,
                Some((&prefix, suffix, TextSplitKind::ParagraphBoundary)),
            );
        }
    }
}
