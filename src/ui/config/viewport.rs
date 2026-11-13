// SPDX-License-Identifier: Apache-2.0

use crate::app::config::ConfigState;
use ratatui::{Frame, buffer::Buffer, layout::Rect};

/// Clip a fully laid-out read-only pane, keeping its cards intact and the shell controls outside it.
pub(super) fn present(frame: &mut Frame, area: Rect, content: &Buffer, state: &mut ConfigState) {
    state.read_only.resize(area.height, content.area.height);
    for y in 0..area.height.min(content.area.height) {
        for x in 0..area.width {
            frame.buffer_mut()[(area.x + x, area.y + y)] =
                content[(x, state.read_only.offset() + y)].clone();
        }
    }
}
