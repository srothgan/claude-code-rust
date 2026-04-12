// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use ratatui::layout::{Constraint, Layout, Rect};

pub struct AppLayout {
    pub body: Rect,
    pub input_sep: Rect,
    /// Area for the todo panel (zero-height when hidden or no todos).
    /// Positioned below the input top separator and above the input field.
    pub todo: Rect,
    pub input: Rect,
    pub input_bottom_sep: Rect,
    pub help: Rect,
    pub footer: Option<Rect>,
}

pub fn compute(area: Rect, input_lines: u16, todo_height: u16, help_height: u16) -> AppLayout {
    let input_height = input_lines.max(1);

    if area.height < 8 {
        // Ultra-compact: no footer, no todo
        let [body, input, input_bottom_sep, help] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(input_height),
            Constraint::Length(1),
            Constraint::Length(help_height),
        ])
        .areas(area);
        AppLayout {
            body,
            todo: Rect::new(area.x, input.y, area.width, 0),
            input_sep: Rect::new(area.x, input.y, area.width, 0),
            input,
            input_bottom_sep,
            help,
            footer: None,
        }
    } else {
        let [body, input_sep, todo, input, input_bottom_sep, help, footer] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(todo_height),
            Constraint::Length(input_height),
            Constraint::Length(1),
            Constraint::Length(help_height),
            Constraint::Length(2),
        ])
        .areas(area);
        AppLayout { body, input_sep, todo, input, input_bottom_sep, help, footer: Some(footer) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    /// Sum all layout area heights (handles optional footer).
    fn total_height(layout: &AppLayout) -> u16 {
        layout.body.height
            + layout.todo.height
            + layout.input_sep.height
            + layout.input.height
            + layout.input_bottom_sep.height
            + layout.help.height
            + layout.footer.map_or(0, |f| f.height)
    }

    /// Collect all non-zero-height areas in top-to-bottom order.
    fn visible_areas(layout: &AppLayout) -> Vec<Rect> {
        let mut areas = vec![
            layout.body,
            layout.input_sep,
            layout.todo,
            layout.input,
            layout.input_bottom_sep,
            layout.help,
        ];
        if let Some(f) = layout.footer {
            areas.push(f);
        }
        areas.into_iter().filter(|r| r.height > 0).collect()
    }

    /// Assert no vertical overlap and areas are in ascending y-order.
    fn assert_no_overlap_and_ordered(layout: &AppLayout) {
        let areas = visible_areas(layout);
        for i in 1..areas.len() {
            let prev = areas[i - 1];
            let curr = areas[i];
            assert!(
                prev.y + prev.height <= curr.y,
                "Area {i}-1 ({prev:?}) overlaps or is not before area {i} ({curr:?})"
            );
        }
    }

    #[test]
    fn normal_layout_respects_requested_sections_and_footer_contract() {
        let layout = compute(area(80, 24), 5, 3, 2);
        let footer = layout.footer.expect("normal layout should include a footer");

        assert_eq!(layout.input_sep.height, 1);
        assert_eq!(layout.todo.height, 3);
        assert_eq!(layout.input.height, 5);
        assert_eq!(layout.input_bottom_sep.height, 1);
        assert_eq!(layout.help.height, 2);
        assert_eq!(footer.height, 2);
        assert!(layout.body.height >= 3);
        assert_eq!(total_height(&layout), 24);
        assert_eq!(footer.y + footer.height, 24);
    }

    #[test]
    fn compact_layout_omits_footer_and_todo_and_allocates_remaining_space_to_input_and_help() {
        let layout = compute(area(80, 6), 3, 4, 2);

        assert!(layout.footer.is_none());
        assert_eq!(layout.todo.height, 0);
        assert_eq!(layout.input_sep.height, 0);
        assert_eq!(layout.help.height, 2);
        assert!(layout.input.height >= 1);
        assert_eq!(total_height(&layout), 6);
    }

    #[test]
    fn layout_threshold_switches_at_height_eight() {
        let compact = compute(area(80, 7), 1, 0, 0);
        let normal = compute(area(80, 8), 1, 0, 1);

        assert!(compact.footer.is_none());
        assert!(normal.footer.is_some());
        assert_eq!(normal.help.height, 1);
    }

    #[test]
    fn layout_preserves_origin_and_width_in_both_modes() {
        let normal = compute(Rect::new(10, 5, 80, 24), 1, 0, 0);
        let compact = compute(Rect::new(5, 10, 60, 6), 1, 0, 0);

        for area in visible_areas(&normal) {
            assert_eq!(area.x, 10);
            assert_eq!(area.width, 80);
        }
        for area in visible_areas(&compact) {
            assert_eq!(area.x, 5);
            assert_eq!(area.width, 60);
        }
        assert_eq!(normal.body.y, 5);
        assert_eq!(compact.body.y, 10);
    }

    #[test]
    fn layout_clamps_input_and_preserves_total_height_for_degenerate_sizes() {
        let zero_height = compute(area(80, 0), 1, 0, 0);
        let height_one = compute(area(80, 1), 1, 0, 0);
        let width_one = compute(Rect::new(0, 0, 1, 24), 0, 0, 0);
        let width_zero = compute(area(0, 24), 1, 0, 0);

        assert!(zero_height.footer.is_none());
        assert_eq!(total_height(&zero_height), 0);
        assert_eq!(total_height(&height_one), 1);
        assert_eq!(width_one.input.height, 1);
        assert_eq!(width_one.body.width, 1);
        assert_eq!(width_zero.body.width, 0);
        assert_eq!(total_height(&width_zero), 24);
    }

    #[test]
    fn layout_squeezes_body_when_requested_sections_exceed_available_space() {
        let oversize_input = compute(area(80, 10), 50, 0, 0);
        let competing = compute(area(80, 12), 3, 4, 3);
        let large = compute(area(200, 100), 3, 5, 2);

        assert_eq!(total_height(&oversize_input), 10);
        assert_eq!(total_height(&competing), 12);
        assert_eq!(total_height(&large), 100);
        assert!(large.body.height >= 3);
    }

    #[test]
    fn layout_areas_remain_ordered_in_normal_and_compact_modes() {
        let normal = compute(area(80, 30), 2, 3, 1);
        let compact = compute(area(80, 6), 1, 0, 1);

        assert_no_overlap_and_ordered(&normal);
        assert_no_overlap_and_ordered(&compact);
        assert_eq!(normal.body.y, 0);
    }

    #[test]
    fn parametric_layout_invariants_hold_across_sizes_and_feature_combinations() {
        for h in [0, 1, 2, 3, 5, 7, 8, 10, 15, 24, 50, 100] {
            for w in [0, 1, 10, 80, 200] {
                let layout = compute(Rect::new(0, 0, w, h), 1, 0, 0);
                assert_eq!(total_height(&layout), h, "height mismatch for {w}x{h}");
                for area in visible_areas(&layout) {
                    assert_eq!(area.width, w, "width mismatch in area {area:?} for {w}x{h}");
                }
            }
        }

        for input in [0, 1, 3, 10] {
            for todo in [0, 2, 5] {
                for help in [0, 1, 3] {
                    let layout = compute(area(80, 30), input, todo, help);
                    assert_eq!(
                        total_height(&layout),
                        30,
                        "height mismatch for input={input} todo={todo} help={help}"
                    );
                    assert_no_overlap_and_ordered(&layout);
                }
            }
        }
    }
}
