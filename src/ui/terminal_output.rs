// SPDX-License-Identifier: Apache-2.0

//! Terminal output interpretation. SGR styling is kept relative to the program's
//! default style; every other escape sequence and control character is dropped.

use super::diff;
use super::layout_control::LayoutControl;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::iter::Peekable;
use std::str::CharIndices;

enum Event<'a> {
    Char(char),
    Newline,
    CarriageReturn,
    /// Parameters of a Select Graphic Rendition sequence (`ESC [ params m`).
    Sgr(&'a str),
    Control(char),
}

struct Scanner<'a> {
    text: &'a str,
    chars: Peekable<CharIndices<'a>>,
    control: &'a LayoutControl<'a>,
}

impl<'a> Scanner<'a> {
    fn scan(text: &'a str, control: &'a LayoutControl<'a>, mut emit: impl FnMut(Event<'a>)) {
        let mut scanner = Self { text, chars: text.char_indices().peekable(), control };
        while let Some((_, ch)) = scanner.chars.next() {
            if control.cancelled() {
                break;
            }
            match ch {
                '\u{1b}' => scanner.escape(&mut emit),
                '\n' => emit(Event::Newline),
                '\r' => emit(Event::CarriageReturn),
                '\t' => emit(Event::Char(ch)),
                _ if ch.is_control() => emit(Event::Control(ch)),
                _ => emit(Event::Char(ch)),
            }
        }
    }

    fn escape(&mut self, emit: &mut impl FnMut(Event<'a>)) {
        let Some(&(_, next)) = self.chars.peek() else {
            return;
        };
        match next {
            '[' => {
                self.chars.next();
                self.csi(emit);
            }
            ']' | 'P' | 'X' | '^' | '_' => {
                self.chars.next();
                self.control_string(emit);
            }
            '\u{20}'..='\u{2f}' => {
                while self.chars.next_if(|&(_, ch)| ('\u{20}'..='\u{2f}').contains(&ch)).is_some() {
                }
                self.chars.next_if(|&(_, ch)| ('\u{30}'..='\u{7e}').contains(&ch));
            }
            '\u{30}'..='\u{7e}' => {
                self.chars.next();
            }
            // A lone ESC is dropped; the following character is ordinary output.
            _ => {}
        }
    }

    fn csi(&mut self, emit: &mut impl FnMut(Event<'a>)) {
        let start = self.chars.peek().map_or(self.text.len(), |&(index, _)| index);
        while let Some(&(index, ch)) = self.chars.peek() {
            if self.control.cancelled() {
                return;
            }
            match ch {
                '\u{20}'..='\u{3f}' => {
                    self.chars.next();
                }
                '\u{40}'..='\u{7e}' => {
                    self.chars.next();
                    let params = &self.text[start..index];
                    if ch == 'm'
                        && params.bytes().all(|b| b.is_ascii_digit() || b == b';' || b == b':')
                    {
                        emit(Event::Sgr(params));
                    }
                    return;
                }
                // Malformed sequence: the character is processed as output.
                _ => return,
            }
        }
    }

    /// OSC, DCS, SOS, PM, and APC strings end at BEL or ST (`ESC \`).
    fn control_string(&mut self, emit: &mut impl FnMut(Event<'a>)) {
        while let Some(&(_, ch)) = self.chars.peek() {
            if self.control.cancelled() {
                return;
            }
            match ch {
                '\u{07}' => {
                    self.chars.next();
                    return;
                }
                '\u{1b}' => {
                    self.chars.next();
                    if self.chars.next_if(|&(_, ch)| ch == '\\').is_none() {
                        self.escape(emit);
                    }
                    return;
                }
                // An unterminated string does not hide the following lines.
                '\n' => return,
                _ => {
                    self.chars.next();
                }
            }
        }
    }
}

/// Remove escape sequences, keeping all other characters.
pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    Scanner::scan(text, &LayoutControl::uninterrupted(), |event| match event {
        Event::Char(ch) | Event::Control(ch) => out.push(ch),
        Event::Newline => out.push('\n'),
        Event::CarriageReturn => out.push('\r'),
        Event::Sgr(_) => {}
    });
    out
}

/// Render output as the terminal shows it. Unstyled text carries no style, so
/// callers choose the default appearance by patching their base style underneath.
pub(crate) fn render_terminal_output(
    text: &str,
    control: &LayoutControl<'_>,
) -> Vec<Line<'static>> {
    let lines = styled_lines(text, control);
    let plain = lines
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    if diff::looks_like_unified_diff(&plain) {
        return diff::render_raw_unified_diff(&plain);
    }
    lines
}

#[derive(Default)]
struct LineBuilder {
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    run: String,
    run_style: Style,
    style: Style,
    /// A carriage return moved the cursor to the line start; the next text overwrites the line.
    overwrite: bool,
}

impl LineBuilder {
    fn push_char(&mut self, ch: char) {
        if self.overwrite {
            self.spans.clear();
            self.run.clear();
            self.overwrite = false;
        }
        if self.run.is_empty() {
            self.run_style = self.style;
        } else if self.run_style != self.style {
            self.flush_run();
            self.run_style = self.style;
        }
        self.run.push(ch);
    }

    fn flush_run(&mut self) {
        if !self.run.is_empty() {
            self.spans.push(Span::styled(std::mem::take(&mut self.run), self.run_style));
        }
    }

    fn end_line(&mut self) {
        self.flush_run();
        self.lines.push(Line::from(std::mem::take(&mut self.spans)));
        self.overwrite = false;
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_run();
        if !self.spans.is_empty() || self.lines.is_empty() {
            self.lines.push(Line::from(self.spans));
        }
        self.lines
    }
}

fn styled_lines(text: &str, control: &LayoutControl<'_>) -> Vec<Line<'static>> {
    let mut builder = LineBuilder::default();
    Scanner::scan(text, control, |event| match event {
        Event::Char(ch) => builder.push_char(ch),
        Event::Newline => builder.end_line(),
        Event::CarriageReturn => builder.overwrite = true,
        Event::Sgr(params) => builder.style = apply_sgr(builder.style, params),
        Event::Control(_) => {}
    });
    builder.finish()
}

/// Apply SGR parameters. Resets return to `Style::default()` rather than a terminal
/// reset color, so the caller's base style shows through. Blinking and concealed
/// text are not reproduced in the transcript.
fn apply_sgr(mut style: Style, params: &str) -> Style {
    let mut params = params.split(';');
    while let Some(param) = params.next() {
        let mut sub_params = param.split(':');
        let Some(code) = sub_params.next().map_or(Some(0), sgr_number) else {
            continue;
        };
        match code {
            0 => style = Style::default(),
            1 => style.add_modifier.insert(Modifier::BOLD),
            2 => style.add_modifier.insert(Modifier::DIM),
            3 => style.add_modifier.insert(Modifier::ITALIC),
            4 => style.add_modifier.insert(Modifier::UNDERLINED),
            7 => style.add_modifier.insert(Modifier::REVERSED),
            9 => style.add_modifier.insert(Modifier::CROSSED_OUT),
            22 => style.add_modifier.remove(Modifier::BOLD | Modifier::DIM),
            23 => style.add_modifier.remove(Modifier::ITALIC),
            24 => style.add_modifier.remove(Modifier::UNDERLINED),
            27 => style.add_modifier.remove(Modifier::REVERSED),
            29 => style.add_modifier.remove(Modifier::CROSSED_OUT),
            30..=37 => style.fg = Some(basic_color(code - 30)),
            39 => style.fg = None,
            40..=47 => style.bg = Some(basic_color(code - 40)),
            49 => style.bg = None,
            90..=97 => style.fg = Some(bright_color(code - 90)),
            100..=107 => style.bg = Some(bright_color(code - 100)),
            38 | 48 => {
                let color = if param.contains(':') {
                    let mut values = sub_params.collect::<Vec<_>>();
                    // The colon true-color form may carry a color-space field: `2:cs:r:g:b`.
                    if values.first() == Some(&"2") && values.len() >= 5 {
                        values.remove(1);
                    }
                    extended_color(&mut values.into_iter())
                } else {
                    extended_color(&mut params)
                };
                if let Some(color) = color {
                    if code == 38 {
                        style.fg = Some(color);
                    } else {
                        style.bg = Some(color);
                    }
                }
            }
            _ => {}
        }
    }
    style
}

fn sgr_number(value: &str) -> Option<u16> {
    if value.is_empty() { Some(0) } else { value.parse().ok() }
}

/// `5;n` selects a palette index and `2;r;g;b` a true color. Only the values
/// belonging to the color are consumed, so following attributes still apply.
fn extended_color<'p>(values: &mut impl Iterator<Item = &'p str>) -> Option<Color> {
    let mut component = || values.next()?.parse::<u8>().ok();
    match component()? {
        5 => Some(Color::Indexed(component()?)),
        2 => Some(Color::Rgb(component()?, component()?, component()?)),
        _ => None,
    }
}

fn basic_color(index: u16) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

fn bright_color(index: u16) -> Color {
    match index {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn strip_ansi_removes_csi_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m plain\u{1b}[3~ key"), "red plain key");
    }

    #[test]
    fn strip_ansi_removes_osc_sequences_ending_in_bel_or_st() {
        let input =
            "prefix\u{1b}]0;title\u{07}\u{1b}]8;;https://example.com\u{1b}\\link\u{1b}]8;;\u{1b}\\";
        assert_eq!(strip_ansi(input), "prefixlink");
    }
}
