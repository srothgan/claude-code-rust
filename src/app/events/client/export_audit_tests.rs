// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::agent::events::HistoryChunk;
use crate::agent::model::{ContentBlock, ContentChunk, SessionUpdate, TextContent};
use crate::app::export::{ExportProgress, apply_chunk_with_writer, begin};
use crate::app::{AppStatus, MessageBlock, PendingCommandAck};
use std::io::Write as _;

fn history(request: &str, text: &str) -> HistoryChunk {
    HistoryChunk {
        request_id: Some(request.to_owned()),
        updates: vec![SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new(text),
        )))],
        last: true,
        omitted_updates: 0,
        unmapped_updates: 0,
        error: None,
    }
}

fn start(app: &mut App) -> String {
    let request = begin(app, "s-1", "retry.md").expect("available explicit path");
    app.status = AppStatus::CommandPending;
    app.turn.pending_command_ack = Some(PendingCommandAck::HistoryExport);
    app.turn.pending_command_label = Some("Exporting conversation...".to_owned());
    request
}

#[test]
fn a_partial_export_write_is_cleaned_up_and_releases_the_command_for_same_path_retry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("retry.md");
    let neighbor = dir.path().join("neighbor.md");
    std::fs::write(&neighbor, b"independent neighbor").expect("seed neighbor");
    let mut app = App::test_default();
    app.cwd_raw = dir.path().display().to_string();
    app.session_runtime.session_title = Some("Retry artifact".to_owned());
    let request = start(&mut app);
    let mut reached_write = false;
    apply_history_export_chunk_with(
        &mut app,
        history(&request, "failed attempt only"),
        |app, chunk| {
            let progress = apply_chunk_with_writer(app, chunk, |file, content| {
                reached_write = true;
                file.write_all(&content.as_bytes()[..23])?;
                // Prove bytes reached the real exclusive-created target before the injected failure.
                assert_eq!(
                    std::fs::read(&path).expect("partial artifact"),
                    &content.as_bytes()[..23]
                );
                Err(std::io::Error::other("controlled partial write failure"))
            });
            assert!(
                matches!(&progress, ExportProgress::Failed(error) if error.contains("controlled partial write failure"))
            );
            progress
        },
    );
    assert!(reached_write);
    assert!(!path.exists());
    assert_eq!(std::fs::read(&neighbor).expect("neighbor preserved"), b"independent neighbor");
    assert!(app.history_export.is_none());
    assert!(app.turn.pending_command_ack.is_none());
    assert!(app.turn.pending_command_label.is_none());
    assert_eq!(app.status, AppStatus::Ready);
    let notice = app.transcript.messages.last().expect("error feedback");
    assert!(notice.blocks.iter().any(|block| matches!(block, MessageBlock::Notice(notice)
        if notice.severity == SystemSeverity::Error && notice.text.text.contains("controlled partial write failure"))));
    crate::app::events::handle_terminal_event(
        &mut app,
        crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        )),
    );
    assert_eq!(app.input.text(), "r");

    let retry = start(&mut app);
    apply_history_export_chunk(&mut app, history(&request, "stale failed content"));
    assert_eq!(app.status, AppStatus::CommandPending);
    assert!(!path.exists());
    apply_history_export_chunk(&mut app, history(&retry, "complete retry content"));
    assert_eq!(app.status, AppStatus::Ready);
    assert!(app.history_export.is_none());
    assert!(app.turn.pending_command_ack.is_none());
    let file = std::fs::read_to_string(&path).expect("retry artifact");
    let timestamp =
        file.lines().find_map(|line| line.strip_prefix("- Exported: ")).expect("export timestamp");
    chrono::DateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S %:z").expect("valid timestamp");
    assert_eq!(
        file,
        format!(
            concat!(
                "# Retry artifact\n\n- Session: s-1\n- Directory: {}\n- Model: test-model\n- Exported: {}\n",
                "- Not included: thinking blocks, images inside tool results, background task state, timing and other session metadata\n\n",
                "## User\n\ncomplete retry content\n\n"
            ),
            dir.path().display(),
            timestamp
        )
    );
    assert_eq!(std::fs::read(neighbor).expect("neighbor still preserved"), b"independent neighbor");
    let mut names: Vec<_> = std::fs::read_dir(dir.path())
        .expect("dir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    names.sort();
    assert_eq!(names, ["neighbor.md", "retry.md"]);
}
