// SPDX-License-Identifier: Apache-2.0

use super::common::{detail_kv, section_heading};
use crate::app::App;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget, Wrap};

pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.is_empty() {
        return;
    }
    let lines = status_lines(app);
    let height = super::common::wrapped_height(lines.clone(), area.width);
    let mut content = Buffer::empty(Rect::new(0, 0, area.width, height));
    Paragraph::new(lines).wrap(Wrap { trim: false }).render(content.area, &mut content);
    super::viewport::present(frame, area, &content, &mut app.config);
}

pub(crate) fn status_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // ---- Session ----
    lines.push(section_heading("Session"));
    lines.push(detail_kv("Version", env!("CARGO_PKG_VERSION"), Color::White));
    lines.push(detail_kv("Session name", &derive_session_name(app), Color::White));

    let session_id_str = app
        .session_runtime
        .session_id
        .as_ref()
        .map_or_else(|| "(none)".to_owned(), std::string::ToString::to_string);
    lines.push(detail_kv("Session ID", &session_id_str, Color::White));

    lines.push(detail_kv("cwd", &app.cwd, Color::White));

    if let Some(branch) = app.git_branch() {
        lines.push(detail_kv("Git branch", branch, Color::White));
    }

    lines.push(Line::default());

    // ---- Account ----
    if let Some(ref account) = app.session_runtime.account_info {
        lines.push(section_heading("Account"));
        lines.push(detail_kv("Login method", &login_method_label(account), Color::White));
        if let Some(ref provider) = account.api_provider {
            lines.push(detail_kv("API provider", provider.label(), Color::White));
        }
        if let Some(ref org) = account.organization
            && !org.is_empty()
        {
            lines.push(detail_kv("Organization", org, Color::White));
        }
        if let Some(ref email) = account.email
            && !email.is_empty()
        {
            lines.push(detail_kv("Email", email, Color::White));
        }
        if let Some(ref sub) = account.subscription_type
            && !sub.is_empty()
        {
            lines.push(detail_kv("Subscription", sub, Color::White));
        }
        lines.push(Line::default());
    }

    // ---- Model ----
    lines.push(section_heading("Model"));
    lines.push(detail_kv("Model", &model_display(app), Color::White));
    if let Some(current_model) = app.session_runtime.current_model.as_ref() {
        lines.push(detail_kv("Resolved model ID", &current_model.resolved_id, Color::White));
        if let Some(requested_id) = current_model.requested_id.as_deref()
            && requested_id != current_model.resolved_id
        {
            lines.push(detail_kv("Requested model", requested_id, Color::White));
        }
    }

    if let Some(ref mode) = app.session_runtime.mode {
        lines.push(detail_kv("Mode", &mode.current_mode_name, Color::White));
    }

    lines.push(Line::default());

    // ---- Settings ----
    lines.push(section_heading("Settings"));

    let memory_path = resolve_memory_path(app);
    lines.push(detail_kv("Memory", &memory_path, Color::White));

    let sources = setting_sources(app);
    lines.push(detail_kv("Setting sources", &sources, Color::White));

    lines
}

/// The session's title, never its summary or first prompt: after `/clear`
/// both of those are the `/clear` command itself.
fn derive_session_name(app: &App) -> String {
    app.active_session_custom_title().unwrap_or("(unnamed session)").to_owned()
}

fn model_display(app: &App) -> String {
    let Some(current_model) = app.session_runtime.current_model.as_ref() else {
        return "(not set)".to_owned();
    };
    current_model.display_name_long.clone()
}

pub(crate) fn login_method_label(account: &crate::agent::model::AccountInfo) -> String {
    account.login_method_label()
}

fn resolve_memory_path(app: &App) -> String {
    let Some(memory_md) = app.config.memory_file.as_ref() else {
        return "(unable to resolve Claude configuration directory)".to_owned();
    };

    if memory_md.exists() {
        format!("auto memory ({})", memory_md.display())
    } else {
        "(no memory file found)".to_owned()
    }
}

fn setting_sources(app: &App) -> String {
    app.config.snapshot.as_ref().map_or_else(
        || "(not inspected)".to_owned(),
        |snapshot| {
            if snapshot.sources.is_empty() {
                return "(no sources reported)".to_owned();
            }
            snapshot
                .sources
                .iter()
                .map(|source| format!("{} ({})", source.scope.label(), source.status))
                .collect::<Vec<_>>()
                .join(", ")
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_name_shows_only_a_title_of_the_active_session() {
        let mut app = App::test_default();
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("active"));
        let unicode = "\u{1f980}".repeat(61);
        // What the listing reports for the session `/clear` starts: no title,
        // and the command itself as summary and first prompt.
        app.recent_sessions = vec![crate::app::RecentSessionInfo {
            session_id: "active".to_owned(),
            summary: "/clear".to_owned(),
            last_modified_ms: 0,
            file_size_bytes: 0,
            cwd: None,
            git_branch: None,
            custom_title: None,
            first_prompt: Some("/clear".to_owned()),
        }];
        let name_row = |app: &App| {
            status_lines(app)
                .into_iter()
                .map(|line| line.to_string())
                .find(|line| line.starts_with("Session name:"))
                .expect("name row")
        };
        assert_eq!(name_row(&app), "Session name: (unnamed session)");
        app.recent_sessions[0].custom_title = Some(unicode.clone());
        assert_eq!(name_row(&app), format!("Session name: {unicode}"));
        app.recent_sessions[0].session_id = "other".to_owned();
        assert_eq!(name_row(&app), "Session name: (unnamed session)");
    }
}
