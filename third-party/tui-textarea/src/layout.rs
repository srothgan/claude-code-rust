use crate::util::num_digits;
use crate::wrap::WrapMode;

const CARET_WIDTH: usize = 1;

/// Geometry shared by wrapping, measurement, rendering, and hit testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayoutMetrics {
    line_number_width: usize,
    wrap_width: Option<usize>,
}

impl LayoutMetrics {
    pub(crate) fn new(
        inner_width: u16,
        line_count: usize,
        line_numbers: bool,
        wrap_mode: WrapMode,
    ) -> Self {
        let line_number_width = if line_numbers {
            num_digits(line_count) as usize + 2
        } else {
            0
        };
        let editable_width = usize::from(inner_width).saturating_sub(line_number_width);
        // Widths smaller than the caret plus one terminal cell cannot satisfy both requirements. Keeping a one-cell wrap width still guarantees forward progress for these physically constrained layouts.
        let wrap_width = (wrap_mode != WrapMode::None)
            .then(|| editable_width.saturating_sub(CARET_WIDTH).max(1));

        Self {
            line_number_width,
            wrap_width,
        }
    }

    pub(crate) fn line_number_width(self) -> usize {
        self.line_number_width
    }

    pub(crate) fn wrap_width(self) -> Option<usize> {
        self.wrap_width
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_layout_reserves_a_caret_cell() {
        let metrics = LayoutMetrics::new(8, 1, false, WrapMode::Glyph);
        assert_eq!(metrics.wrap_width(), Some(7));
    }

    #[test]
    fn line_numbers_are_reserved_before_the_caret() {
        let metrics = LayoutMetrics::new(8, 1, true, WrapMode::Word);
        assert_eq!(metrics.line_number_width(), 3);
        assert_eq!(metrics.wrap_width(), Some(4));
    }

    #[test]
    fn unwrapped_layout_keeps_the_full_width_for_scrolling() {
        let metrics = LayoutMetrics::new(8, 1, false, WrapMode::None);
        assert_eq!(metrics.wrap_width(), None);
    }

    #[test]
    fn wrapped_layout_retains_a_minimum_progress_width() {
        let metrics = LayoutMetrics::new(0, 1, false, WrapMode::WordOrGlyph);
        assert_eq!(metrics.wrap_width(), Some(1));
    }
}
