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

    #[test]
    fn picker_copies_canonical_code_and_retains_choice_on_clipboard_failure() {
        let mut app = App::test_default();
        response(&mut app, "Explanation\n\n```rust\nlet x = 1;\n```\n");
        open_with(&mut app, |_| panic!("picker must wait for a choice"));
        assert_eq!(
            app.surface_mode,
            crate::app::SurfaceMode::Fullscreen(crate::app::FullscreenView::Copy)
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(38, 12)).expect("terminal");
        terminal
            .draw(|frame| crate::ui::render_fullscreen_surface(frame, &mut app))
            .expect("render picker");
        handle_key_with(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), |_| Ok(()));
        handle_key_with(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), |_| {
            Err("Clipboard unavailable".into())
        });
        assert_eq!(app.copy_picker.as_ref().expect("retain picker").selected, 1);
        assert_eq!(
            app.copy_picker.as_ref().expect("error").error.as_deref(),
            Some("Clipboard unavailable")
        );
        let mut copied = String::new();
        handle_key_with(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), |text| {
            copied = text.into();
            Ok(())
        });
        assert_eq!(copied, "let x = 1;\n");
        assert_eq!(app.surface_mode, crate::app::SurfaceMode::Chat);
        assert!(app.copy_picker.is_none());
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
