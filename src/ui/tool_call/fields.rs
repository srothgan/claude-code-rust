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

fn render_field_line(field: ToolField<'_>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{}: ", field.label), Style::default().fg(theme::DIM)),
        Span::raw(field.value.into_owned()),
    ])
}
