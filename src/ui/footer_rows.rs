// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use crate::app::{App, MessageBlock, MessageRole};
use crate::ui::theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const FOOTER_COLUMN_GAP: u16 = 1;
const PRIMARY_ROW_LEFT_MIN_WIDTH: u16 = 24;
const SECONDARY_ROW_LEFT_MIN_WIDTH: u16 = 28;
const MIN_CONTEXT_LOCATION_WIDTH: usize = 10;
const MIN_CONTEXT_BRANCH_WIDTH: usize = 4;
const FOOTER_CONTEXT_VALUE: Color = Color::Gray;

type FooterItem = Option<(String, Color)>;

pub(crate) struct SerializedFooterRows {
    pub rows: [Line<'static>; 2],
}

pub(crate) fn serialize_footer_rows(app: &App, total_width: u16) -> SerializedFooterRows {
    let first_row = compose_footer_row(
        build_primary_line(app),
        footer_primary_hint(app),
        total_width,
        PRIMARY_ROW_LEFT_MIN_WIDTH,
    );

    let second_hint = footer_secondary_hint(app);
    let second_left_width = footer_row_widths(
        total_width,
        second_hint.as_ref().map(|(text, _)| text.as_str()),
        SECONDARY_ROW_LEFT_MIN_WIDTH,
    )
    .0;
    let second_row = compose_footer_row(
        build_context_line(app, usize::from(second_left_width)),
        second_hint,
        total_width,
        SECONDARY_ROW_LEFT_MIN_WIDTH,
    );

    SerializedFooterRows { rows: [first_row, second_row] }
}

fn footer_primary_hint(app: &App) -> FooterItem {
    let permission_count = pending_permission_request_count(app);
    if permission_count > 0 {
        return Some((format!("{permission_count} PEND. PERM."), Color::Yellow));
    }
    None
}

fn footer_mcp_auth_hint(app: &App) -> FooterItem {
    let needs_auth_count = mcp_needs_auth_count(app);
    (needs_auth_count > 0 && should_show_startup_mcp_hint(app))
        .then(|| (format!("{needs_auth_count} MCP NEEDS AUTH"), Color::Yellow))
}

fn footer_context_usage_hint(app: &App) -> FooterItem {
    app.session_usage.context_usage_percent.map(|percentage| {
        let remaining = 100_u8.saturating_sub(percentage);
        (format!("{remaining}%"), FOOTER_CONTEXT_VALUE)
    })
}

fn footer_secondary_hint(app: &App) -> FooterItem {
    footer_mcp_auth_hint(app).or_else(|| footer_context_usage_hint(app))
}

fn footer_row_widths(
    total_width: u16,
    right_text: Option<&str>,
    left_min_width: u16,
) -> (u16, u16) {
    if total_width == 0 {
        return (0, 0);
    }

    let Some(right_text) = right_text else {
        return (total_width, 0);
    };

    let left_min_width = left_min_width.min(total_width);
    let available_right =
        total_width.saturating_sub(left_min_width).saturating_sub(FOOTER_COLUMN_GAP);
    if available_right == 0 {
        return (total_width, 0);
    }

    let natural_right_width = u16::try_from(UnicodeWidthStr::width(right_text)).unwrap_or(u16::MAX);
    let right_width = natural_right_width.min(available_right);
    if right_width == 0 {
        return (total_width, 0);
    }

    let left_width = total_width.saturating_sub(right_width).saturating_sub(FOOTER_COLUMN_GAP);
    (left_width, right_width)
}

fn compose_footer_row(
    left: Line<'static>,
    right: FooterItem,
    total_width: u16,
    left_min_width: u16,
) -> Line<'static> {
    if total_width == 0 {
        return Line::default();
    }

    let (left_width, right_width) = footer_row_widths(
        total_width,
        right.as_ref().map(|(text, _)| text.as_str()),
        left_min_width,
    );

    let area = Rect::new(0, 0, total_width, 1);
    let mut buf = Buffer::empty(area);
    let left_area = Rect::new(0, 0, left_width, 1);
    Paragraph::new(left).render(left_area, &mut buf);

    if let Some((right_text, right_color)) = right
        && right_width > 0
        && let Some(fitted) = fit_footer_right_text(&right_text, usize::from(right_width))
    {
        let right_x = left_width.saturating_add(FOOTER_COLUMN_GAP);
        let right_area = Rect::new(right_x, 0, right_width, 1);
        let line = Line::from(Span::styled(fitted, Style::default().fg(right_color)));
        Paragraph::new(line).alignment(Alignment::Right).render(right_area, &mut buf);
    }

    buffer_row_to_line(&buf, area, 0)
}

fn build_primary_line(app: &App) -> Line<'static> {
    if let Some(ref mode) = app.mode {
        let color = mode_color(&mode.current_mode_id);
        let (fast_mode_text, fast_mode_color) = fast_mode_badge(app.fast_mode_state);
        let mut spans = Vec::new();
        push_badge(&mut spans, mode.current_mode_name.clone(), color);
        if let Some(model_badge) = footer_model_badge(app) {
            spans.push(Span::raw("  "));
            push_badge(&mut spans, model_badge, FOOTER_CONTEXT_VALUE);
        }
        spans.push(Span::raw("  "));
        push_badge(&mut spans, fast_mode_text.to_owned(), fast_mode_color);
        Line::from(spans)
    } else {
        let (fast_mode_text, fast_mode_color) = fast_mode_badge(app.fast_mode_state);
        let mut spans = Vec::new();
        if let Some((status_text, status_color)) = startup_status_badge(app) {
            push_badge(&mut spans, status_text.to_owned(), status_color);
            spans.push(Span::raw("  "));
        }
        push_badge(&mut spans, fast_mode_text.to_owned(), fast_mode_color);
        Line::from(spans)
    }
}

fn push_badge(spans: &mut Vec<Span<'static>>, text: String, color: Color) {
    spans.push(Span::styled("[", Style::default().fg(color)));
    spans.push(Span::styled(text, Style::default().fg(color)));
    spans.push(Span::styled("]", Style::default().fg(color)));
}

fn footer_model_badge(app: &App) -> Option<String> {
    let current_model = app.current_model.as_ref()?;
    let mut badge = current_model.display_name_short.clone();
    if current_model.supports_effort {
        badge.push('/');
        badge.push_str(footer_effort_label(app.config.thinking_effort_effective()));
    }
    Some(badge)
}

const fn footer_effort_label(effort: model::EffortLevel) -> &'static str {
    match effort {
        model::EffortLevel::Low => "Low",
        model::EffortLevel::Medium => "Med",
        model::EffortLevel::High => "High",
    }
}

fn build_context_line(app: &App, max_width: usize) -> Line<'static> {
    let Some((location_value, branch_value)) = context_values(app, max_width) else {
        return Line::default();
    };

    let mut spans = vec![
        Span::styled("Loc: ", Style::default().fg(theme::DIM)),
        Span::styled(location_value, Style::default().fg(FOOTER_CONTEXT_VALUE)),
    ];

    if let Some(branch_value) = branch_value {
        spans.push(Span::styled(" (", Style::default().fg(theme::DIM)));
        spans.push(Span::styled(branch_value, Style::default().fg(FOOTER_CONTEXT_VALUE)));
        spans.push(Span::styled(")", Style::default().fg(theme::DIM)));
    }

    Line::from(spans)
}

fn context_values(app: &App, max_width: usize) -> Option<(String, Option<String>)> {
    const LOCATION_LABEL_WIDTH: usize = 5;
    const BRANCH_WRAP_WIDTH: usize = 3;

    let location_only_width = max_width.saturating_sub(LOCATION_LABEL_WIDTH);
    let branch = app.git_branch().filter(|branch| !branch.is_empty());

    if let Some(branch) = branch {
        let fixed_width = LOCATION_LABEL_WIDTH + BRANCH_WRAP_WIDTH;
        let available_values = max_width.saturating_sub(fixed_width);
        if available_values >= MIN_CONTEXT_LOCATION_WIDTH + MIN_CONTEXT_BRANCH_WIDTH {
            let branch_width = UnicodeWidthStr::width(branch)
                .min(available_values.saturating_sub(MIN_CONTEXT_LOCATION_WIDTH));
            let branch_value = fit_footer_right_text(branch, branch_width);
            let branch_display_width =
                branch_value.as_ref().map_or(0, |value| UnicodeWidthStr::width(value.as_str()));
            let location_width = available_values.saturating_sub(branch_display_width);
            if let Some(location_value) = fit_location_value(&app.cwd, location_width) {
                return Some((location_value, branch_value));
            }
        }
    }

    fit_location_value(&app.cwd, location_only_width).map(|location_value| (location_value, None))
}

fn fit_location_value(cwd: &str, max_width: usize) -> Option<String> {
    if max_width == 0 {
        return None;
    }

    for candidate in location_candidates(cwd) {
        if UnicodeWidthStr::width(candidate.as_str()) <= max_width {
            return Some(candidate);
        }
    }

    fit_footer_suffix_text(cwd, max_width)
}

fn location_candidates(cwd: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    push_unique(&mut candidates, Some(cwd.to_owned()));
    push_unique(&mut candidates, trailing_path_components(cwd, 2));
    push_unique(&mut candidates, trailing_path_components(cwd, 1));
    candidates
}

fn trailing_path_components(path: &str, count: usize) -> Option<String> {
    let separator = if path.contains('\\') { "\\" } else { "/" };
    let components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty() && *component != "~")
        .collect();
    if components.is_empty() {
        return None;
    }
    let start = components.len().saturating_sub(count);
    Some(components[start..].join(separator))
}

fn push_unique(candidates: &mut Vec<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn fit_footer_right_text(text: &str, max_width: usize) -> Option<String> {
    if max_width == 0 || text.trim().is_empty() {
        return None;
    }

    if UnicodeWidthStr::width(text) <= max_width {
        return Some(text.to_owned());
    }

    if max_width <= 3 {
        return Some(".".repeat(max_width));
    }

    let mut fitted = String::new();
    let mut width: usize = 0;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width).saturating_add(3) > max_width {
            break;
        }
        fitted.push(ch);
        width = width.saturating_add(ch_width);
    }

    if fitted.is_empty() {
        return Some("...".to_owned());
    }
    fitted.push_str("...");
    Some(fitted)
}

fn fit_footer_suffix_text(text: &str, max_width: usize) -> Option<String> {
    if max_width == 0 || text.trim().is_empty() {
        return None;
    }

    if UnicodeWidthStr::width(text) <= max_width {
        return Some(text.to_owned());
    }

    if max_width <= 3 {
        return Some(".".repeat(max_width));
    }

    let mut fitted = String::new();
    let mut width = 0usize;
    for ch in text.chars().rev() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width.saturating_add(ch_width).saturating_add(3) > max_width {
            break;
        }
        fitted.insert(0, ch);
        width = width.saturating_add(ch_width);
    }

    if fitted.is_empty() {
        return Some("...".to_owned());
    }

    Some(format!("...{fitted}"))
}

fn pending_permission_request_count(app: &App) -> usize {
    app.pending_interaction_ids
        .iter()
        .filter(|tool_id| {
            let Some((mi, bi)) = app.lookup_tool_call(tool_id) else {
                return false;
            };
            matches!(
                app.messages.get(mi).and_then(|msg| msg.blocks.get(bi)),
                Some(MessageBlock::ToolCall(tc)) if tc.pending_permission.is_some()
            )
        })
        .count()
}

fn mcp_needs_auth_count(app: &App) -> usize {
    app.mcp
        .servers
        .iter()
        .filter(|server| {
            matches!(server.status, crate::agent::types::McpServerConnectionStatus::NeedsAuth)
        })
        .count()
}

fn should_show_startup_mcp_hint(app: &App) -> bool {
    !app.messages
        .iter()
        .any(|message| matches!(message.role, MessageRole::User | MessageRole::Assistant))
}

fn mode_color(mode_id: &str) -> Color {
    match mode_id {
        "default" => theme::DIM,
        "auto" | "acceptEdits" => Color::Yellow,
        "plan" => Color::Blue,
        "bypassPermissions" | "dontAsk" => Color::Red,
        _ => Color::Magenta,
    }
}

fn fast_mode_badge(state: model::FastModeState) -> (&'static str, Color) {
    match state {
        model::FastModeState::Off => ("FAST:OFF", theme::DIM),
        model::FastModeState::Cooldown => ("FAST:CD", Color::Yellow),
        model::FastModeState::On => ("FAST:ON", theme::RUST_ORANGE),
    }
}

fn startup_status_badge(app: &App) -> Option<(&'static str, Color)> {
    match app.status {
        crate::app::AppStatus::Connecting => None,
        crate::app::AppStatus::Ready => Some(("READY", theme::DIM)),
        crate::app::AppStatus::CommandPending
        | crate::app::AppStatus::Thinking
        | crate::app::AppStatus::Running => Some(("WORKING", Color::Yellow)),
        crate::app::AppStatus::Error => Some(("ERROR", theme::STATUS_ERROR)),
    }
}

fn buffer_row_to_line(buf: &Buffer, area: Rect, row: u16) -> Line<'static> {
    let y = area.y.saturating_add(row);
    let mut cells = Vec::with_capacity(usize::from(area.width));
    for x in 0..area.width {
        if let Some(cell) = buf.cell((area.x.saturating_add(x), y)) {
            cells.push((cell.symbol().to_owned(), cell.style()));
        }
    }

    let Some(last_non_blank) = cells
        .iter()
        .rposition(|(symbol, _)| !symbol.is_empty() && !symbol.chars().all(char::is_whitespace))
    else {
        return Line::default();
    };

    let mut spans = Vec::new();
    let mut current_style: Option<Style> = None;
    let mut current_text = String::new();

    for (symbol, style) in cells.into_iter().take(last_non_blank + 1) {
        if symbol.is_empty() {
            continue;
        }
        match current_style {
            Some(existing) if existing == style => current_text.push_str(&symbol),
            Some(existing) => {
                spans.push(Span::styled(std::mem::take(&mut current_text), existing));
                current_text.push_str(&symbol);
                current_style = Some(style);
            }
            None => {
                current_text.push_str(&symbol);
                current_style = Some(style);
            }
        }
    }

    if let Some(style) = current_style
        && !current_text.is_empty()
    {
        spans.push(Span::styled(current_text, style));
    }

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{McpServerConnectionStatus, McpServerStatus};
    use crate::app::{
        App, AppStatus, BlockCache, ChatMessage, InlinePermission, MessageBlock, MessageRole,
        ModeState, TerminalSnapshotMode, ToolCallInfo,
    };
    use tokio::sync::oneshot;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn app_with_mode() -> App {
        let mut app = App::test_default();
        app.mode = Some(ModeState {
            current_mode_id: "default".to_owned(),
            current_mode_name: "default".to_owned(),
            available_modes: Vec::new(),
        });
        app
    }

    #[test]
    fn footer_row_widths_left_gets_minimum() {
        let (left, right) = footer_row_widths(80, Some("1 PEND. PERM."), 24);
        assert_eq!(left + FOOTER_COLUMN_GAP + right, 80);
        assert!(left >= 24);
    }

    #[test]
    fn footer_row_widths_drops_right_when_left_min_cannot_be_preserved() {
        let (left, right) = footer_row_widths(24, Some("1 MCP NEEDS AUTH"), 24);
        assert_eq!(left, 24);
        assert_eq!(right, 0);
    }

    #[test]
    fn fit_footer_right_text_truncates_when_needed() {
        let text = "Update available: v9.9.9 (current v0.2.0)";
        let fitted = fit_footer_right_text(text, 12).expect("fitted text");
        assert!(fitted.ends_with("..."));
        assert!(UnicodeWidthStr::width(fitted.as_str()) <= 12);
    }

    #[test]
    fn fit_footer_suffix_text_keeps_path_tail() {
        let text = "~/work/company/claude_rust";
        let fitted = fit_footer_suffix_text(text, 14).expect("fitted text");
        assert!(fitted.starts_with("..."));
        assert!(fitted.ends_with("claude_rust"));
        assert!(UnicodeWidthStr::width(fitted.as_str()) <= 14);
    }

    #[test]
    fn serialize_footer_rows_produces_exactly_two_rows() {
        let app = App::test_default();
        let serialized = serialize_footer_rows(&app, 80);

        assert_eq!(serialized.rows.len(), 2);
    }

    #[test]
    fn first_row_preserves_mode_and_fast_mode_badges() {
        let app = app_with_mode();
        let serialized = serialize_footer_rows(&app, 80);
        let text = line_text(&serialized.rows[0]);

        assert!(text.contains("[default]"));
        assert!(text.contains("[FAST:OFF]"));
    }

    #[test]
    fn first_row_omits_connecting_badge_when_mode_is_unset() {
        let mut app = App::test_default();
        app.status = AppStatus::Connecting;

        let serialized = serialize_footer_rows(&app, 80);
        let text = line_text(&serialized.rows[0]);

        assert!(!text.contains("[CONNECTING]"));
        assert!(text.contains("[FAST:OFF]"));
    }

    #[test]
    fn second_row_preserves_context_line_content() {
        let app = App::test_default();
        let serialized = serialize_footer_rows(&app, 80);
        let text = line_text(&serialized.rows[1]);

        assert!(text.contains("Loc:"));
    }

    #[test]
    fn pending_permission_hint_wins_on_first_row() {
        let mut app = App::test_default();
        let (response_tx, _response_rx) = oneshot::channel();
        app.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
                id: "perm-1".into(),
                title: "Read".into(),
                sdk_tool_name: "Read".into(),
                raw_input: None,
                raw_input_bytes: 0,
                output_metadata: None,
                task_metadata: None,
                status: model::ToolCallStatus::Pending,
                content: vec![],
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: None,
                terminal_output_len: 0,
                terminal_bytes_seen: 0,
                terminal_snapshot_mode: TerminalSnapshotMode::AppendOnly,
                cache: BlockCache::default(),
                pending_permission: Some(InlinePermission {
                    options: vec![],
                    display: None,
                    response_tx,
                    selected_index: 0,
                    focused: true,
                }),
                pending_question: None,
            }))],
            None,
        ));
        app.index_tool_call("perm-1".into(), 0, 0);
        app.pending_interaction_ids.push("perm-1".into());

        let serialized = serialize_footer_rows(&app, 80);
        let text = line_text(&serialized.rows[0]);
        assert!(text.contains("1 PEND. PERM."));
    }

    #[test]
    fn mcp_auth_hint_wins_over_context_usage_on_second_row() {
        let mut app = App::test_default();
        app.session_usage.context_usage_percent = Some(62);
        app.mcp.servers.push(McpServerStatus {
            name: "filesystem".to_owned(),
            status: McpServerConnectionStatus::NeedsAuth,
            server_info: None,
            error: None,
            config: None,
            scope: None,
            tools: Vec::new(),
        });

        let serialized = serialize_footer_rows(&app, 80);
        let text = line_text(&serialized.rows[1]);
        assert!(text.contains("1 MCP NEEDS AUTH"));
        assert!(!text.contains("38%"));
    }

    #[test]
    fn narrow_width_truncates_right_hint_but_preserves_left_content() {
        let mut app = App::test_default();
        app.mcp.servers.push(McpServerStatus {
            name: "filesystem".to_owned(),
            status: McpServerConnectionStatus::NeedsAuth,
            server_info: None,
            error: None,
            config: None,
            scope: None,
            tools: Vec::new(),
        });

        let serialized = serialize_footer_rows(&app, 36);
        let text = line_text(&serialized.rows[1]);
        assert!(text.contains("Loc:"));
        assert!(text.contains("..."));
    }

    #[test]
    fn context_location_and_branch_truncation_matches_helpers() {
        let mut app = App::test_default();
        app.set_git_branch_for_test(Some("feature/footer"));

        let serialized = serialize_footer_rows(&app, 24);
        let text = line_text(&serialized.rows[1]);
        assert!(text.contains("Loc:"));
    }
}
