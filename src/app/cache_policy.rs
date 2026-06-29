// Copyright 2025 Simon Peter Rothgang
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
    let table_ranges = markdown_table_ranges(text);
    let mut in_fence = false;
    let mut i = 0usize;

    let mut soft_newline = None;
    let mut soft_sentence = None;
    let mut hard_newline = None;
    let mut hard_sentence = None;
    let mut post_hard_newline = None;
    let mut post_hard_sentence = None;

    while i < bytes.len() {
        if (i == 0 || bytes[i - 1] == b'\n') && bytes[i..].starts_with(b"```") {
            in_fence = !in_fence;
        }

        if !in_fence {
            if i + 1 < bytes.len() && bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
                let split_at = i + 2;
                if split_inside_markdown_table(split_at, &table_ranges) {
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

            if bytes[i] == b'\n' && !split_inside_markdown_table(i + 1, &table_ranges) {
                track_text_split_candidate(
                    i + 1,
                    &policy,
                    &mut soft_newline,
                    &mut hard_newline,
                    &mut post_hard_newline,
                );
            }

            if is_sentence_boundary(bytes, i) && !split_inside_markdown_table(i + 1, &table_ranges)
            {
                track_text_split_candidate(
                    i + 1,
                    &policy,
                    &mut soft_sentence,
                    &mut hard_sentence,
                    &mut post_hard_sentence,
                );
            }
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

#[must_use]
pub fn find_text_split_index(text: &str, policy: CacheSplitPolicy) -> Option<usize> {
    find_text_split(text, policy).map(|decision| decision.split_at)
}

#[must_use]
pub(crate) fn markdown_table_tail_is_open(text: &str) -> bool {
    markdown_table_ranges(text).last().is_some_and(|range| range.end == text.len())
}

#[must_use]
pub(crate) fn starts_with_markdown_table_row(text: &str) -> bool {
    let first_line = text.split_inclusive('\n').next().unwrap_or(text);
    markdown_table_row_cells(line_content(first_line)).is_some()
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

fn split_inside_markdown_table(split_at: usize, ranges: &[Range<usize>]) -> bool {
    ranges.iter().any(|range| split_at > range.start && split_at < range.end)
}

#[derive(Debug, Clone, Copy)]
struct MarkdownLine<'a> {
    start: usize,
    end: usize,
    content: &'a str,
}

fn markdown_table_ranges(text: &str) -> Vec<Range<usize>> {
    let lines = markdown_lines(text);
    let mut ranges = Vec::new();
    let mut in_fence = false;
    let mut idx = 0usize;

    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.content.trim();
        if markdown_fence_line(trimmed) {
            in_fence = !in_fence;
            idx += 1;
            continue;
        }

        if !in_fence
            && idx + 1 < lines.len()
            && markdown_table_row_cells(line.content).is_some()
            && markdown_table_delimiter_cells(lines[idx + 1].content).is_some()
        {
            let start = line.start;
            let mut end = lines[idx + 1].end;
            let mut next_idx = idx + 2;
            while next_idx < lines.len() {
                let next = lines[next_idx];
                let next_trimmed = next.content.trim();
                if next_trimmed.is_empty() || markdown_fence_line(next_trimmed) {
                    break;
                }
                if markdown_table_row_cells(next.content).is_none() {
                    break;
                }
                end = next.end;
                next_idx += 1;
            }
            ranges.push(start..end);
            idx = next_idx;
            continue;
        }

        idx += 1;
    }

    ranges
}

fn markdown_lines(text: &str) -> Vec<MarkdownLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for raw in text.split_inclusive('\n') {
        let end = start + raw.len();
        lines.push(MarkdownLine { start, end, content: line_content(raw) });
        start = end;
    }
    if start < text.len() {
        lines.push(MarkdownLine { start, end: text.len(), content: line_content(&text[start..]) });
    }
    lines
}

fn line_content(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

fn markdown_fence_line(trimmed: &str) -> bool {
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn markdown_table_row_cells(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.contains('|') {
        return None;
    }

    let without_leading = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let without_outer = without_leading.strip_suffix('|').unwrap_or(without_leading);
    Some(without_outer.split('|').map(str::trim).collect())
}

fn markdown_table_delimiter_cells(line: &str) -> Option<Vec<&str>> {
    let cells = markdown_table_row_cells(line)?;
    (!cells.is_empty() && cells.iter().all(|cell| markdown_table_delimiter_cell(cell)))
        .then_some(cells)
}

fn markdown_table_delimiter_cell(cell: &str) -> bool {
    let trimmed = cell.trim();
    let without_leading_align = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let core = without_leading_align.strip_suffix(':').unwrap_or(without_leading_align);
    core.len() >= 3 && core.bytes().all(|byte| byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn split_prefers_double_newline() {
        let text = "first\n\nsecond";
        let split_at = find_text_split_index(text, *default_cache_split_policy());
        assert_eq!(split_at, Some("first\n\n".len()));
    }

    #[test]
    fn split_respects_soft_limit() {
        let policy = *default_cache_split_policy();
        let prefix = "a".repeat(policy.soft_limit_bytes - 1);
        let text = format!("{prefix}\nsecond line");
        let split_at = find_text_split_index(&text, policy).expect("expected split");
        assert_eq!(&text[..split_at], format!("{prefix}\n"));
    }

    #[test]
    fn split_ignores_double_newline_inside_fence() {
        let text = "```rust\nfirst\n\nsecond\n```";
        assert!(find_text_split_index(text, *default_cache_split_policy()).is_none());
    }

    #[test]
    fn split_does_not_cut_inside_long_markdown_table() {
        let policy = *default_cache_split_policy();
        let mut rows = String::new();
        for idx in 0..80 {
            let _ = writeln!(
                rows,
                "| Slow startup {idx} | Users report delayed echo once context is large. | https://example.com/issues/{idx}?query=very-long-value |"
            );
        }
        let text = format!("| Hassle | Report | Ref |\n| --- | --- | --- |\n{rows}");

        assert!(text.len() > policy.hard_limit_bytes);
        assert!(find_text_split_index(&text, policy).is_none());
    }

    #[test]
    fn split_allows_boundary_after_table_closing_blank_line() {
        let policy = CacheSplitPolicy {
            soft_limit_bytes: 32,
            hard_limit_bytes: 256,
            preview_limit_bytes: DEFAULT_TOOL_PREVIEW_LIMIT_BYTES,
        };
        let text = "| Hassle | Report |\n| --- | --- |\n| Slow startup | Delayed echo. |\n\nNext paragraph.";

        let split_at = find_text_split_index(text, policy);

        assert_eq!(
            split_at,
            Some("| Hassle | Report |\n| --- | --- |\n| Slow startup | Delayed echo. |\n\n".len())
        );
    }

    #[test]
    fn ordinary_paragraphs_still_split_at_soft_newline() {
        let policy = CacheSplitPolicy {
            soft_limit_bytes: 32,
            hard_limit_bytes: 256,
            preview_limit_bytes: DEFAULT_TOOL_PREVIEW_LIMIT_BYTES,
        };
        let prefix = "ordinary paragraph line";
        let text = format!("{prefix}\nnext line keeps streaming");

        let split_at = find_text_split_index(&text, policy).expect("expected soft split");

        assert_eq!(&text[..split_at], format!("{prefix}\n"));
    }

    #[test]
    fn table_detection_supports_empty_cells_and_long_url_cells() {
        let policy = *default_cache_split_policy();
        let mut rows = String::new();
        for idx in 0..60 {
            let _ = writeln!(
                rows,
                "| Row {idx} |  | https://example.com/articles/{idx}/with/a/very/long/path?alpha=beta&gamma=delta |"
            );
        }
        let text = format!("| Name | Empty | URL |\n| --- | --- | --- |\n{rows}");

        assert!(text.len() > policy.hard_limit_bytes);
        assert!(markdown_table_tail_is_open(&text));
        assert!(find_text_split_index(&text, policy).is_none());
    }
}
