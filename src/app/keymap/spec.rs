// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt;
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct KeySpec {
    code: KeyCodeSpec,
    modifiers: KeyModifiers,
}

impl KeySpec {
    pub fn new(code: KeyCodeSpec, modifiers: KeyModifiers) -> Self {
        Self { code: code.normalized_for_modifiers(modifiers), modifiers }
    }

    pub fn char(ch: char, modifiers: KeyModifiers) -> Self {
        Self::new(KeyCodeSpec::Char(ch), modifiers)
    }

    pub fn from_event(key: KeyEvent) -> Option<Self> {
        let mut modifiers = key.modifiers;
        let code = match key.code {
            KeyCode::Backspace => KeyCodeSpec::Backspace,
            KeyCode::Enter => KeyCodeSpec::Enter,
            KeyCode::Left => KeyCodeSpec::Left,
            KeyCode::Right => KeyCodeSpec::Right,
            KeyCode::Up => KeyCodeSpec::Up,
            KeyCode::Down => KeyCodeSpec::Down,
            KeyCode::Home => KeyCodeSpec::Home,
            KeyCode::End => KeyCodeSpec::End,
            KeyCode::PageUp => KeyCodeSpec::PageUp,
            KeyCode::PageDown => KeyCodeSpec::PageDown,
            KeyCode::Tab => KeyCodeSpec::Tab,
            KeyCode::BackTab => {
                modifiers.insert(KeyModifiers::SHIFT);
                KeyCodeSpec::Tab
            }
            KeyCode::Delete => KeyCodeSpec::Delete,
            KeyCode::Insert => KeyCodeSpec::Insert,
            KeyCode::F(index) => KeyCodeSpec::F(index),
            KeyCode::Char(ch) => normalized_char_code(ch, &mut modifiers),
            KeyCode::Esc => KeyCodeSpec::Esc,
            KeyCode::Null
            | KeyCode::CapsLock
            | KeyCode::ScrollLock
            | KeyCode::NumLock
            | KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::Menu
            | KeyCode::KeypadBegin
            | KeyCode::Media(_)
            | KeyCode::Modifier(_) => return None,
        };
        Some(Self::new(code, modifiers))
    }

    pub fn code(&self) -> KeyCodeSpec {
        self.code
    }

    pub fn modifiers(&self) -> KeyModifiers {
        self.modifiers
    }

    pub fn matches_event(&self, key: KeyEvent) -> bool {
        Self::from_event(key).is_some_and(|candidate| candidate == *self)
    }
}

impl fmt::Display for KeySpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("ctrl".to_owned());
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            parts.push("alt".to_owned());
        }
        if self.modifiers.contains(KeyModifiers::SUPER) {
            parts.push("cmd".to_owned());
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("shift".to_owned());
        }
        parts.push(self.code.to_string());
        formatter.write_str(&parts.join("-"))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum KeyCodeSpec {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Insert,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

impl KeyCodeSpec {
    fn normalized_for_modifiers(self, modifiers: KeyModifiers) -> Self {
        match self {
            Self::Char(ch) if should_canonicalize_char(ch, modifiers) => {
                Self::Char(ch.to_ascii_lowercase())
            }
            _ => self,
        }
    }
}

impl fmt::Display for KeyCodeSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(' ') => formatter.write_str("space"),
            Self::Char(ch) if ch.is_ascii_control() => {
                write!(formatter, "u+{:04x}", u32::from(*ch))
            }
            Self::Char(ch) => write!(formatter, "{ch}"),
            Self::Enter => formatter.write_str("enter"),
            Self::Esc => formatter.write_str("esc"),
            Self::Backspace => formatter.write_str("backspace"),
            Self::Delete => formatter.write_str("delete"),
            Self::Insert => formatter.write_str("insert"),
            Self::Tab => formatter.write_str("tab"),
            Self::Left => formatter.write_str("left"),
            Self::Right => formatter.write_str("right"),
            Self::Up => formatter.write_str("up"),
            Self::Down => formatter.write_str("down"),
            Self::Home => formatter.write_str("home"),
            Self::End => formatter.write_str("end"),
            Self::PageUp => formatter.write_str("page-up"),
            Self::PageDown => formatter.write_str("page-down"),
            Self::F(index) => write!(formatter, "f{index}"),
        }
    }
}

fn normalized_char_code(ch: char, modifiers: &mut KeyModifiers) -> KeyCodeSpec {
    if let Some(alpha) = control_char_to_alpha(ch)
        && !modifiers.contains(KeyModifiers::ALT)
    {
        modifiers.insert(KeyModifiers::CONTROL);
        return KeyCodeSpec::Char(alpha);
    }
    KeyCodeSpec::Char(ch)
}

fn control_char_to_alpha(ch: char) -> Option<char> {
    let value = u32::from(ch);
    if (1..=26).contains(&value) { char::from_u32(value + u32::from(b'a') - 1) } else { None }
}

fn should_canonicalize_char(ch: char, modifiers: KeyModifiers) -> bool {
    ch.is_ascii_alphabetic()
        && modifiers.intersects(
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::SUPER,
        )
}
