// SPDX-License-Identifier: Apache-2.0

use super::common::{detail_kv, section_heading};
use crate::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

pub(super) fn render(frame: &mut Frame, area: Rect, app: &App) {
    let lines = status_lines(app);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
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
    fn status_lines_contains_version() {
        let app = App::test_default();
        let text = lines_to_string(&status_lines(&app));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn memory_row_finds_claude_codes_project_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        std::fs::create_dir(&project).expect("project");
        let mut app = App::test_default();
        app.settings_home_override = Some(dir.path().join("home"));
        app.cwd_raw = project.to_string_lossy().into_owned();
        // Claude Code's folder: the resolved path with every non-alphanumeric
        // character as `-`, leading separator included (the fixture is ASCII).
        let folder: String = std::fs::canonicalize(&project)
            .expect("resolved project")
            .to_string_lossy()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        let memory = dir.path().join("home/.claude/projects").join(folder).join("memory/MEMORY.md");
        std::fs::create_dir_all(memory.parent().expect("memory folder")).expect("memory folder");
        std::fs::write(&memory, "notes").expect("memory file");

        crate::app::config::open_tab(&mut app, crate::app::config::ConfigTab::Status)
            .expect("status tab");

        let row = status_lines(&app)
            .into_iter()
            .map(|line| line.to_string())
            .find(|line| line.starts_with("Memory:"))
            .expect("memory row");
        assert_eq!(row, format!("Memory: auto memory ({})", memory.display()));
    }

    #[test]
    fn status_lines_shows_cwd() {
        let mut app = App::test_default();
        app.cwd = "/test/project".to_owned();
        let text = lines_to_string(&status_lines(&app));
        assert!(text.contains("/test/project"));
    }

    #[test]
    fn status_lines_shows_model() {
        let mut app = App::test_default();
        app.session_runtime.current_model = Some(
            crate::agent::model::CurrentModel::new("claude-sonnet-4-7", "Sonnet", "Sonnet 4.7")
                .authoritative(true),
        );
        let text = lines_to_string(&status_lines(&app));
        assert!(text.contains("Sonnet 4.7"));
    }

    #[test]
    fn status_lines_unnamed_session_fallback() {
        let app = App::test_default();
        let text = lines_to_string(&status_lines(&app));
        assert!(text.contains("(unnamed session)"));
    }

    #[test]
    fn status_lines_name_the_session_by_its_live_title() {
        let mut app = App::test_default();
        app.session_runtime.session_id = Some(crate::agent::model::SessionId::new("test-sess-1"));
        app.recent_sessions = vec![crate::app::RecentSessionInfo {
            session_id: "test-sess-1".to_owned(),
            summary: "Listed summary".to_owned(),
            last_modified_ms: 0,
            file_size_bytes: 0,
            cwd: None,
            git_branch: None,
            custom_title: Some("Stale listed title".to_owned()),
            first_prompt: None,
        }];
        let name_row = |app: &App| {
            status_lines(app)
                .iter()
                .map(std::string::ToString::to_string)
                .find(|line| line.contains("Session name"))
                .expect("session name row")
        };
        assert!(name_row(&app).contains("Stale listed title"));
        app.session_runtime.session_title = Some("probe-e2e".to_owned());
        assert!(name_row(&app).contains("probe-e2e"));
    }

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

    #[test]
    fn section_headers_present() {
        let app = App::test_default();
        let text = lines_to_string(&status_lines(&app));
        assert!(text.contains("Session"));
        assert!(text.contains("Model"));
        assert!(text.contains("Settings"));
    }

    #[test]
    fn login_method_maps_oauth() {
        let account = crate::agent::model::AccountInfo {
            api_key_source: Some("oauth".to_owned()),
            ..Default::default()
        };
        assert_eq!(login_method_label(&account), "Claude Max Account");
    }

    #[test]
    fn login_method_maps_user_key() {
        let account = crate::agent::model::AccountInfo {
            api_key_source: Some("user".to_owned()),
            ..Default::default()
        };
        assert_eq!(login_method_label(&account), "User API key");
    }

    #[test]
    fn login_method_maps_external_provider() {
        let account = crate::agent::model::AccountInfo {
            api_provider: Some(crate::agent::model::AccountApiProvider::Bedrock),
            ..Default::default()
        };
        assert_eq!(login_method_label(&account), "External provider");
    }

    #[test]
    fn status_lines_render_api_provider() {
        let mut app = App::test_default();
        app.session_runtime.account_info = Some(crate::agent::model::AccountInfo {
            api_provider: Some(crate::agent::model::AccountApiProvider::Gateway),
            ..Default::default()
        });

        let text = lines_to_string(&status_lines(&app));

        assert!(text.contains("API provider"));
        assert!(text.contains("Gateway"));
    }

    #[test]
    fn login_method_falls_back_to_unknown() {
        let account = crate::agent::model::AccountInfo::default();
        assert_eq!(login_method_label(&account), "Unknown");
    }

    fn lines_to_string(lines: &[Line<'_>]) -> String {
        lines.iter().map(std::string::ToString::to_string).collect::<Vec<_>>().join("\n")
    }
}
