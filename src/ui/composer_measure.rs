// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::ui::footer_rows::FOOTER_ROW_COUNT;
use crate::ui::input_rows::InputRowsMeasurement;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ComposerMeasurement {
    pub width: u16,
    pub hint_rows: u16,
    pub editor_rows: u16,
    pub footer_rows: u16,
    pub total_rows: u16,
    pub caret_row: u16,
    pub caret_col: u16,
}

pub(crate) fn measure_composer(
    width: u16,
    input: InputRowsMeasurement,
    footer_present: bool,
) -> ComposerMeasurement {
    let footer_rows = if footer_present { FOOTER_ROW_COUNT } else { 0 };
    let total_rows = input.hint_rows.saturating_add(input.editor_rows).saturating_add(footer_rows);
    let caret_row = input.hint_rows.saturating_add(input.caret_row);

    ComposerMeasurement {
        width,
        hint_rows: input.hint_rows,
        editor_rows: input.editor_rows,
        footer_rows,
        total_rows,
        caret_row,
        caret_col: input.caret_col,
    }
}

#[cfg(test)]
mod tests {
    use super::{ComposerMeasurement, measure_composer};
    use crate::ui::input_rows::InputRowsMeasurement;

    #[test]
    fn measure_composer_includes_footer_rows_when_present() {
        let measurement = measure_composer(
            80,
            InputRowsMeasurement { hint_rows: 1, editor_rows: 3, caret_row: 2, caret_col: 4 },
            true,
        );

        assert_eq!(
            measurement,
            ComposerMeasurement {
                width: 80,
                hint_rows: 1,
                editor_rows: 3,
                footer_rows: 2,
                total_rows: 6,
                caret_row: 3,
                caret_col: 4,
            }
        );
    }

    #[test]
    fn measure_composer_omits_footer_and_other_non_composer_regions() {
        let measurement = measure_composer(
            72,
            InputRowsMeasurement { hint_rows: 0, editor_rows: 2, caret_row: 1, caret_col: 1 },
            false,
        );

        assert_eq!(
            measurement,
            ComposerMeasurement {
                width: 72,
                hint_rows: 0,
                editor_rows: 2,
                footer_rows: 0,
                total_rows: 2,
                caret_row: 1,
                caret_col: 1,
            }
        );
    }
}
