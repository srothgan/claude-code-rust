// SPDX-License-Identifier: Apache-2.0

//! The rule above the composer that carries the session's title in the shape
//! of Claude Code's named prompt bar: `──── name ─`. The title comes from the
//! Agent SDK, which also reports Claude Code's generated title, so an unnamed
//! session shows a rule after its first turn where Claude Code shows none.

use super::theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const RULE: &str = "─";
const ELLIPSIS: char = '…';
/// One rule cell before the name and one after it, as stock leaves.
const MIN_EDGE_CELLS: usize = 2;
/// The spaces either side of the name.
const NAME_PADDING_CELLS: usize = 2;

/// The rule row for a named session; `None` keeps the composer exactly as it
/// is for a session without a name.
pub(crate) fn session_rule_line(title: Option<&str>, width: u16) -> Option<Line<'static>> {
    let title = title?;
    let width = usize::from(width);
    let rule_style = Style::default().fg(theme::DIM);

    let name_budget = width.saturating_sub(MIN_EDGE_CELLS + NAME_PADDING_CELLS);
    let name = truncate_to_width(title, name_budget);
    if name.is_empty() {
        return Some(Line::from(Span::styled(RULE.repeat(width), rule_style)));
    }
    let label = format!(" {name} ");
    let lead = width.saturating_sub(label.width() + 1);
    Some(Line::from(vec![
        Span::styled(RULE.repeat(lead), rule_style),
        Span::raw(label),
        Span::styled(RULE, rule_style),
    ]))
}

/// The name cut to `max_cells` display cells, ending in `…` when cut.
fn truncate_to_width(name: &str, max_cells: usize) -> String {
    let name = name.trim();
    if name.width() <= max_cells {
        return name.to_owned();
    }
    if max_cells == 0 {
        return String::new();
    }
    let mut kept = String::new();
    let mut used = 0;
    for ch in name.chars() {
        let cells = ch.width().unwrap_or(0);
        if used + cells + 1 > max_cells {
            break;
        }
        kept.push(ch);
        used += cells;
    }
    kept.push(ELLIPSIS);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn no_rule_without_a_name() {
        assert!(session_rule_line(None, 80).is_none());
    }

    #[test]
    fn name_sits_right_aligned_in_a_full_width_rule() {
        let line = session_rule_line(Some("probe-e2e"), 30).expect("rule");
        assert_eq!(text(&line), format!("{} probe-e2e ─", "─".repeat(18)));
        assert_eq!(text(&line).width(), 30);
    }

    #[test]
    fn a_long_name_is_cut_with_an_ellipsis_and_never_wraps() {
        for width in [4_u16, 5, 8, 12, 20] {
            let line = session_rule_line(Some("a-very-long-session-name"), width).expect("rule");
            assert_eq!(text(&line).width(), usize::from(width), "width {width}");
        }
        let line = session_rule_line(Some("a-very-long-session-name"), 12).expect("rule");
        assert_eq!(text(&line), "─ a-very-… ─");
        let wide = session_rule_line(Some("名前のセッション"), 12).expect("rule");
        assert_eq!(text(&wide).width(), 12);
        assert!(text(&wide).contains('…'));
    }
}
