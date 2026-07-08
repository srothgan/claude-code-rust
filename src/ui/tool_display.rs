// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Presentation-only formatting for tool names and titles.

use std::borrow::Cow;

use crate::ui::theme;

const MCP_PREFIX: &str = "mcp__";
const MCP_ICON: &str = "\u{232c}";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolLabel<'a> {
    pub(crate) icon: &'static str,
    pub(crate) label: Cow<'a, str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpToolDisplay {
    pub(crate) server: String,
    pub(crate) tool: String,
}

impl McpToolDisplay {
    #[must_use]
    pub(crate) fn title(&self) -> String {
        format!("{}: {}", self.server, self.tool)
    }
}

#[must_use]
pub(crate) fn parse_mcp_tool_name(raw: &str) -> Option<McpToolDisplay> {
    let rest = raw.strip_prefix(MCP_PREFIX)?;
    let (server_raw, tool_raw) = rest.split_once("__")?;
    if server_raw.is_empty() || tool_raw.is_empty() {
        return None;
    }

    Some(McpToolDisplay {
        server: humanize_server_name(server_raw).unwrap_or_else(|| server_raw.to_owned()),
        tool: humanize_tool_name(tool_raw).unwrap_or_else(|| tool_raw.to_owned()),
    })
}

#[must_use]
pub(crate) fn tool_name_label(sdk_tool_name: &str) -> ToolLabel<'_> {
    if let Some(display) = parse_mcp_tool_name(sdk_tool_name) {
        return ToolLabel { icon: MCP_ICON, label: Cow::Owned(display.server) };
    }

    let (icon, label) = theme::tool_name_label(sdk_tool_name);
    ToolLabel { icon, label: Cow::Borrowed(label) }
}

#[must_use]
pub(crate) fn tool_title<'a>(raw_tool_name: &'a str, raw_title: &'a str) -> Cow<'a, str> {
    let title = if raw_title.is_empty() { raw_tool_name } else { raw_title };
    if title == raw_tool_name
        && let Some(display) = parse_mcp_tool_name(raw_tool_name)
    {
        return Cow::Owned(display.title());
    }

    Cow::Borrowed(title)
}

fn humanize_server_name(raw: &str) -> Option<String> {
    let words = clean_server_words(split_identifier_words(raw));
    join_non_empty(words.into_iter().map(|word| format_server_word(&word)))
}

fn clean_server_words(mut words: Vec<String>) -> Vec<String> {
    if words.len() > 2
        && words[0].eq_ignore_ascii_case("claude")
        && words[1].eq_ignore_ascii_case("ai")
    {
        words.drain(0..2);
    }
    if words.len() > 1 && words[0].eq_ignore_ascii_case("plugin") {
        words.remove(0);
    }

    let mut seen = Vec::new();
    let mut out = Vec::new();
    for word in words {
        let key = word.to_ascii_lowercase();
        if seen.iter().any(|existing| existing == &key) {
            continue;
        }
        seen.push(key);
        out.push(word);
    }
    out
}

fn humanize_tool_name(raw: &str) -> Option<String> {
    let words = split_identifier_words(raw);
    join_non_empty(
        words.into_iter().enumerate().map(|(index, word)| format_tool_word(&word, index == 0)),
    )
}

fn split_identifier_words(raw: &str) -> Vec<String> {
    let chars = raw.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();

    for (index, ch) in chars.iter().copied().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            push_identifier_word(&mut words, &mut current);
            continue;
        }

        if starts_identifier_word(&current, ch, chars.get(index + 1).copied()) {
            push_identifier_word(&mut words, &mut current);
        }
        current.push(ch);
    }
    push_identifier_word(&mut words, &mut current);

    words
}

fn starts_identifier_word(current: &str, ch: char, next: Option<char>) -> bool {
    let Some(previous) = current.chars().last() else {
        return false;
    };

    (previous.is_ascii_lowercase() && ch.is_ascii_uppercase())
        || (previous.is_ascii_uppercase()
            && ch.is_ascii_uppercase()
            && current.len() > 1
            && next.is_some_and(|next| next.is_ascii_lowercase()))
}

fn push_identifier_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}

fn format_server_word(word: &str) -> String {
    if word.chars().any(|ch| ch.is_ascii_uppercase()) || word.len() <= 4 {
        return word.to_owned();
    }
    capitalize_ascii(&word.to_ascii_lowercase())
}

fn format_tool_word(word: &str, first: bool) -> String {
    if has_intentional_case(word) {
        return word.to_owned();
    }

    let normalized = word.to_ascii_lowercase();
    if first { capitalize_ascii(&normalized) } else { normalized }
}

fn has_intentional_case(word: &str) -> bool {
    word.chars().filter(char::is_ascii_uppercase).count() > 1
}

fn capitalize_ascii(value: &str) -> String {
    let Some(first) = value.chars().next() else {
        return String::new();
    };
    let mut out = String::with_capacity(value.len());
    out.push(first.to_ascii_uppercase());
    out.push_str(&value[first.len_utf8()..]);
    out
}

fn join_non_empty(words: impl Iterator<Item = String>) -> Option<String> {
    let mut out = String::new();
    for word in words {
        if word.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word);
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::{MCP_ICON, parse_mcp_tool_name, tool_name_label, tool_title};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn parse_mcp_tool_name_formats_known_examples() {
        let cases = [
            (
                "mcp__claude_ai_Strava__list_activities",
                "Strava",
                "List activities",
                "Strava: List activities",
            ),
            (
                "mcp__claude_ai_Strava__get_activity_streams",
                "Strava",
                "Get activity streams",
                "Strava: Get activity streams",
            ),
            ("mcp__fff__find_files", "fff", "Find files", "fff: Find files"),
            (
                "mcp__plugin_Notion_notion__notion-search",
                "Notion",
                "Notion search",
                "Notion: Notion search",
            ),
        ];

        for (raw, server, tool, title) in cases {
            let display = parse_mcp_tool_name(raw).expect("valid MCP tool name");
            assert_eq!(display.server, server);
            assert_eq!(display.tool, tool);
            assert_eq!(display.title(), title);
        }
    }

    #[test]
    fn parse_mcp_tool_name_requires_non_empty_server_and_tool_segments() {
        for raw in ["mcp__missing_separator", "mcp____find_files", "mcp__fff__", "not_mcp__x__y"] {
            assert_eq!(parse_mcp_tool_name(raw), None);
        }
    }

    #[test]
    fn parse_mcp_tool_name_preserves_raw_segments_when_humanizing_has_no_words() {
        let display = parse_mcp_tool_name("mcp__---__...").expect("valid MCP wrapper");
        assert_eq!(display.server, "---");
        assert_eq!(display.tool, "...");
        assert_eq!(display.title(), "---: ...");
    }

    #[test]
    fn tool_name_label_uses_mcp_icon_and_server_label() {
        let label = tool_name_label("mcp__claude_ai_Strava__get_gear");
        assert_eq!(label.icon, MCP_ICON);
        assert_eq!(label.label.as_ref(), "Strava");
    }

    #[test]
    fn mcp_icon_is_single_column() {
        assert_eq!(UnicodeWidthStr::width(MCP_ICON), 1);
    }

    #[test]
    fn tool_name_label_keeps_generic_fallback_for_unknown_non_mcp_tools() {
        let label = tool_name_label("UnknownFutureTool");
        assert_eq!(label.icon, "\u{25cb}");
        assert_eq!(label.label.as_ref(), "Tool");
    }

    #[test]
    fn tool_title_formats_raw_mcp_titles_only() {
        assert_eq!(
            tool_title(
                "mcp__claude_ai_Strava__get_athlete_profile",
                "mcp__claude_ai_Strava__get_athlete_profile",
            )
            .as_ref(),
            "Strava: Get athlete profile",
        );
        assert_eq!(
            tool_title("mcp__claude_ai_Strava__get_athlete_zones", "Athlete zones").as_ref(),
            "Athlete zones",
        );
    }

    #[test]
    fn tool_title_falls_back_to_raw_name_for_unknown_mcp_shapes() {
        assert_eq!(
            tool_title("mcp__fff_find_files", "mcp__fff_find_files").as_ref(),
            "mcp__fff_find_files",
        );
    }

    #[test]
    fn tool_title_uses_tool_name_when_title_is_empty() {
        assert_eq!(tool_title("mcp__fff__find_files", "").as_ref(), "fff: Find files");
        assert_eq!(tool_title("UnknownFutureTool", "").as_ref(), "UnknownFutureTool");
    }
}
