// SPDX-License-Identifier: Apache-2.0

//! Copy material is derived from the last finished canonical response, never terminal cells.

use crate::app::{App, MessageBlock, MessageRole, SystemSeverity};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

pub(crate) struct CopyChoice {
    pub label: String,
    pub text: String,
}

pub(crate) struct CopyPicker {
    pub choices: Vec<CopyChoice>,
    pub selected: usize,
    pub error: Option<String>,
}

pub(crate) fn choices(app: &App) -> Vec<CopyChoice> {
    let Some(message) = app.transcript.messages.iter().rev().find(|message| {
        matches!(message.role, MessageRole::Assistant)
            && message.timing.is_finished()
            && message.blocks.iter().any(
                |block| matches!(block, MessageBlock::Text(text) if !text.text.trim().is_empty()),
            )
    }) else {
        return Vec::new();
    };
    let mut response = String::new();
    let mut spacing = crate::app::TextBlockSpacing::None;
    for block in &message.blocks {
        if let MessageBlock::Text(text) = block {
            spacing.append_source(&mut response, &text.text);
            spacing = text.trailing_spacing;
        }
    }
    let mut choices = vec![CopyChoice { label: "Full response".into(), text: response.clone() }];
    let mut code: Option<CopyChoice> = None;
    for event in Parser::new(&response) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(language) => language.into_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let label = format!(
                    "Code block {}{}",
                    choices.len(),
                    if language.is_empty() { String::new() } else { format!(" ({language})") }
                );
                code = Some(CopyChoice { label, text: String::new() });
            }
            Event::Text(text) => {
                if let Some(code) = &mut code {
                    code.text.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(code) = code.take() {
                    choices.push(code);
                }
            }
            _ => {}
        }
    }
    choices
}

pub(crate) fn open(app: &mut App) {
    open_with(app, crate::app::clipboard::copy_text);
}

fn open_with(app: &mut App, clipboard: impl FnOnce(&str) -> Result<(), String>) {
    let choices = choices(app);
    if choices.is_empty() {
        crate::app::events::push_submission_feedback(
            app,
            SystemSeverity::Info,
            "No completed response to copy yet.",
        );
        return;
    }
    if app.config.copy_full_response_effective() || choices.len() == 1 {
        report_copy(app, clipboard(&choices[0].text));
    } else {
        app.copy_picker = Some(CopyPicker { choices, selected: 0, error: None });
        crate::app::view::set_fullscreen_view(app, crate::app::FullscreenView::Copy);
    }
}

fn report_copy(app: &mut App, result: Result<(), String>) {
    let (severity, text) = match result {
        Ok(()) => (SystemSeverity::Info, "Copied to clipboard.".to_owned()),
        Err(error) => (SystemSeverity::Error, error),
    };
    crate::app::events::push_submission_feedback(app, severity, &text);
}

pub(crate) fn handle_key(app: &mut App, key: KeyEvent) {
    handle_key_with(app, key, crate::app::clipboard::copy_text);
}

fn handle_key_with(
    app: &mut App,
    key: KeyEvent,
    clipboard: impl FnOnce(&str) -> Result<(), String>,
) {
    let Some(picker) = &mut app.copy_picker else { return };
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            app.copy_picker = None;
            crate::app::view::set_chat_surface(app);
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            picker.selected = picker.selected.saturating_sub(1);
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            picker.selected = (picker.selected + 1).min(picker.choices.len().saturating_sub(1));
        }
        (KeyCode::Enter, KeyModifiers::NONE) => {
            match clipboard(&picker.choices[picker.selected].text) {
                Ok(()) => {
                    app.copy_picker = None;
                    crate::app::view::set_chat_surface(app);
                    report_copy(app, Ok(()));
                }
                Err(error) => picker.error = Some(error),
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ChatMessage, TextBlock, TextBlockSpacing};

    fn response(app: &mut App, text: &str) {
        let mut message = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete(text))],
            None,
        );
        message.timing.finish();
        app.transcript.messages.push(message);
    }

    fn picker_rows(
        app: &mut App,
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
    ) -> Vec<String> {
        terminal.draw(|frame| crate::ui::render_fullscreen_surface(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(70)
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect())
            .collect()
    }

    fn choice_style(
        terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
        label: &str,
    ) -> ratatui::style::Style {
        let buffer = terminal.backend().buffer();
        for y in 0..buffer.area.height {
            let row = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>();
            if let Some(byte) = row.find(label) {
                let column = unicode_width::UnicodeWidthStr::width(&row[..byte]);
                return buffer[(u16::try_from(column).unwrap(), y)].style();
            }
        }
        panic!("missing copy choice {label}");
    }

    #[test]
    fn copy_picker_shows_the_selected_payload_and_retry_then_discards_cancelled_selection() {
        let source = "Cedar explanation\n\n```rust\nfn cedar() {}\n```\n\nElm explanation\n\n```text\n  雪 e\u{301}clair\nend\n```\n";
        let expected = "  雪 e\u{301}clair\nend\n";
        let mut app = App::test_default();
        app.input.set_text("unsent 界");
        response(&mut app, source);
        open_with(&mut app, |_| panic!("copy must wait for explicit activation"));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(70, 16)).unwrap();
        picker_rows(&mut app, &mut terminal);
        let normal = choice_style(&terminal, "Code block 1 (rust)");
        for _ in 0..2 {
            handle_key_with(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), |_| {
                panic!("navigation must not copy")
            });
        }
        picker_rows(&mut app, &mut terminal);
        let selected = choice_style(&terminal, "Code block 2 (text)");
        assert_ne!(selected, normal, "selected choice must have visible emphasis");
        assert_eq!(choice_style(&terminal, "Code block 1 (rust)"), normal);
        let mut attempts = Vec::new();
        handle_key_with(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), |text| {
            attempts.push(text.to_owned());
            assert_eq!(text, expected);
            Err("COPY_DENIED_492".into())
        });
        let rows = picker_rows(&mut app, &mut terminal);
        assert_eq!(choice_style(&terminal, "Code block 2 (text)"), selected);
        assert_eq!(choice_style(&terminal, "Code block 1 (rust)"), normal);
        let choice = rows.iter().position(|row| row.contains("Code block 2 (text)")).unwrap();
        let error = rows.iter().position(|row| row.contains("COPY_DENIED_492")).unwrap();
        let controls = rows
            .iter()
            .position(|row| row.contains("Enter retry") && row.contains("Esc cancel"))
            .unwrap();
        assert!(choice < error && error < controls);
        handle_key_with(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), |text| {
            attempts.push(text.to_owned());
            assert_eq!(text, expected);
            Ok(())
        });
        assert_eq!(attempts, [expected, expected]);
        assert!(app.copy_picker.is_none());
        assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
        assert_eq!(app.input.text(), "unsent 界");
        crate::app::handle_terminal_event(&mut app, crossterm::event::Event::Paste("!".into()));
        crate::app::finalize_pending_paste_event(&mut app);
        assert_eq!(app.input.text(), "unsent 界!");

        open_with(&mut app, |_| panic!("picker must wait"));
        handle_key_with(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), |_| {
            panic!("navigation must not copy")
        });
        crate::app::handle_terminal_event(
            &mut app,
            crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        );
        assert!(app.copy_picker.is_none());
        assert!(!app.shutdown_requested());
        assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
        assert_eq!(app.input.text(), "unsent 界!");
        open_with(&mut app, |_| panic!("reopening must not copy"));
        picker_rows(&mut app, &mut terminal);
        assert_ne!(
            choice_style(&terminal, "Full response"),
            choice_style(&terminal, "Code block 2 (text)")
        );
        assert_eq!(choice_style(&terminal, "Code block 2 (text)"), normal);
        handle_key_with(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), |_| {
            panic!("cancel must not copy")
        });
        assert_eq!(app.input.text(), "unsent 界!");
    }

    #[test]
    fn direct_copy_waits_for_completion_and_reports_failure_before_successful_retry() {
        use crate::agent::{events::ClientEvent, model};
        use crate::app::test_support::{app_with_connection, deliver_update, source_chunk};
        let (mut app, _commands) = app_with_connection();
        app.status = crate::app::AppStatus::Ready;
        app.input.set_text("unsent 雪 draft");
        for role in [MessageRole::User, MessageRole::System(Some(SystemSeverity::Info))] {
            app.push_message_tracked(ChatMessage::new(
                role,
                vec![MessageBlock::Text(TextBlock::from_complete("NOT_COPYABLE"))],
                None,
            ));
        }
        let mut tool_only = ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::ToolCall(Box::new(crate::app::test_support::tool_call_info(
                "tool-only",
                model::ToolCallStatus::Completed,
            )))],
            None,
        );
        tool_only.timing.finish();
        app.push_message_tracked(tool_only);
        deliver_update(&mut app, model::SessionUpdate::AgentResponseStarted);
        let expected = "Completed cedar response: 雪 e\u{301}clair\nsecond line";
        deliver_update(&mut app, source_chunk(expected, "copy-source", false));
        open_with(&mut app, |_| panic!("no completed response may reach the clipboard"));
        assert!(app.copy_picker.is_none());
        assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
        assert!(app.transcript.messages.iter().flat_map(|message| &message.blocks).any(|block| matches!(block, MessageBlock::Notice(notice) if notice.text.text.contains("No completed response"))));
        assert_eq!(app.input.text(), "unsent 雪 draft");
        crate::app::handle_client_event(
            &mut app,
            ClientEvent::TurnComplete {
                session_id: "session-1".into(),
                queued_turn_count: None,
                terminal_reason: None,
            },
        );
        let mut attempts = Vec::new();
        open_with(&mut app, |text| {
            attempts.push(text.to_owned());
            assert_eq!(text, expected);
            Err("DIRECT_COPY_DENIED_725".into())
        });
        let failure = app.transcript.messages.last().unwrap();
        assert!(matches!(failure.role, MessageRole::System(Some(SystemSeverity::Error))));
        assert!(
            matches!(&failure.blocks[..], [MessageBlock::Text(text)] if text.text.contains("DIRECT_COPY_DENIED_725") && !text.text.contains("Copied to clipboard"))
        );
        assert!(app.copy_picker.is_none());
        assert_eq!(app.input.text(), "unsent 雪 draft");
        assert_eq!(choices(&app)[0].text, expected);
        open_with(&mut app, |text| {
            attempts.push(text.to_owned());
            assert_eq!(text, expected);
            Ok(())
        });
        assert_eq!(attempts, [expected, expected]);
        let success = app.transcript.messages.last().unwrap();
        assert!(matches!(success.role, MessageRole::System(Some(SystemSeverity::Info))));
        assert!(
            matches!(&success.blocks[..], [MessageBlock::Text(text)] if text.text.contains("Copied") && !text.text.contains("DIRECT_COPY_DENIED_725"))
        );
        assert!(app.copy_picker.is_none());
        assert_eq!(app.input.text(), "unsent 雪 draft");
        assert_eq!(choices(&app)[0].text, expected);
    }

    #[test]
    fn direct_copy_joins_split_markdown_and_ignores_an_unfinished_new_response() {
        let mut app = App::test_default();
        response(&mut app, "First paragraph");
        app.transcript.messages[0].blocks = vec![
            MessageBlock::Text(
                TextBlock::from_complete("First paragraph")
                    .with_trailing_spacing(TextBlockSpacing::ParagraphBreak),
            ),
            MessageBlock::Text(TextBlock::from_complete("```text\nfull response\n```")),
        ];
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::Assistant,
            vec![MessageBlock::Text(TextBlock::from_complete("unfinished"))],
            None,
        ));
        app.config.snapshot = Some(crate::agent::settings::SettingsSnapshot::test_value(
            "presentation.copyFullResponse",
            serde_json::json!(true),
        ));
        let mut copied = String::new();
        open_with(&mut app, |text| {
            copied = text.into();
            Ok(())
        });
        assert_eq!(copied, "First paragraph\n\n```text\nfull response\n```");
        assert!(app.copy_picker.is_none());
        // Resumed responses can be copied even when the SDK history omits duration.
        app.transcript.messages[0].timing.duration = None;
        app.transcript.messages[0].timing.discard_replay_observations();
        assert_eq!(choices(&app)[0].text, copied);
    }
}
