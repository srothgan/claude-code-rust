// SPDX-License-Identifier: Apache-2.0

use crate::agent::model;
use crate::app::footer::{FooterFieldId, FooterLayout, FooterSide};
use crate::app::{App, MessageBlock, MessageRole};
use crate::ui::theme;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const FOOTER_CONTEXT_VALUE: Color = Color::Gray;
const FIELD_SEPARATOR: &str = " · ";

type FooterItem = Option<(String, Color)>;

pub(crate) struct SerializedFooterRows {
    pub rows: [Line<'static>; 1],
}

pub(crate) fn serialize_footer_rows(app: &App, total_width: u16) -> SerializedFooterRows {
    let layout = app.config.footer.layout();
    SerializedFooterRows { rows: [serialize_footer(app, &layout, total_width)] }
}

/// Format the configured fields and notices within the chat footer's single row.
pub(crate) fn serialize_footer(app: &App, layout: &FooterLayout, width: u16) -> Line<'static> {
    let width = usize::from(width);
    let notice = footer_notice(app)
        .and_then(|(text, color)| fit_footer_right_text(&text, width).map(|text| (text, color)));
    let notice_width = notice.as_ref().map(|(text, _)| text.width());
    let mut fields: Vec<_> = layout
        .fields()
        .iter()
        .filter(|field| field.visible)
        .filter_map(|field| {
            format_field(app, field.id).map(|(text, color)| RenderedField {
                id: field.id,
                side: field.side,
                text,
                color: field.color.map_or(color, crate::app::footer::FooterColor::color),
            })
        })
        .collect();
    let mut overflow = fields_width(&fields, notice_width).saturating_sub(width);
    // Flexible workspace values shrink before any status badge is omitted.
    for field in fields.iter_mut().rev() {
        let minimum = match field.id {
            FooterFieldId::Directory => 10,
            FooterFieldId::GitBranch => 12,
            _ => continue,
        };
        let reduction = overflow.min(field.text.width().saturating_sub(minimum));
        let target = field.text.width().saturating_sub(reduction);
        if reduction > 0 {
            field.text = if field.id == FooterFieldId::Directory {
                format!(
                    "Loc: {}",
                    fit_location_value(&app.cwd, target.saturating_sub(5)).unwrap_or_default()
                )
            } else {
                fit_footer_right_text(&field.text, target).unwrap_or_default()
            };
            overflow = overflow.saturating_sub(reduction);
        }
    }
    // Earlier entries take precedence; groups never overlap or wrap.
    while fields_width(&fields, notice_width) > width {
        fields.pop();
    }
    let mut left = group_spans(&fields, FooterSide::Left);
    let mut right = group_spans(&fields, FooterSide::Right);
    if let Some((text, color)) = notice {
        if !right.is_empty() {
            right.push(Span::styled(FIELD_SEPARATOR, Style::default().fg(theme::DIM)));
        }
        right.push(Span::styled(text, Style::default().fg(color)));
    }
    let occupied: usize = left.iter().chain(&right).map(Span::width).sum();
    if !right.is_empty() {
        left.push(Span::raw(" ".repeat(width.saturating_sub(occupied))));
        left.extend(right);
    }
    Line::from(left)
}

struct RenderedField {
    id: FooterFieldId,
    side: FooterSide,
    text: String,
    color: Color,
}

fn fields_width(fields: &[RenderedField], notice_width: Option<usize>) -> usize {
    let left_count = fields.iter().filter(|field| field.side == FooterSide::Left).count();
    let right_count = fields.len() - left_count + usize::from(notice_width.is_some());
    fields.iter().map(|field| field.text.width()).sum::<usize>()
        + notice_width.unwrap_or(0)
        + (left_count.saturating_sub(1) + right_count.saturating_sub(1)) * FIELD_SEPARATOR.width()
        + usize::from(left_count > 0 && right_count > 0)
}

fn group_spans(fields: &[RenderedField], side: FooterSide) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for field in fields.iter().filter(|field| field.side == side) {
        if !spans.is_empty() {
            spans.push(Span::styled(FIELD_SEPARATOR, Style::default().fg(theme::DIM)));
        }
        spans.push(Span::styled(field.text.clone(), Style::default().fg(field.color)));
    }
    spans
}

fn format_field(app: &App, id: FooterFieldId) -> FooterItem {
    let gray = FOOTER_CONTEXT_VALUE;
    match id {
        FooterFieldId::PermissionMode => app.session_runtime.mode.as_ref().map(|mode| {
            (format!("[{}]", mode.current_mode_name), mode_color(&mode.current_mode_id))
        }),
        FooterFieldId::Model => app
            .session_runtime
            .current_model
            .as_ref()
            .map(|model| (format!("[{}]", model.display_name_short), gray)),
        FooterFieldId::Effort => app
            .session_runtime
            .current_model
            .as_ref()
            .filter(|model| model.supports_effort)
            .and_then(|_| app.session_effort())
            .map(|effort| (format!("[{}]", footer_effort_label(effort)), gray)),
        FooterFieldId::Ultracode => app
            .session_runtime
            .ultracode
            .filter(|state| state.effective())
            .map(|_| ("[Ultracode]".to_owned(), theme::ULTRACODE_ACCENT)),
        FooterFieldId::FastMode => {
            let (text, color) = fast_mode_badge(app.session_runtime.fast_mode_state);
            Some((format!("[{text}]"), color))
        }
        FooterFieldId::RunningSubagents => {
            let running = crate::app::agents::running_subagent_count(app);
            (running > 0).then(|| (format!("[AGENTS:{running}]"), Color::Yellow))
        }
        FooterFieldId::Directory => Some((format!("Loc: {}", app.cwd), gray)),
        FooterFieldId::GitBranch => app
            .git_branch()
            .filter(|branch| !branch.is_empty())
            .map(|branch| (format!("Branch: {branch}"), gray)),
        FooterFieldId::ContextRemaining => app
            .session_runtime
            .session_usage
            .context_usage_percent
            .map(|used| (format!("Context: {}%", 100_u8.saturating_sub(used)), gray)),
    }
}

fn footer_notice(app: &App) -> FooterItem {
    let permissions = pending_permission_request_count(app);
    if permissions > 0 {
        return Some((format!("{permissions} PEND. PERM."), Color::Yellow));
    }
    let auth = mcp_needs_auth_count(app);
    if auth > 0 && should_show_startup_mcp_hint(app) {
        return Some((format!("{auth} MCP NEEDS AUTH"), Color::Yellow));
    }
    app.session_runtime
        .mode
        .is_none()
        .then(|| startup_status_badge(app))
        .flatten()
        .map(|(text, color)| (format!("[{text}]"), color))
}

const fn footer_effort_label(effort: model::EffortLevel) -> &'static str {
    match effort {
        model::EffortLevel::Low => "Low",
        model::EffortLevel::Medium => "Med",
        model::EffortLevel::High => "High",
        model::EffortLevel::XHigh => "XHigh",
        model::EffortLevel::Max => "Max",
    }
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
                app.transcript.messages.get(mi).and_then(|msg| msg.blocks.get(bi)),
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
            matches!(server.status, crate::agent::model::McpServerConnectionStatus::NeedsAuth)
        })
        .count()
}

fn should_show_startup_mcp_hint(app: &App) -> bool {
    !app.transcript
        .messages
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
        model::FastModeState::Unknown => ("FAST:?", theme::DIM),
        model::FastModeState::Off => ("FAST:OFF", theme::DIM),
        model::FastModeState::Cooldown => ("FAST:CD", Color::Yellow),
        model::FastModeState::On => ("FAST:ON", theme::RUST_ORANGE),
    }
}

fn startup_status_badge(app: &App) -> Option<(&'static str, Color)> {
    match app.displayed_status() {
        crate::app::AppStatus::Connecting => None,
        crate::app::AppStatus::Ready => Some(("READY", theme::DIM)),
        crate::app::AppStatus::CommandPending
        | crate::app::AppStatus::Thinking
        | crate::app::AppStatus::Running => Some(("WORKING", Color::Yellow)),
        crate::app::AppStatus::Error => Some(("ERROR", theme::STATUS_ERROR)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model::{McpServerConnectionStatus, McpServerStatus};
    use crate::app::ModeState;

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn app_with_mode() -> App {
        let mut app = App::test_default();
        app.session_runtime.mode = Some(ModeState {
            current_mode_id: "default".to_owned(),
            current_mode_name: "default".to_owned(),
            available_modes: Vec::new(),
        });
        app
    }

    #[test]
    fn mcp_auth_notice_and_context_are_both_visible_when_space_allows() {
        let mut app = App::test_default();
        app.session_runtime.session_usage.context_usage_percent = Some(62);
        app.mcp.servers.push(McpServerStatus {
            name: "filesystem".to_owned(),
            status: McpServerConnectionStatus::NeedsAuth,
            server_info: None,
            error: None,
            config: None,
            scope: None,
            source: None,
            tools: Vec::new(),
        });

        let serialized = serialize_footer_rows(&app, 160);
        let text = line_text(&serialized.rows[0]);
        assert!(text.contains("1 MCP NEEDS AUTH"));
        assert!(text.contains("Context: 38%"));
    }

    #[test]
    fn directory_and_branch_render_without_each_other() {
        let mut app = app_with_mode();
        app.cwd = "~/work/claude_rust".into();
        app.set_git_branch_for_test(Some("feature/footer"));
        for (id, expected, hidden) in [
            ("directory", "Loc: ~/work/claude_rust", "Branch:"),
            ("gitBranch", "Branch: feature/footer", "Loc:"),
        ] {
            let layout = FooterLayout::from_saved(Some(&serde_json::json!([
                {"id":id,"visible":true,"side":"left"}
            ])))
            .expect("layout");
            let text = line_text(&serialize_footer(&app, &layout, 80));
            assert!(text.contains(expected), "{text}");
            assert!(!text.contains(hidden), "{text}");
        }
    }

    #[test]
    fn effort_and_ultracode_do_not_require_a_visible_model() {
        let mut app = app_with_mode();
        app.session_runtime.current_model =
            Some(model::CurrentModel::new("opus", "Opus", "Opus").supports_effort(true));
        app.session_runtime.config_options.insert("effortLevel".into(), serde_json::json!("high"));
        app.session_runtime.ultracode = model::UltracodeState::new(true, true, true);
        let layout = FooterLayout::from_saved(Some(&serde_json::json!([
            {"id":"effort","visible":true,"side":"left"},
            {"id":"ultracode","visible":true,"side":"right"}
        ])))
        .expect("layout");
        let text = line_text(&serialize_footer(&app, &layout, 80));
        assert!(text.starts_with("[High]"));
        assert!(text.ends_with("[Ultracode]"));
        assert!(!text.contains("Opus"));
        app.session_runtime.current_model.as_mut().expect("model").supports_effort = false;
        assert!(!line_text(&serialize_footer(&app, &layout, 80)).contains("High"));
    }
}

#[cfg(test)]
#[path = "footer_audit_tests.rs"]
mod audit_tests;
