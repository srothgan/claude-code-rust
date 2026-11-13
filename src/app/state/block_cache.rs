// SPDX-License-Identifier: Apache-2.0

use std::cell::RefCell;

/// Cached body lines and source-keyed inline content for a block. The version
/// tracks body changes; unchanged inline content survives body invalidation.
///
/// Fields are private - use `invalidate()` to mark stale, `is_stale()` to check,
/// `get()` to read cached lines, and `store()` to populate.
#[derive(Default)]
pub struct BlockCache {
    version: u64,
    rendered_version: Option<u64>,
    lines: Option<Vec<ratatui::text::Line<'static>>>,
    render_width: Option<u16>,
    /// Approximate UTF-8 byte size of cached rendered lines.
    cached_bytes: usize,
    /// Wrapped line count of the cached lines at `wrapped_width`.
    /// Computed via `Paragraph::line_count(width)` on the same lines stored in `lines`.
    wrapped_height: usize,
    /// The viewport width used to compute `wrapped_height`.
    wrapped_width: u16,
    wrapped_height_valid: bool,
    /// Source-keyed inline content survives body invalidation and width changes.
    inline: RefCell<Option<InlineCache>>,
}

struct InlineCache {
    source: String,
    literal: bool,
    spans: Vec<ratatui::text::Span<'static>>,
}

impl BlockCache {
    /// Memoize inline content independently of animated chrome and body versions.
    pub(crate) fn inline_spans(
        &self,
        source: &str,
        literal: bool,
        render: impl FnOnce(&str) -> Vec<ratatui::text::Span<'static>>,
    ) -> Vec<ratatui::text::Span<'static>> {
        if let Some(cached) = self.inline.borrow().as_ref()
            && cached.source == source
            && cached.literal == literal
        {
            return cached.spans.clone();
        }
        let spans = render(source);
        *self.inline.borrow_mut() =
            Some(InlineCache { source: source.to_owned(), literal, spans: spans.clone() });
        spans
    }

    /// Source changes advance this revision; storing or evicting derived rows does not.
    pub(crate) const fn source_revision(&self) -> u64 {
        self.version
    }

    /// Bump the version to invalidate cached lines and height.
    pub fn invalidate(&mut self) {
        self.version = self.version.wrapping_add(1);
        self.wrapped_height_valid = false;
    }

    /// Get a reference to the cached lines, if fresh.
    #[must_use]
    pub fn get(&self) -> Option<&Vec<ratatui::text::Line<'static>>> {
        if self.rendered_version == Some(self.version) && self.render_width.is_none() {
            self.lines.as_ref()
        } else {
            None
        }
    }

    #[must_use]
    pub fn get_for_width(&self, width: u16) -> Option<&Vec<ratatui::text::Line<'static>>> {
        if self.rendered_version == Some(self.version) && self.render_width == Some(width) {
            self.lines.as_ref()
        } else {
            None
        }
    }

    /// Store freshly rendered lines, marking the cache as clean.
    /// Height is set separately via `set_height()` after measurement.
    pub fn store(&mut self, lines: Vec<ratatui::text::Line<'static>>) {
        self.store_rows(lines, None);
    }

    pub fn store_for_width(&mut self, lines: Vec<ratatui::text::Line<'static>>, width: u16) {
        self.store_rows(lines, Some(width));
    }

    fn store_rows(&mut self, lines: Vec<ratatui::text::Line<'static>>, width: Option<u16>) {
        self.cached_bytes =
            lines.iter().fold(0usize, |total, line| total.saturating_add(line_utf8_bytes(line)));
        self.lines = Some(lines);
        self.render_width = width;
        self.rendered_version = Some(self.version);
        self.wrapped_height = 0;
        self.wrapped_width = 0;
        self.wrapped_height_valid = false;
    }

    /// Set the wrapped height for the cached lines at the given width.
    /// Called by the viewport/chat layer after `Paragraph::line_count(width)`.
    /// Separate from `store()` so height measurement is the viewport's job.
    pub fn set_height(&mut self, height: usize, width: u16) {
        self.wrapped_height = height;
        self.wrapped_width = width;
        self.wrapped_height_valid = true;
    }

    /// Get the cached wrapped height if cache is valid and was computed at the given width.
    #[must_use]
    pub fn height_at(&self, width: u16) -> Option<usize> {
        if self.rendered_version == Some(self.version)
            && self.wrapped_height_valid
            && self.wrapped_width == width
        {
            Some(self.wrapped_height)
        } else {
            None
        }
    }

    #[must_use]
    pub fn cached_bytes(&self) -> usize {
        let inline_bytes = self.inline.borrow().as_ref().map_or(0, |cached| {
            cached.spans.iter().fold(cached.source.len().saturating_add(1), |bytes, span| {
                bytes.saturating_add(span.content.len())
            })
        });
        self.cached_bytes.saturating_add(inline_bytes)
    }

    pub fn evict_cached_render(&mut self) -> usize {
        let removed = self.cached_bytes();
        if removed == 0 {
            return 0;
        }
        self.lines = None;
        self.render_width = None;
        self.cached_bytes = 0;
        *self.inline.get_mut() = None;
        self.wrapped_height = 0;
        self.wrapped_width = 0;
        self.wrapped_height_valid = false;
        self.rendered_version = None;
        removed
    }
}

fn line_utf8_bytes(line: &ratatui::text::Line<'static>) -> usize {
    let span_bytes =
        line.spans.iter().fold(0usize, |acc, span| acc.saturating_add(span.content.len()));
    span_bytes.saturating_add(1)
}
