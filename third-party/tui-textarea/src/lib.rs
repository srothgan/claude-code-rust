#![forbid(unsafe_code)]
#![allow(clippy::needless_range_loop)]
#![warn(clippy::dbg_macro, clippy::print_stdout)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = "Vendored tui-textarea editor widget. See the package repository for upstream documentation."]

mod cursor;
mod highlight;
mod history;
mod input;
mod layout;
mod screen_map;
mod scroll;
#[cfg(feature = "search")]
mod search;
mod textarea;
mod util;
mod widget;
mod word;
mod wrap;

#[cfg(feature = "ratatui")]
#[allow(clippy::single_component_path_imports)]
mod ratatui {
    // Best effort to reproduce ratatui 0.29 module layout to keep compatibility with tui module layout
    pub use ratatui_core::{buffer, layout, style, text};
    pub mod widgets {
        pub use ratatui_core::widgets::*;
        pub use ratatui_widgets::{block::Block, paragraph::Paragraph};
    }
}
#[cfg(all(feature = "crossterm", not(feature = "crossterm_0_28")))]
#[allow(clippy::single_component_path_imports)]
use crossterm;
#[cfg(feature = "crossterm_0_28")]
#[allow(clippy::single_component_path_imports)]
use crossterm_028 as crossterm;

#[cfg(feature = "termion")]
#[allow(clippy::single_component_path_imports)]
use termion;

pub use cursor::CursorMove;
pub use input::{Input, Key};
pub use scroll::Scrolling;
pub use textarea::{
    AtomicCursorBias, AtomicDeleteDirection, AtomicRange, AtomicRangeError,
    AtomicRangeRejectReason, CursorRenderMode, RejectedAtomicRange, TextArea, TextAreaMeasure,
};
pub use wrap::WrapMode;
