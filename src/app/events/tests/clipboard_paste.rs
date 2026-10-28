// SPDX-License-Identifier: Apache-2.0
//! Ctrl+V terminal event -> clipboard read -> image badge and attachment -> prompt command.

use super::{app_with_bridge_connection, make_test_app};
use crate::agent::wire::BridgeCommand;
use crate::app::clipboard_image::{ClipboardImagePaste, ClipboardRead, ImageAttachment};
use crate::app::events::handle_terminal_event;
use crate::app::{App, AppStatus, MessageRole, SurfaceMode, SystemSeverity, input_submit};
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

#[test]
fn ctrl_v_without_a_clipboard_image_leaves_the_draft_and_transcript_alone() {
    let mut app = app_with_clipboard(KeyEventKind::Press, || ClipboardRead::NoImage);
    app.input.set_text("draft");

    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));

    assert_eq!(app.input.text(), "draft");
    assert!(app.input.image_payloads.is_empty());
    assert!(app.transcript.messages.is_empty());
}

#[test]
fn clipboard_undo_and_redo_control_the_images_in_the_submitted_prompt() {
    use crate::app::test_support::press;
    for redo in [false, true] {
        let (mut app, mut commands) = app_with_bridge_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} draft suffix");
        app.input.set_cursor(0, 2);
        app.clipboard_paste =
            ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
        handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        assert_eq!(app.input.text(), "\u{e9} [Image #1]draft suffix");
        history_key(&mut app, false);
        assert_eq!(app.input.text(), "\u{e9} draft suffix");
        if redo {
            history_key(&mut app, true);
        }
        let expected = if redo { "\u{e9} [Image #1]draft suffix" } else { "\u{e9} draft suffix" };
        assert_eq!(app.input.text(), expected);
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        let BridgeCommand::Prompt { session_id, chunks, .. } =
            commands.try_recv().expect("submitted draft").command
        else {
            panic!("expected Prompt")
        };
        assert_eq!(session_id, "test-session");
        let text: Vec<_> =
            chunks.iter().filter(|c| c.kind == "text").map(|c| c.value.clone()).collect();
        assert_eq!(text, [serde_json::json!(expected)]);
        let images: Vec<_> = chunks.iter().filter(|c| c.kind == "image").collect();
        assert_eq!(images.len(), usize::from(redo), "undone clipboard image must not be sent");
        if redo {
            assert_eq!(images[0].value["mime_type"], "image/png");
            assert_eq!(
                decoded_rgba(images[0].value["data"].as_str().unwrap()),
                (2, 1, [RED, HALF_BLUE].concat())
            );
        }
    }
}

thread_local! { static CLIPBOARD_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
struct ReadScope(usize);
impl ReadScope {
    fn new() -> Self {
        Self(CLIPBOARD_READS.with(|count| count.replace(0)))
    }
    fn count() -> usize {
        CLIPBOARD_READS.with(std::cell::Cell::get)
    }
}
impl Drop for ReadScope {
    fn drop(&mut self) {
        CLIPBOARD_READS.with(|count| count.set(self.0));
    }
}
fn counted_image() -> ClipboardRead {
    CLIPBOARD_READS.with(|count| count.set(count.get() + 1));
    two_pixel_image()
}
fn green_image() -> ClipboardRead {
    ClipboardRead::Image(arboard::ImageData {
        width: 1,
        height: 2,
        bytes: vec![4, 200, 6, 255, 7, 220, 9, 64].into(),
    })
}
fn forbidden_read() -> ClipboardRead {
    panic!("clipboard reader must not run on this route")
}

#[test]
fn clipboard_encoding_and_edge_policy_reads_once_per_accepted_gesture() {
    for (code, modifiers) in
        [(KeyCode::Char('v'), KeyModifiers::CONTROL), (KeyCode::Char('\u{16}'), KeyModifiers::NONE)]
    {
        for (trigger, edges) in [
            (KeyEventKind::Press, vec![KeyEventKind::Press]),
            (KeyEventKind::Press, vec![KeyEventKind::Press, KeyEventKind::Release]),
            (KeyEventKind::Release, vec![KeyEventKind::Press, KeyEventKind::Release]),
            (KeyEventKind::Release, vec![KeyEventKind::Release]),
        ] {
            let _reads = ReadScope::new();
            let (mut app, mut commands) = app_with_bridge_connection();
            app.input.set_text("\u{e9} suffix");
            app.input.set_cursor(0, 2);
            app.clipboard_paste = ClipboardImagePaste { trigger, read: counted_image };
            let mut accepted = false;
            for edge in edges {
                handle_terminal_event(
                    &mut app,
                    Event::Key(KeyEvent::new_with_kind(code, modifiers, edge)),
                );
                accepted |= edge == trigger;
                assert_eq!(ReadScope::count(), usize::from(accepted));
                assert_eq!(
                    app.input.text(),
                    if accepted { "\u{e9} [Image #1]suffix" } else { "\u{e9} suffix" }
                );
            }
            assert_eq!(app.input.images().len(), 1);
            assert_two_pixel_attachment(&app.input.images()[0]);
            assert!(commands.try_recv().is_err());
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new_with_kind(code, modifiers, trigger)),
            );
            assert_eq!(ReadScope::count(), 2);
            assert_eq!(app.input.text(), "\u{e9} [Image #1][Image #2]suffix");
        }
    }
    for (trigger, key) in [
        (
            KeyEventKind::Press,
            KeyEvent::new_with_kind(
                KeyCode::Char('v'),
                KeyModifiers::CONTROL,
                KeyEventKind::Repeat,
            ),
        ),
        (
            KeyEventKind::Press,
            KeyEvent::new_with_kind(
                KeyCode::Char('v'),
                KeyModifiers::CONTROL,
                KeyEventKind::Release,
            ),
        ),
        (KeyEventKind::Release, KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)),
        (KeyEventKind::Press, KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::ALT)),
        (KeyEventKind::Press, KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT)),
    ] {
        let (mut app, mut commands) = app_with_bridge_connection();
        app.input.set_text("untouched");
        app.clipboard_paste = ClipboardImagePaste { trigger, read: forbidden_read };
        handle_terminal_event(&mut app, Event::Key(key));
        assert_eq!(app.input.text(), "untouched");
        assert!(app.input.images().is_empty());
        assert!(commands.try_recv().is_err());
    }
}

fn assert_prompt_payload(command: BridgeCommand, expected_text: &str, identities: &[bool]) {
    let BridgeCommand::Prompt { session_id, chunks, .. } = command else {
        panic!("expected Prompt")
    };
    assert_eq!(session_id, "test-session");
    assert_eq!(
        chunks.iter().filter(|c| c.kind == "text").map(|c| c.value.clone()).collect::<Vec<_>>(),
        [serde_json::json!(expected_text)]
    );
    let images: Vec<_> = chunks.iter().filter(|c| c.kind == "image").collect();
    assert_eq!(images.len(), identities.len());
    for (chunk, green) in images.iter().zip(identities) {
        assert_eq!(chunk.value["mime_type"], "image/png");
        let expected = if *green {
            (1, 2, vec![4, 200, 6, 255, 7, 220, 9, 64])
        } else {
            (2, 1, [RED, HALF_BLUE].concat())
        };
        assert_eq!(decoded_rgba(chunk.value["data"].as_str().unwrap()), expected);
    }
}
fn no_prompt(commands: &mut crate::agent::client::CommandReceiver) {
    while let Ok(envelope) = commands.try_recv() {
        assert!(!matches!(
            envelope.command,
            BridgeCommand::Prompt { .. } | BridgeCommand::CancelTurn { .. }
        ));
    }
}

#[test]
fn clipboard_images_keep_cursor_association_and_exact_prompt_payloads() {
    for multi in [false, true] {
        let (mut app, mut commands) = app_with_bridge_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} \u{5c3e}");
        app.input.set_cursor(0, 2);
        app.clipboard_paste =
            ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
        handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        if multi {
            app.input.set_cursor(0, 0);
            app.clipboard_paste.read = green_image;
            handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        }
        let expected =
            if multi { "[Image #2]\u{e9} [Image #1]\u{5c3e}" } else { "\u{e9} [Image #1]\u{5c3e}" };
        assert_eq!(app.input.text(), expected);
        assert!(commands.try_recv().is_err());
        input_submit::submit_input(&mut app);
        assert_prompt_payload(
            commands.try_recv().expect("clipboard Prompt").command,
            expected,
            if multi { &[false, true] } else { &[false] },
        );
        assert!(app.input.is_empty());
        assert!(app.input.image_payloads.is_empty());
        no_prompt(&mut commands);
        super::handle_client_event(&mut app, super::turn_complete(None));
        no_prompt(&mut commands);
        app.input.set_text("distinct next prompt");
        input_submit::submit_input(&mut app);
        assert_prompt_payload(
            commands.try_recv().expect("next Prompt").command,
            "distinct next prompt",
            &[],
        );
        no_prompt(&mut commands);
    }
}

fn corrupt_image() -> ClipboardRead {
    ClipboardRead::Image(arboard::ImageData { width: 2, height: 1, bytes: vec![0; 3].into() })
}
fn oversized_image() -> ClipboardRead {
    ClipboardRead::Image(arboard::ImageData {
        width: 1700,
        height: 1700,
        bytes: crate::app::test_support::clipboard_noise(1700, 1700).into(),
    })
}

#[test]
fn clipboard_failure_preserves_prior_content_and_allows_a_successful_retry() {
    use crate::app::{ChatMessage, MessageBlock, TextBlock};
    for (read, causes) in [
        ((|| ClipboardRead::Unavailable) as fn() -> ClipboardRead, vec!["access", "clipboard"]),
        (corrupt_image, vec!["invalid", "image"]),
        (oversized_image, vec!["large", "10 MiB"]),
    ] {
        let (mut app, mut commands) = app_with_bridge_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("draft tail");
        app.input.set_cursor(0, 6);
        app.clipboard_paste =
            ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
        handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        app.input.set_cursor(0, 2);
        app.transcript.messages.push(ChatMessage::new(
            MessageRole::User,
            vec![MessageBlock::Text(TextBlock::from_complete("earlier preserved message"))],
            None,
        ));
        let before = app.input.snapshot();
        let prior_id = app.transcript.messages[0].id;
        let previous_messages = app.transcript.messages.len();
        app.clipboard_paste.read = read;
        handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        assert_eq!(app.input.snapshot(), before);
        assert_eq!(app.transcript.messages[0].id, prior_id);
        let warnings: Vec<_> = app.transcript.messages[previous_messages..]
            .iter()
            .filter(|m| matches!(m.role, MessageRole::System(Some(SystemSeverity::Warning))))
            .flat_map(|m| m.blocks.iter())
            .filter_map(|b| match b {
                MessageBlock::Text(t) => Some(t.text.as_str()),
                MessageBlock::Notice(n) => Some(n.text.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            causes.iter().all(|cause| warnings.iter().any(|text| text.contains(cause))),
            "{warnings:?}"
        );
        assert!(commands.try_recv().is_err());
        app.clipboard_paste.read = green_image;
        handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        input_submit::submit_input(&mut app);
        assert_prompt_payload(
            commands.try_recv().expect("retry Prompt").command,
            "dr[Image #2]aft [Image #1]tail",
            &[false, true],
        );
        assert!(
            matches!(&app.transcript.messages[0].blocks[0], MessageBlock::Text(t) if t.text == "earlier preserved message")
        );
        no_prompt(&mut commands);
    }
}

#[tokio::test]
async fn fullscreen_clipboard_routing_preserves_the_composer_and_remains_reusable() {
    use crate::app::test_support::press;
    let (mut app, mut commands) = app_with_bridge_connection();
    app.status = AppStatus::Ready;
    app.input.set_text("draft tail");
    app.input.set_cursor(0, 6);
    app.clipboard_paste =
        ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    app.input.set_cursor(0, 2);
    let before = app.input.snapshot();
    crate::app::config::open_tab(&mut app, crate::app::ConfigTab::Settings).expect("open Config");
    app.clipboard_paste.read = forbidden_read;
    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    assert_eq!(app.input.snapshot(), before);
    no_prompt(&mut commands);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.surface_mode, SurfaceMode::Chat);
    app.clipboard_paste.read = green_image;
    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    input_submit::submit_input(&mut app);
    assert_prompt_payload(
        commands.try_recv().expect("post-Config Prompt").command,
        "dr[Image #2]aft [Image #1]tail",
        &[false, true],
    );
}

#[test]
fn clipboard_image_follows_accepted_held_text_at_its_original_cursor() {
    use crate::app::test_support::{EventClock, press};
    for typed in ["ab", "abc"] {
        let clock = EventClock::new();
        let (mut app, mut commands) = app_with_bridge_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("\u{e9} |\u{5c3e}");
        app.input.set_cursor(0, 2);
        for ch in typed.chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        app.clipboard_paste =
            ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
        handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        let expected = format!("\u{e9} {typed}[Image #1]|\u{5c3e}");
        assert_eq!(app.input.text(), expected, "clipboard must not overtake held typing");
        if let Some(action) = app.paste.burst.tick(clock.at(1)) {
            crate::app::apply_paste_burst_flush(&mut app, action);
        }
        crate::app::finalize_pending_paste_event(&mut app);
        assert_eq!(app.input.text(), expected, "later tick must not duplicate or reorder input");
        assert!(commands.try_recv().is_err());
        press(&mut app, KeyCode::Enter);
        assert_prompt_payload(
            commands.try_recv().expect("held text and clipboard Prompt").command,
            &expected,
            &[false],
        );
    }
}

fn history_key(app: &mut App, redo: bool) {
    #[cfg(target_os = "macos")]
    let base = KeyModifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let base = KeyModifiers::CONTROL;
    #[cfg(all(unix, not(target_os = "macos")))]
    let undo_char = '_';
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let undo_char = 'z';
    let (ch, modifiers) = if redo { ('z', base | KeyModifiers::SHIFT) } else { (undo_char, base) };
    handle_terminal_event(app, Event::Key(KeyEvent::new(KeyCode::Char(ch), modifiers)));
}

#[test]
fn clipboard_history_keeps_each_payload_and_survives_a_no_op_edit() {
    use crate::app::test_support::press;
    let (mut app, mut commands) = app_with_bridge_connection();
    app.status = AppStatus::Ready;
    app.input.set_text("suffix");
    app.input.set_cursor(0, 0);
    app.clipboard_paste =
        ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    app.clipboard_paste.read = green_image;
    handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
    history_key(&mut app, false);
    assert_eq!(app.input.text(), "[Image #1]suffix");
    assert_eq!(app.input.images().len(), 1);
    assert_two_pixel_attachment(&app.input.images()[0]);
    history_key(&mut app, false);
    assert_eq!(app.input.text(), "suffix");
    assert!(app.input.images().is_empty());
    press(&mut app, KeyCode::Backspace); // At column zero: preserve the redo history.
    history_key(&mut app, true);
    assert_eq!(app.input.text(), "[Image #1]suffix");
    assert_two_pixel_attachment(&app.input.images()[0]);
    history_key(&mut app, true);
    assert_eq!(app.input.text(), "[Image #1][Image #2]suffix");
    press(&mut app, KeyCode::Enter);
    assert_prompt_payload(
        commands.try_recv().expect("restored Prompt").command,
        "[Image #1][Image #2]suffix",
        &[false, true],
    );
}

#[test]
fn editing_after_clipboard_undo_cannot_revive_the_discarded_image() {
    use crate::app::test_support::press;
    for replacement_image in [false, true] {
        let (mut app, mut commands) = app_with_bridge_connection();
        app.status = AppStatus::Ready;
        app.input.set_text("draft ");
        app.clipboard_paste =
            ClipboardImagePaste { trigger: KeyEventKind::Press, read: two_pixel_image };
        handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        history_key(&mut app, false);
        if replacement_image {
            app.clipboard_paste.read = green_image;
            handle_terminal_event(&mut app, ctrl_v(KeyEventKind::Press));
        } else {
            handle_terminal_event(&mut app, Event::Paste("[Image #1]".to_owned()));
            crate::app::finalize_pending_paste_event(&mut app);
        }
        history_key(&mut app, true); // A new edit has discarded the old redo branch.
        assert_eq!(app.input.text(), "draft [Image #1]");
        press(&mut app, KeyCode::Enter);
        assert_prompt_payload(
            commands.try_recv().expect("replacement Prompt").command,
            "draft [Image #1]",
            if replacement_image { &[true] } else { &[] },
        );
    }
}

#[test]
fn clipboard_without_an_image_consumes_both_shortcut_encodings_silently() {
    use crate::app::test_support::press;
    for (code, modifiers) in
        [(KeyCode::Char('v'), KeyModifiers::CONTROL), (KeyCode::Char('\u{16}'), KeyModifiers::NONE)]
    {
        for trigger in [KeyEventKind::Press, KeyEventKind::Release] {
            let (mut app, mut commands) = app_with_bridge_connection();
            app.status = AppStatus::Ready;
            app.input.set_text("draft suffix");
            app.input.set_cursor(0, 6);
            let before = app.input.snapshot();
            app.clipboard_paste = ClipboardImagePaste { trigger, read: || ClipboardRead::NoImage };
            handle_terminal_event(
                &mut app,
                Event::Key(KeyEvent::new_with_kind(code, modifiers, trigger)),
            );
            assert_eq!(app.input.snapshot(), before, "no-image shortcut cannot become draft text");
            assert!(app.transcript.messages.is_empty());
            assert!(commands.try_recv().is_err());
            handle_terminal_event(&mut app, Event::Paste("new ".to_owned()));
            crate::app::finalize_pending_paste_event(&mut app);
            press(&mut app, KeyCode::Enter);
            assert_prompt_payload(
                commands.try_recv().expect("usable composer").command,
                "draft new suffix",
                &[],
            );
        }
    }
}
