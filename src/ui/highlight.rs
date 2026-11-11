// SPDX-License-Identifier: Apache-2.0

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, HighlightIterator, HighlightState, Highlighter, Theme,
};
use syntect::parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};
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
pub(crate) struct SourceHighlighter {
    parser: ParseState,
    highlighter: Highlighter<'static>,
    state: SourceHighlightState,
    syntax: &'static str,
    timings: HighlightTimings,
}

enum SourceHighlightState {
    Styled(HighlightState),
    Scopes(ScopeStack),
}

#[derive(Default)]
pub(crate) struct HighlightTimings {
    pub lines: usize,
    pub visible_lines: usize,
    pub parse: Duration,
    pub hidden_scope: Duration,
    pub visible_style: Duration,
    pub max_line: Duration,
}

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
        Self::with_syntax(syntax, theme(theme_name))
    }

    pub(crate) fn for_code(language: Option<&str>) -> Self {
        Self::with_syntax(syntax_for_language(language), highlight_theme())
    }

    fn with_syntax(syntax: &'static SyntaxReference, theme: &'static Theme) -> Self {
        let highlighter = Highlighter::new(theme);
        let state =
            SourceHighlightState::Styled(HighlightState::new(&highlighter, ScopeStack::new()));
        Self {
            parser: ParseState::new(syntax),
            highlighter,
            state,
            syntax: &syntax.name,
            timings: HighlightTimings::default(),
        }
    }

    pub(crate) fn timings(&self) -> &HighlightTimings {
        &self.timings
    }

    pub(crate) fn line(&mut self, source: &str, visible: bool) -> Option<Line<'static>> {
        let started =
            tracing::enabled!(target: crate::logging::targets::APP_RENDER, tracing::Level::DEBUG)
                .then(Instant::now);
        let operations = self.parser.parse_line(source, &SYNTAX_SET);
        let parse = started.map_or(Duration::ZERO, |start| start.elapsed());
        let styling_started = started.map(|_| Instant::now());
        let result = operations
            .map_err(syntect::Error::from)
            .and_then(|operations| self.render_parsed_line(source, visible, &operations));
        let styling = styling_started.map_or(Duration::ZERO, |start| start.elapsed());
        let elapsed = started.map_or(Duration::ZERO, |start| start.elapsed());
        self.timings.lines += 1;
        self.timings.visible_lines += usize::from(visible);
        self.timings.parse += parse;
        if visible {
            self.timings.visible_style += styling;
        } else {
            self.timings.hidden_scope += styling;
        }
        self.timings.max_line = self.timings.max_line.max(elapsed);
        if elapsed >= Duration::from_millis(50) {
            tracing::debug!(
                target: crate::logging::targets::APP_RENDER,
                event_name = "syntax_highlight_slow_line",
                syntax = self.syntax,
                source_line = self.timings.lines,
                visible,
                bytes = source.len(),
                parse_us = parse.as_micros(),
                hidden_scope_us = if visible { 0 } else { styling.as_micros() },
                visible_style_us = if visible { styling.as_micros() } else { 0 },
                elapsed_us = elapsed.as_micros(),
            );
        }
        match result {
            Ok(line) => line,
            Err(err) => {
                tracing::warn!(
                    target: crate::logging::targets::APP_RENDER,
                    event_name = "syntax_highlight_failed",
                    message = "syntax highlighting failed; falling back to plain text",
                    outcome = "fallback",
                    error_message = %err,
                );
                visible.then(|| Line::from(source.trim_end_matches('\n').to_owned()))
            }
        }
    }

    fn render_parsed_line(
        &mut self,
        source: &str,
        visible: bool,
        operations: &[(usize, ScopeStackOp)],
    ) -> Result<Option<Line<'static>>, syntect::Error> {
        match (&mut self.state, visible) {
            (SourceHighlightState::Styled(state), false) => {
                self.state = SourceHighlightState::Scopes(std::mem::take(&mut state.path));
            }
            (SourceHighlightState::Scopes(scopes), true) => {
                // Rebuild styles once when leaving a hidden run. Keep the complete stack,
                // including saved scopes used by embedded syntaxes' Clear/Restore.
                self.state = SourceHighlightState::Styled(HighlightState::new(
                    &self.highlighter,
                    std::mem::take(scopes),
                ));
            }
            _ => {}
        }
        match &mut self.state {
            SourceHighlightState::Scopes(scopes) => {
                for (_, operation) in operations {
                    scopes.apply(operation)?;
                }
                Ok(None)
            }
            SourceHighlightState::Styled(state) => {
                let spans = HighlightIterator::new(state, operations, source, &self.highlighter)
                    .filter_map(|(style, segment)| {
                        let content = segment.strip_suffix('\n').unwrap_or(segment);
                        (!content.is_empty()).then(|| {
                            Span::styled(
                                content.to_owned(),
                                ratatui_style(style.foreground, style.font_style),
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(Some(if spans.is_empty() { Line::default() } else { Line::from(spans) }))
            }
        }
    }
}

fn highlight_with_syntax(
    text: &str,
    syntax: &'static SyntaxReference,
    theme: &'static Theme,
    control: &super::layout_control::LayoutControl<'_>,
) -> Vec<Line<'static>> {
    if text.is_empty() {
        return vec![Line::default()];
    }

    let mut highlighter = SourceHighlighter::with_syntax(syntax, theme);
    let mut lines = Vec::new();

    for raw_line in LinesWithEndings::from(text) {
        if control.cancelled() {
            return Vec::new();
        }
        lines.extend(highlighter.line(raw_line, true));
    }

    if text.ends_with('\n') {
        lines.push(Line::default());
    }

    lines
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
    use syntect::easy::HighlightLines;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn reference_style(style: syntect::highlighting::Style) -> Style {
        let color = style.foreground;
        let mut result = Style::default().fg(Color::Rgb(color.r, color.g, color.b));
        if style.font_style.contains(FontStyle::BOLD) {
            result = result.add_modifier(Modifier::BOLD);
        }
        if style.font_style.contains(FontStyle::ITALIC) {
            result = result.add_modifier(Modifier::ITALIC);
        }
        if style.font_style.contains(FontStyle::UNDERLINE) {
            result = result.add_modifier(Modifier::UNDERLINED);
        }
        result
    }

    #[test]
    fn hidden_lines_preserve_upstream_styles_across_syntax_boundaries() {
        let fixtures = [
            (
                "rs",
                "fn head() {}\n/* outer\n/* nested */\nstill comment\n*/\nlet s = r###\"raw\nstring\"###;\nfn tail() {}\n",
            ),
            ("py", "x = 1\ns = \"\"\"start\ninside\nmore\nend\"\"\"\nprint(s)\n"),
            (
                "ts",
                "const message = `start\n${(() => {\n/* nested expression\ninside comment\n*/\nreturn 'value';\n})()}\nend`;\nconsole.log(message);\n",
            ),
            (
                "tsx",
                "const view = (\n<section>\n{/* comment\ninside\n*/}\n<span>{`hello\nworld`}</span>\n</section>\n);\n",
            ),
            (
                "html",
                "<html>\n<script>\n/* comment\nstill comment\n*/\nconst s = `text`;\n</script>\n<p>tail</p>\n</html>\n",
            ),
            ("md", "# Heading\n\n```rust\n/* comment\ninside\n*/\nfn main() {}\n```\n**tail**\n"),
        ];
        for theme_name in
            [EmbeddedThemeName::MonokaiExtendedBright, EmbeddedThemeName::Base16OceanDark]
        {
            for (language, source) in fixtures {
                let syntax = SYNTAX_SET.find_syntax_by_token(language).expect("fixture syntax");
                let lines: Vec<_> = LinesWithEndings::from(source).collect();
                let visibility =
                    (0..lines.len()).map(|i| i < 2 || i >= lines.len() - 3).collect::<Vec<_>>();
                let mut reference = HighlightLines::new(syntax, THEME_SET.get(theme_name));
                let mut actual = SourceHighlighter::new(None, Some(language), theme_name);
                for (index, (line, visible)) in lines.iter().zip(visibility).enumerate() {
                    let expected =
                        reference.highlight_line(line, &SYNTAX_SET).expect("reference highlight");
                    let rendered = actual.line(line, visible);
                    if !visible {
                        assert!(
                            rendered.is_none(),
                            "hidden line emitted spans: {language} {index}"
                        );
                        continue;
                    }
                    let expected = expected
                        .into_iter()
                        .filter_map(|(style, text)| {
                            let text = text.strip_suffix('\n').unwrap_or(text);
                            (!text.is_empty()).then(|| (text.to_owned(), reference_style(style)))
                        })
                        .collect::<Vec<_>>();
                    let rendered = rendered.expect("visible line");
                    let actual = rendered
                        .spans
                        .iter()
                        .map(|span| (span.content.to_string(), span.style))
                        .collect::<Vec<_>>();
                    assert_eq!(actual, expected, "{language} line {index}, theme {theme_name:?}");
                }
            }
        }
    }

    #[test]
    fn highlight_code_preserves_complete_source_for_known_and_fallback_languages() {
        for (source, language, expected) in [
            (
                "  fn main() {\n    let café = \"x y\";\n}\n",
                Some("rs"),
                vec!["  fn main() {", "    let café = \"x y\";", "}", ""],
            ),
            ("first  row\n\n雪; !?\n", None, vec!["first  row", "", "雪; !?", ""]),
            ("one\ntwo", Some("no-such-language"), vec!["one", "two"]),
            ("", Some("rs"), vec![""]),
        ] {
            let rendered = highlight_code(source, language);
            let actual = rendered.iter().map(line_text).collect::<Vec<_>>();
            assert_eq!(actual, expected, "language={language:?}, source={source:?}");
        }
    }
}
