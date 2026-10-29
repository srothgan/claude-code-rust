// SPDX-License-Identifier: Apache-2.0

use ratatui::{buffer::Buffer, style::Modifier};
use std::path::Path;
use unicode_width::UnicodeWidthStr;

pub(crate) fn physical_key(path: &Path) -> String {
    let physical = path.canonicalize().unwrap().to_string_lossy().into_owned();
    if cfg!(windows) {
        physical.strip_prefix(r"\\?\").unwrap_or(&physical).replace('\\', "/")
    } else {
        physical
    }
}

pub(crate) fn row_text(buffer: &Buffer, y: u16, left: u16, right: u16) -> String {
    let mut text = String::new();
    let mut x = left;
    while x < right {
        let symbol = buffer[(x, y)].symbol();
        text.push_str(symbol);
        x += u16::try_from(symbol.width().max(1)).unwrap();
    }
    text
}

pub(crate) fn choices(buffer: &Buffer) -> (u16, u16) {
    let find = |label: &str| {
        (1..buffer.area.height - 1)
            .find(|&y| {
                let text = row_text(buffer, y, 2, buffer.area.width - 2);
                text.trim() == label || text.trim() == format!("> {label}")
            })
            .unwrap()
    };
    let (yes, no) = (find("Yes"), find("No"));
    assert!(yes < no);
    (yes, no)
}

/// Read occupied path cells, including styled spaces at row boundaries.
pub(crate) fn path_region(buffer: &Buffer) -> (Vec<u16>, String) {
    let (yes, _) = choices(buffer);
    let labelled =
        (1..yes).find(|&y| row_text(buffer, y, 2, buffer.area.width - 2).starts_with("Path: "));
    let start = labelled.unwrap_or(1);
    let left = if labelled.is_some() { 8 } else { 2 };
    let mut rows = Vec::new();
    let mut text = String::new();
    for y in start..yes {
        if !buffer[(left, y)].modifier.contains(Modifier::BOLD) {
            break;
        }
        let end = (left..buffer.area.width - 2)
            .rev()
            .find(|&x| buffer[(x, y)].modifier.contains(Modifier::BOLD))
            .unwrap()
            + 1;
        rows.push(y);
        text.push_str(&row_text(buffer, y, left, end));
    }
    (rows, text)
}
