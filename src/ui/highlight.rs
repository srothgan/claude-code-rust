// SPDX-License-Identifier: Apache-2.0

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::EmbeddedThemeName;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static THEME_SET: LazyLock<two_face::theme::EmbeddedLazyThemeSet> =
    LazyLock::new(two_face::theme::extra);

pub(crate) fn highlight_code(text: &str, language: Option<&str>) -> Vec<Line<'static>> {
    highlight_code_with_control(
        text,
        language,
        &super::layout_control::LayoutControl::uninterrupted(),
    )
}

pub(super) fn highlight_code_with_control(
    text: &str,
    language: Option<&str>,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    highlight_with_syntax(text, syntax_for_language(language), highlight_theme(), control)
}

/// The syntax state of one active source traversal; dropped when that traversal ends.
pub(crate) struct SourceHighlighter(HighlightLines<'static>);

impl SourceHighlighter {
    pub(crate) fn new(
        path: Option<&Path>,
        language_fallback: Option<&str>,
        theme_name: EmbeddedThemeName,
    ) -> Self {
        let syntax = path
            .and_then(find_syntax_for_path)
            .or_else(|| language_fallback.and_then(find_syntax))
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
        Self(HighlightLines::new(syntax, theme(theme_name)))
    }

    pub(crate) fn for_code(language: Option<&str>) -> Self {
        Self(HighlightLines::new(syntax_for_language(language), highlight_theme()))
    }

    pub(crate) fn line(&mut self, source: &str, visible: bool) -> Option<Line<'static>> {
        let started = std::time::Instant::now();
        let line = highlight_line(source, &mut self.0, visible);
        if started.elapsed() >= std::time::Duration::from_millis(50) {
            tracing::debug!(target: crate::logging::targets::APP_RENDER, event_name = "syntax_highlight_slow_line", bytes = source.len(), elapsed_us = started.elapsed().as_micros());
        }
        line
    }
}

fn highlight_with_syntax(
    text: &str,
    syntax: &SyntaxReference,
    theme: &'static Theme,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::default()];
    }

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for raw_line in LinesWithEndings::from(text) {
        if control.cancelled() {
            return Vec::new();
        }
        lines.extend(highlight_line(raw_line, &mut highlighter, true));
    }

    if text.ends_with('\n') {
        lines.push(Line::default());
    }

    lines
}

fn highlight_line(
    line: &str,
    highlighter: &mut HighlightLines<'_>,
    visible: bool,
) -> Option<Line<'static>> {
    match highlighter.highlight_line(line, &SYNTAX_SET) {
        Ok(_) if !visible => None,
        Ok(ranges) => {
            let spans = ranges
                .into_iter()
                .filter_map(|(style, segment)| {
                    let content = segment.strip_suffix('\n').unwrap_or(segment);
                    if content.is_empty() {
                        None
                    } else {
                        Some(Span::styled(
                            content.to_owned(),
                            ratatui_style(style.foreground, style.font_style),
                        ))
                    }
                })
                .collect::<Vec<_>>();
            Some(if spans.is_empty() { Line::default() } else { Line::from(spans) })
        }
        Err(err) => {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "syntax_highlight_failed",
                message = "syntax highlighting failed; falling back to plain text",
                outcome = "fallback",
                error_message = %err,
            );
            visible.then(|| Line::from(line.trim_end_matches('\n').to_owned()))
        }
    }
}

pub(crate) fn plain_text_lines(text: &str) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::default()];
    }
    let mut lines: Vec<Line<'static>> =
        text.split('\n').map(|line| Line::from(line.to_owned())).collect();
    if lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

/// Unknown languages render as plain text.
fn syntax_for_language(language: Option<&str>) -> &'static SyntaxReference {
    let Some(language) = language.filter(|language| !language.trim().is_empty()) else {
        return SYNTAX_SET.find_syntax_plain_text();
    };
    find_syntax(language).unwrap_or_else(|| {
        tracing::debug!(
            target: crate::logging::targets::APP_RENDER,
            event_name = "syntax_language_unknown",
            message = "code language is not recognized; rendering plain text",
            outcome = "fallback",
            language,
        );
        SYNTAX_SET.find_syntax_plain_text()
    })
}

fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    let token = language.trim();
    if token.is_empty() {
        return None;
    }
    SYNTAX_SET
        .find_syntax_by_token(token)
        .or_else(|| SYNTAX_SET.find_syntax_by_extension(token))
        .or_else(|| SYNTAX_SET.find_syntax_by_name(token))
}

fn find_syntax_for_path(path: &Path) -> Option<&'static SyntaxReference> {
    match SYNTAX_SET.find_syntax_for_file(path) {
        Ok(syntax) => syntax,
        Err(err) => {
            tracing::warn!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "syntax_detection_failed",
                message = "syntax detection from file path failed; falling back",
                outcome = "fallback",
                path = %path.display(),
                error_message = %err,
            );
            None
        }
    }
}

fn highlight_theme() -> &'static Theme {
    theme(EmbeddedThemeName::Base16OceanDark)
}

fn theme(theme_name: EmbeddedThemeName) -> &'static Theme {
    THEME_SET.get(theme_name)
}

fn ratatui_style(color: SyntectColor, font_style: FontStyle) -> Style {
    let mut style = Style::default().fg(Color::Rgb(color.r, color.g, color.b));

    if font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if font_style.contains(FontStyle::UNDERLINE) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }

    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_code_preserves_text() {
        let rendered = highlight_code("fn main() {}\n", Some("rs"));
        let text: String = rendered[0].spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(text.contains("fn"));
        assert!(text.contains("main"));
    }

    #[test]
    fn highlight_code_accepts_explicit_theme() {
        let rendered =
            SourceHighlighter::new(None, Some("rs"), EmbeddedThemeName::MonokaiExtendedBright)
                .line("fn main() {}\n", true)
                .unwrap();
        let text: String = rendered.spans.iter().map(|span| span.content.as_ref()).collect();

        assert!(text.contains("fn"));
        assert!(text.contains("main"));
        assert!(rendered.spans.iter().any(|span| span.style.fg.is_some()));
    }
}
