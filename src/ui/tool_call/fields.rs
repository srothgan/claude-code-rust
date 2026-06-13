// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Shared field rendering for compact typed tool-call bodies.

use std::borrow::Cow;

use crate::ui::theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ToolField<'a> {
    label: &'static str,
    value: Cow<'a, str>,
}

impl<'a> ToolField<'a> {
    pub(super) fn new(value_label: &'static str, value: impl Into<Cow<'a, str>>) -> Self {
        Self { label: value_label, value: value.into() }
    }
}

pub(super) fn render_fields<'a>(
    fields: impl IntoIterator<Item = ToolField<'a>>,
) -> Vec<Line<'static>> {
    fields.into_iter().map(render_field_line).collect()
}

pub(super) fn render_field<'a>(
    value_label: &'static str,
    value: impl Into<Cow<'a, str>>,
) -> Line<'static> {
    render_field_line(ToolField::new(value_label, value))
}

pub(super) fn render_dynamic_field<'a>(
    value_label: impl Into<String>,
    value: impl Into<Cow<'a, str>>,
) -> Line<'static> {
    let value_label = value_label.into();
    Line::from(vec![
        Span::styled(format!("{value_label}: "), Style::default().fg(theme::DIM)),
        Span::raw(value.into().into_owned()),
    ])
}

pub(super) fn render_multiline_field<'a>(
    value_label: &'static str,
    value: impl Into<Cow<'a, str>>,
) -> Vec<Line<'static>> {
    let value = value.into().into_owned();
    let mut value_lines = value.lines();
    let Some(first) = value_lines.next() else {
        return vec![render_field(value_label, "")];
    };

    let continuation_prefix = " ".repeat(value_label.len().saturating_add(2));
    let mut lines = vec![render_field(value_label, first.to_owned())];
    lines.extend(value_lines.map(|line| {
        Line::from(vec![
            Span::styled(continuation_prefix.clone(), Style::default().fg(theme::DIM)),
            Span::raw(line.to_owned()),
        ])
    }));
    lines
}

fn render_field_line(field: ToolField<'_>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{}: ", field.label), Style::default().fg(theme::DIM)),
        Span::raw(field.value.into_owned()),
    ])
}
