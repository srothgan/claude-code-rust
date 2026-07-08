// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crossterm::event::KeyModifiers;
use std::error::Error;
use std::fmt;
use std::str::FromStr;

use super::spec::{KeyCodeSpec, KeySpec};
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseKeySpecError {
    Empty,
    MissingKey,
    DuplicateModifier(String),
    UnsupportedKey(String),
}

impl fmt::Display for ParseKeySpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("key spec is empty"),
            Self::MissingKey => formatter.write_str("key spec is missing a key"),
            Self::DuplicateModifier(modifier) => {
                write!(formatter, "key spec repeats modifier '{modifier}'")
            }
            Self::UnsupportedKey(key) => write!(formatter, "unsupported key '{key}'"),
        }
    }
}

impl Error for ParseKeySpecError {}

impl FromStr for KeySpec {
    type Err = ParseKeySpecError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let normalized = input.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err(ParseKeySpecError::Empty);
        }

        let mut modifiers = KeyModifiers::NONE;
        let tokens: Vec<&str> = normalized.split('-').filter(|token| !token.is_empty()).collect();
        if tokens.is_empty() {
            return Err(ParseKeySpecError::Empty);
        }

        let mut key_start = None;
        for (index, token) in tokens.iter().enumerate() {
            if let Some(modifier) = parse_modifier(token) {
                if modifiers.contains(modifier) {
                    return Err(ParseKeySpecError::DuplicateModifier((*token).to_owned()));
                }
                modifiers.insert(modifier);
            } else {
                key_start = Some(index);
                break;
            }
        }

        let Some(key_start) = key_start else {
            return Err(ParseKeySpecError::MissingKey);
        };
        let key_name = tokens[key_start..].join("-");
        let code = parse_key_code(&key_name)
            .ok_or_else(|| ParseKeySpecError::UnsupportedKey(key_name.clone()))?;
        Ok(Self::new(code, modifiers))
    }
}

fn parse_modifier(token: &str) -> Option<KeyModifiers> {
    match token {
        "ctrl" | "control" => Some(KeyModifiers::CONTROL),
        "alt" | "option" => Some(KeyModifiers::ALT),
        "shift" => Some(KeyModifiers::SHIFT),
        "cmd" | "command" | "super" => Some(KeyModifiers::SUPER),
        _ => None,
    }
}

fn parse_key_code(key_name: &str) -> Option<KeyCodeSpec> {
    match key_name {
        "enter" | "return" => Some(KeyCodeSpec::Enter),
        "esc" | "escape" => Some(KeyCodeSpec::Esc),
        "backspace" | "bs" => Some(KeyCodeSpec::Backspace),
        "delete" | "del" => Some(KeyCodeSpec::Delete),
        "insert" | "ins" => Some(KeyCodeSpec::Insert),
        "tab" => Some(KeyCodeSpec::Tab),
        "left" => Some(KeyCodeSpec::Left),
        "right" => Some(KeyCodeSpec::Right),
        "up" => Some(KeyCodeSpec::Up),
        "down" => Some(KeyCodeSpec::Down),
        "home" => Some(KeyCodeSpec::Home),
        "end" => Some(KeyCodeSpec::End),
        "pageup" | "page-up" => Some(KeyCodeSpec::PageUp),
        "pagedown" | "page-down" => Some(KeyCodeSpec::PageDown),
        "space" => Some(KeyCodeSpec::Char(' ')),
        _ => parse_function_key(key_name).or_else(|| parse_single_char_key(key_name)),
    }
}

fn parse_function_key(key_name: &str) -> Option<KeyCodeSpec> {
    let digits = key_name.strip_prefix('f')?;
    let index = digits.parse::<u8>().ok()?;
    (index > 0).then_some(KeyCodeSpec::F(index))
}

fn parse_single_char_key(key_name: &str) -> Option<KeyCodeSpec> {
    let mut chars = key_name.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(KeyCodeSpec::Char(ch))
}
