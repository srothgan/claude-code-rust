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
    use crate::app::{
        App, AppStatus, BlockCache, ChatMessage, InlinePermission, MessageBlock, MessageRole,
        ModeState, ToolCallInfo,
    };
    use tokio::sync::oneshot;

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
    fn serialize_footer_rows_produces_exactly_one_row() {
        let app = App::test_default();
        let serialized = serialize_footer_rows(&app, 80);

        assert_eq!(serialized.rows.len(), 1);
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
    fn footer_counts_running_subagents_and_hides_the_count_at_zero() {
        let subagent = |id: &str, status, terminal: Option<&str>| {
            let mut task = model::TaskItem::new(id, id, status);
            task.metadata = Some(
                serde_json::json!({ "task_type": "local_agent", "terminal_status": terminal }),
            );
            task
        };
        let mut shell = model::TaskItem::new("shell", "shell", model::TaskStatus::InProgress);
        shell.metadata = Some(serde_json::json!({ "task_type": "local_bash" }));
        let mut app = app_with_mode();
        app.sdk_inventory.tasks = vec![
            subagent("agent-1", model::TaskStatus::InProgress, None),
            subagent("agent-2", model::TaskStatus::InProgress, None),
            subagent("agent-3", model::TaskStatus::Completed, Some("completed")),
            shell,
        ];
        let text = line_text(&serialize_footer_rows(&app, 80).rows[0]);
        assert!(text.contains("[FAST:OFF] · [AGENTS:2]"), "{text}");

        for task in &mut app.sdk_inventory.tasks {
            task.status = model::TaskStatus::Completed;
        }
        let text = line_text(&serialize_footer_rows(&app, 80).rows[0]);
        assert!(!text.contains("AGENTS"), "{text}");
    }

    #[test]
    fn reported_session_effort_updates_the_independent_effort_field() {
        let mut app = app_with_mode();
        app.session_runtime.session_id = Some("sess-1".into());
        app.session_runtime.current_model =
            Some(model::CurrentModel::new("opus", "Opus", "Opus").supports_effort(true));
        for (value, label) in [
            ("low", "Low"),
            ("medium", "Med"),
            ("high", "High"),
            ("xhigh", "XHigh"),
            ("max", "Max"),
        ] {
            crate::app::handle_client_event(
                &mut app,
                crate::agent::events::ClientEvent::SessionUpdate {
                    session_id: "sess-1".to_owned(),
                    update: model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                        option_id: "effortLevel".to_owned(),
                        value: serde_json::json!(value),
                    }),
                },
            );
            let footer = serialize_footer_rows(&app, 120);
            assert!(line_text(&footer.rows[0]).contains(&format!("[{label}]")));
            assert!(line_text(&footer.rows[0]).contains("[Opus]"));
        }
        crate::app::handle_client_event(
            &mut app,
            crate::agent::events::ClientEvent::SessionUpdate {
                session_id: "sess-1".to_owned(),
                update: model::SessionUpdate::ConfigOptionUpdate(model::ConfigOptionUpdate {
                    option_id: "effortLevel".to_owned(),
                    value: serde_json::Value::Null,
                }),
            },
        );
        assert!(line_text(&serialize_footer_rows(&app, 120).rows[0]).contains("[Opus]"));
    }

    #[test]
    fn ultracode_badge_appears_only_for_verified_effective_state() {
        for state in [
            None,
            model::UltracodeState::new(true, false, false),
            model::UltracodeState::new(false, true, false),
            model::UltracodeState::new(false, false, false),
            model::UltracodeState::new(true, true, true),
        ] {
            let mut app = app_with_mode();
            app.session_runtime.ultracode = state;
            let footer = serialize_footer_rows(&app, 120);
            assert_eq!(
                line_text(&footer.rows[0]).contains("Ultracode"),
                state.is_some_and(model::UltracodeState::effective)
            );
            if state.is_some_and(model::UltracodeState::effective) {
                let label = footer.rows[0]
                    .spans
                    .iter()
                    .find(|span| span.content == "[Ultracode]")
                    .expect("Ultracode label");
                assert_eq!(label.style.fg, Some(theme::ULTRACODE_ACCENT));
                assert!(
                    footer.rows[0].spans.iter().any(|span| span.content.contains("test-model")
                        && span.style.fg == Some(FOOTER_CONTEXT_VALUE))
                );
            }
        }
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
    fn footer_preserves_directory_when_space_allows() {
        let app = App::test_default();
        let serialized = serialize_footer_rows(&app, 160);
        let text = line_text(&serialized.rows[0]);

        assert!(text.contains("Loc:"));
    }

    #[test]
    fn pending_permission_hint_wins_on_first_row() {
        let mut app = App::test_default();
        let (response_tx, _response_rx) = oneshot::channel();
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(ToolCallInfo {
                id: "perm-1".into(),
                source_message_uuids: Vec::new(),
                title: "Read".into(),
                sdk_tool_name: "Read".into(),
                raw_input: None,
                raw_input_bytes: 0,
                locations: Vec::new(),
                output_metadata: None,
                task_metadata: None,
                status: model::ToolCallStatus::Pending,
                content: vec![],
                hidden: false,
                terminal_id: None,
                terminal_command: None,
                terminal_output: None,
                terminal_output_len: 0,
                cache: BlockCache::default(),
                pending_permission: Some(InlinePermission {
                    options: vec![],
                    display: None,
                    subagent_context: None,
                    response_tx,
                    selected_index: 0,
                    focused: true,
                }),
                pending_question: None,
                history: crate::app::ToolCallHistory::Live,
            }))],
            None,
        ));
        app.index_tool_call("perm-1".into(), 0, 0);
        app.pending_interaction_ids.push("perm-1".into());

        let serialized = serialize_footer_rows(&app, 80);
        let text = line_text(&serialized.rows[0]);
        assert!(text.ends_with("1 PEND. PERM."));
        let directory = tempfile::tempdir().expect("preferences");
        let path = directory.path().join("settings.json");
        std::fs::write(&path, r#"{"presentation":{"footer":[]}}"#).expect("save layout");
        app.config.load_footer(Some(&path));
        let text = line_text(&serialize_footer_rows(&app, 80).rows[0]);
        assert!(text.ends_with("1 PEND. PERM."));
        assert!(!text.contains("FAST:"));
        assert!(!text.contains("Loc:"));
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
    fn narrow_width_preserves_notice_before_configured_fields() {
        let mut app = App::test_default();
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

        let serialized = serialize_footer_rows(&app, 36);
        let text = line_text(&serialized.rows[0]);
        assert!(text.ends_with("1 MCP NEEDS AUTH"));
        assert!(text.width() <= 36);
        let layout = FooterLayout::from_saved(Some(&serde_json::json!([
            {"id":"fastMode","visible":true,"side":"left"}
        ])))
        .expect("layout");
        let exact_fit = line_text(&serialize_footer(&app, &layout, 27));
        assert!(exact_fit.starts_with("[FAST:OFF]"));
        assert!(exact_fit.ends_with("1 MCP NEEDS AUTH"));
        assert_eq!(exact_fit.width(), 27);
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

    #[test]
    fn configured_groups_preserve_order_and_anchor_to_opposite_edges() {
        let mut app = app_with_mode();
        app.set_git_branch_for_test(Some("main"));
        app.session_runtime.session_usage.context_usage_percent = Some(28);
        let layout = FooterLayout::from_saved(Some(&serde_json::json!([
            {"id":"fastMode","visible":true,"side":"left"},
            {"id":"permissionMode","visible":true,"side":"left"},
            {"id":"contextRemaining","visible":true,"side":"right"},
            {"id":"gitBranch","visible":true,"side":"right"}
        ])))
        .expect("layout");
        let text = line_text(&serialize_footer(&app, &layout, 100));
        assert!(text.starts_with("[FAST:OFF] · [default]"));
        assert!(text.ends_with("Context: 72% · Branch: main"));
        assert_eq!(text.width(), 100);
    }

    #[test]
    fn unicode_values_never_overlap_or_exceed_the_single_row() {
        let mut app = app_with_mode();
        app.cwd = "~/工作/项目/claude_rust".into();
        app.set_git_branch_for_test(Some("feature/日本語の長いブランチ"));
        let layout = FooterLayout::default();
        for width in 0..=160 {
            let line = serialize_footer(&app, &layout, width);
            assert!(line.width() <= usize::from(width), "width {width}: {}", line_text(&line));
        }
    }

    #[test]
    fn shortened_branch_retains_part_of_its_name_or_is_omitted() {
        let mut app = app_with_mode();
        app.set_git_branch_for_test(Some("feature/configurable-footer"));
        let layout = FooterLayout::from_saved(Some(&serde_json::json!([
            {"id":"gitBranch","visible":true,"side":"left"}
        ])))
        .expect("layout");
        assert!(line_text(&serialize_footer(&app, &layout, 11)).is_empty());
        let shortened = line_text(&serialize_footer(&app, &layout, 12));
        assert!(shortened.contains("f..."));
        assert_eq!(shortened.width(), 12);
    }
    #[test]
    fn each_informational_field_renders_its_own_color_instead_of_a_global_override() {
        let mut app = app_with_mode();
        app.cwd = "~/work".into();
        app.set_git_branch_for_test(Some("feature/footer"));
        app.session_runtime.session_usage.context_usage_percent = Some(42);
        let layout = FooterLayout::from_saved(Some(&serde_json::json!([
            {"id":"model","visible":true,"side":"left","color":"cyan"},
            {"id":"directory","visible":true,"side":"left","color":"orange"},
            {"id":"gitBranch","visible":true,"side":"right","color":"pink"},
            {"id":"contextRemaining","visible":true,"side":"right","color":"green"}
        ])))
        .expect("layout");
        let line = serialize_footer(&app, &layout, 160);
        for (text, color) in [
            ("test-model", Color::Rgb(34, 211, 238)),
            ("Loc: ~/work", Color::Rgb(251, 146, 60)),
            ("Branch: feature/footer", Color::Rgb(244, 114, 182)),
            ("Context: 58%", Color::Rgb(74, 222, 128)),
        ] {
            let span = line
                .spans
                .iter()
                .find(|span| span.content.contains(text))
                .expect("associated field");
            assert_eq!(span.style.fg, Some(color), "{text}");
        }
    }
}
