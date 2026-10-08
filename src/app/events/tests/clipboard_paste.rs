// SPDX-License-Identifier: Apache-2.0
//! Ctrl+V terminal event -> clipboard read -> image badge and attachment -> prompt command.

use super::{app_with_bridge_connection, make_test_app};
use crate::agent::wire::BridgeCommand;
use crate::app::clipboard_image::{ClipboardImagePaste, ClipboardRead, ImageAttachment};
use crate::app::events::{TerminalEventOutcome, handle_terminal_event};
use crate::app::{
    App, AppStatus, FullscreenView, MessageRole, SurfaceMode, SystemSeverity, input_submit,
};
use base64::Engine as _;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pretty_assertions::assert_eq;

const RED: [u8; 4] = [255, 0, 0, 255];
const HALF_BLUE: [u8; 4] = [0, 0, 255, 128];

fn two_pixel_image() -> ClipboardRead {
    ClipboardRead::Image(arboard::ImageData {
        width: 2,
        height: 1,
        bytes: [RED, HALF_BLUE].concat().into(),
    })
}

fn app_with_clipboard(trigger: KeyEventKind, read: fn() -> ClipboardRead) -> App {
    let mut app = make_test_app();
    app.clipboard_paste = ClipboardImagePaste { trigger, read };
    app
}

fn ctrl_v(kind: KeyEventKind) -> Event {
    Event::Key(KeyEvent::new_with_kind(KeyCode::Char('v'), KeyModifiers::CONTROL, kind))
}

/// Decodes an attachment the way a receiver would: base64, then PNG.
fn decoded_rgba(data: &str) -> (u32, u32, Vec<u8>) {
    let png = base64::engine::general_purpose::STANDARD.decode(data).expect("base64 payload");
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .expect("PNG payload")
        .into_rgba8();
    (image.width(), image.height(), image.into_raw())
}

fn assert_two_pixel_attachment(attachment: &ImageAttachment) {
    assert_eq!(attachment.mime_type, "image/png");
    assert_eq!(decoded_rgba(&attachment.data), (2, 1, [RED, HALF_BLUE].concat()));
}

fn has_warning(app: &App) -> bool {
    app.transcript
        .messages
        .iter()
        .any(|message| matches!(message.role, MessageRole::System(Some(SystemSeverity::Warning))))
}

/// Terminals outside Windows report Ctrl+V as a press and never send a release
/// (issues #385 and #431). Both encodings of the shortcut must attach.
#[test]
fn ctrl_v_press_attaches_the_clipboard_image_where_only_presses_arrive() {
    for (code, modifiers) in
        [(KeyCode::Char('v'), KeyModifiers::CONTROL), (KeyCode::Char('\u{16}'), KeyModifiers::NONE)]
    {
        let mut app = app_with_clipboard(KeyEventKind::Press, two_pixel_image);

        handle_terminal_event(
            &mut app,
            Event::Key(KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Press)),
        );

        assert_eq!(app.input.text(), "[Image #1]", "{code:?}");
        assert_eq!(app.pending_images.len(), 1, "{code:?}");
        assert_two_pixel_attachment(&app.pending_images[0]);
        assert!(!has_warning(&app), "{code:?}");
    }
}

#[test]
fn a_release_after_the_attaching_press_does_not_attach_again() {
    let mut app = app_with_clipboard(KeyEventKind::Press, two_pixel_image);

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    let release = handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Release));

    assert_eq!(release, TerminalEventOutcome::ignored());
    assert_eq!(app.input.text(), "[Image #1]");
    assert_eq!(app.pending_images.len(), 1);
}

/// Windows consoles report both edges of the key. Exactly one of them attaches.
#[test]
fn ctrl_v_attaches_once_where_press_and_release_both_arrive() {
    let mut app = app_with_clipboard(KeyEventKind::Release, two_pixel_image);

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Release));

    assert_eq!(app.input.text(), "[Image #1]");
    assert_eq!(app.pending_images.len(), 1);
    assert_two_pixel_attachment(&app.pending_images[0]);
}

/// A Windows terminal that keeps the press for its own paste forwards only the release.
#[test]
fn ctrl_v_release_alone_attaches_where_the_terminal_keeps_the_press() {
    let mut app = app_with_clipboard(KeyEventKind::Release, two_pixel_image);

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Release));

    assert_eq!(app.input.text(), "[Image #1]");
    assert_eq!(app.pending_images.len(), 1);
}

#[test]
fn each_clipboard_image_gets_the_next_badge_at_the_cursor() {
    let mut app = app_with_clipboard(KeyEventKind::Press, two_pixel_image);
    app.input.set_text("ab");
    let _ = app.input.set_cursor_col(1);

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));

    assert_eq!(app.input.text(), "a[Image #1][Image #2]b");
    assert_eq!(app.pending_images.len(), 2);
}

#[test]
fn a_pasted_clipboard_image_is_sent_with_the_next_prompt() {
    let (mut app, mut commands) = app_with_bridge_connection();
    app.clipboard_paste =
        ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
    app.status = AppStatus::Ready;
    app.input.set_text("what is this ");

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    input_submit::submit_input(&mut app);

    let BridgeCommand::Prompt { chunks, .. } =
        commands.try_recv().expect("prompt admitted").command
    else {
        panic!("expected prompt");
    };
    let images: Vec<_> = chunks.iter().filter(|chunk| chunk.kind == "image").collect();
    assert_eq!(images.len(), 1, "the prompt must carry the pasted image");
    assert_eq!(images[0].value["mime_type"], "image/png");
    assert_eq!(
        decoded_rgba(images[0].value["data"].as_str().expect("image data")),
        (2, 1, [RED, HALF_BLUE].concat())
    );
    let text: Vec<_> = chunks.iter().filter(|chunk| chunk.kind == "text").collect();
    assert_eq!(text.len(), 1);
    assert_eq!(text[0].value.as_str(), Some("what is this [Image #1]"));
    assert!(app.pending_images.is_empty());
    assert!(app.input.is_empty());
}

#[test]
fn ctrl_v_without_a_clipboard_image_leaves_the_draft_and_transcript_alone() {
    let mut app = app_with_clipboard(KeyEventKind::Press, || ClipboardRead::NoImage);
    app.input.set_text("draft");

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));

    assert_eq!(app.input.text(), "draft");
    assert!(app.pending_images.is_empty());
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn ctrl_v_reports_an_unreachable_clipboard_instead_of_failing_silently() {
    let mut app = app_with_clipboard(KeyEventKind::Press, || ClipboardRead::Unavailable);
    app.input.set_text("draft");

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));

    assert!(has_warning(&app));
    assert_eq!(app.input.text(), "draft");
    assert!(app.pending_images.is_empty());
}

#[test]
fn ctrl_v_rejects_a_corrupt_clipboard_image_with_a_warning_and_no_badge() {
    let mut app = app_with_clipboard(KeyEventKind::Press, || {
        // Three bytes cannot hold the 2x1 RGBA image the clipboard announces.
        ClipboardRead::Image(arboard::ImageData { width: 2, height: 1, bytes: vec![0; 3].into() })
    });

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));

    assert!(has_warning(&app));
    assert!(app.input.is_empty());
    assert!(app.pending_images.is_empty());
}

#[test]
fn ctrl_v_outside_the_chat_composer_attaches_nothing() {
    let mut app = app_with_clipboard(KeyEventKind::Press, two_pixel_image);
    app.surface_mode = SurfaceMode::Fullscreen(FullscreenView::Config);

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));

    assert!(app.input.is_empty());
    assert!(app.pending_images.is_empty());
}
