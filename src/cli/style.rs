// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use std::io::IsTerminal as _;

#[derive(Clone, Copy, Debug)]
pub(super) struct HumanStyle {
    color: bool,
}

impl HumanStyle {
    pub(super) fn detect() -> Self {
        Self { color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
    }

    pub(super) fn title(self, text: &str) -> String {
        self.accent(text)
    }

    pub(super) fn heading(self, text: &str) -> String {
        self.accent(text)
    }

    pub(super) fn table_header(self, text: &str) -> String {
        self.muted(text)
    }

    pub(super) fn detail_label(self, text: &str) -> String {
        self.muted(text)
    }

    pub(super) fn command_block(self, mode: &str, command: &str, purpose: &str) -> String {
        format!("  {} {}\n      {}", self.mode(mode), command, purpose)
    }

    pub(super) fn mode(self, mode: &str) -> String {
        self.muted(mode)
    }

    pub(super) fn green(self, text: impl AsRef<str>) -> String {
        self.colorize(text.as_ref(), "32")
    }

    pub(super) fn yellow(self, text: impl AsRef<str>) -> String {
        self.colorize(text.as_ref(), "33")
    }

    pub(super) fn red(self, text: impl AsRef<str>) -> String {
        self.colorize(text.as_ref(), "31")
    }

    pub(super) fn muted(self, text: impl AsRef<str>) -> String {
        self.colorize(text.as_ref(), "2")
    }

    fn accent(self, text: &str) -> String {
        self.colorize(text, "1;36")
    }

    fn colorize(self, text: &str, code: &str) -> String {
        if self.color { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_owned() }
    }
}
