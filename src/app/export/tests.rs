// SPDX-License-Identifier: Apache-2.0

#[path = "audit_tests.rs"]
mod audit;

use super::markdown::{Header, render};
use super::{Destination, ExportProgress, apply_chunk, begin, begin_at, resolve_destination};
use crate::agent::events::HistoryChunk;
use crate::agent::model::{
    ContentBlock, ContentChunk, Diff, SessionUpdate, TextContent, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use crate::app::App;
use pretty_assertions::assert_eq;

use serde_json::json;
use std::path::Path;

fn header() -> Header<'static> {
    Header {
        title: Some("Fix the bug"),
        session_id: "s-1",
        cwd: "/work/proj",
        model: Some("Opus"),
        exported_at: "2026-10-12 10:00:00 +00:00",
    }
}

const HEADER_MARKDOWN: &str = "# Fix the bug\n\n- Session: s-1\n- Directory: /work/proj\n- Model: Opus\n- Exported: 2026-10-12 10:00:00 +00:00\n- Not included: thinking blocks, images inside tool results, background task state, timing and other session metadata\n\n";

fn text(text: &str) -> ContentChunk {
    ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
}

fn user(message: &str) -> SessionUpdate {
    SessionUpdate::UserMessageChunk(text(message))
}

fn assistant(message: &str) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(text(message))
}

fn tool(id: &str, name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall::new(id, name)
        .raw_input(input)
        .meta(json!({ "claudeCode": { "toolName": name } }))
        .status(ToolCallStatus::InProgress)
}

fn result(id: &str, status: ToolCallStatus, output: &str) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
        id,
        ToolCallUpdateFields::new()
            .status(status)
            .content(vec![ToolCallContent::from(output)])
            .raw_output(json!(output)),
    ))
}

#[test]
fn a_conversation_becomes_markdown_with_tools_results_and_diffs() {
    let edit = tool("edit-1", "Edit", json!({ "file_path": "/work/proj/a.txt" })).content(vec![
        ToolCallContent::Diff(
            Diff::new("/work/proj/a.txt", "keep\nnew\n").old_text(Some("keep\nold\n")),
        ),
    ]);
    let updates = vec![
        user("Fix the bug"),
        assistant("I'll look."),
        SessionUpdate::ToolCall(tool("bash-1", "Bash", json!({ "command": "ls  -la\n" }))),
        result("bash-1", ToolCallStatus::Completed, "a\nb"),
        assistant("Done"),
        SessionUpdate::ToolCall(edit),
        result_without_text("edit-1"),
        user("Thanks"),
    ];

    let markdown = render(&header(), &updates, 0, 0);

    let expected = [
        HEADER_MARKDOWN,
        "## User\n\nFix the bug\n\n",
        "## Assistant\n\nI'll look.\n\n",
        "### Tool: Bash - ls -la\n\nResult:\n\n```\na\nb\n```\n\n",
        "Done\n\n",
        "### Tool: Edit - /work/proj/a.txt\n\n",
        "```diff\n--- /work/proj/a.txt\n+++ /work/proj/a.txt\n@@ -1,2 +1,2 @@\n keep\n-old\n+new\n```\n\n",
        "## User\n\nThanks\n\n",
    ]
    .concat();
    assert_eq!(markdown, expected);
}

/// A completion that carries no output, as an Edit does once its diff is on the call.
fn result_without_text(id: &str) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
        id,
        ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
    ))
}

#[test]
fn a_failed_tool_is_labelled_as_an_error() {
    let updates = vec![
        SessionUpdate::ToolCall(tool("bash-1", "Bash", json!({ "command": "false" }))),
        result("bash-1", ToolCallStatus::Failed, "exit code 1"),
    ];

    let markdown = render(&header(), &updates, 0, 0);

    let expected = [
        HEADER_MARKDOWN,
        "## Assistant\n\n",
        "### Tool: Bash - false\n\nError:\n\n```\nexit code 1\n```\n\n",
    ]
    .concat();
    assert_eq!(markdown, expected);
}

#[test]
fn a_result_containing_a_fence_gets_a_longer_fence() {
    let updates = vec![
        SessionUpdate::ToolCall(tool("bash-1", "Bash", json!({ "command": "cat README" }))),
        result("bash-1", ToolCallStatus::Completed, "```rust\nfn main() {}\n```"),
    ];

    let markdown = render(&header(), &updates, 0, 0);

    assert!(markdown.contains("````\n```rust\nfn main() {}\n```\n````\n"), "{markdown}");
}

#[test]
fn edit_and_write_calls_without_a_stored_diff_are_shown_from_their_input() {
    let updates = vec![
        SessionUpdate::ToolCall(tool(
            "w-1",
            "Write",
            json!({ "file_path": "/w/n.txt", "content": "hello\n" }),
        )),
        result("w-1", ToolCallStatus::Completed, ""),
        SessionUpdate::ToolCall(tool(
            "e-1",
            "Edit",
            json!({ "file_path": "/w/a.txt", "old_string": "one\n", "new_string": "two\n" }),
        )),
        result("e-1", ToolCallStatus::Completed, ""),
    ];

    let markdown = render(&header(), &updates, 0, 0);

    let expected = [
        HEADER_MARKDOWN,
        "## Assistant\n\n",
        "### Tool: Write - /w/n.txt\n\n",
        "```diff\n--- /dev/null\n+++ /w/n.txt\n@@ -0,0 +1 @@\n+hello\n```\n\n",
        "### Tool: Edit - /w/a.txt\n\n",
        "```diff\n--- /w/a.txt\n+++ /w/a.txt\n@@ -1 +1 @@\n-one\n+two\n```\n\n",
    ]
    .concat();
    assert_eq!(markdown, expected);
}

const SESSION_ID: &str = "11d785e2-d277-406c-9f99-31d840a3705b";
const DEFAULT_STEM: &str = "export-11d785e2-d277-406c-9f99-31d840a3705b-20261013T123045Z";

fn export_instant() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-10-13T12:30:45Z").expect("fixed instant").to_utc()
}

fn chunk(request_id: Option<&str>, updates: Vec<SessionUpdate>, last: bool) -> HistoryChunk {
    HistoryChunk {
        request_id: request_id.map(str::to_owned),
        updates,
        last,
        omitted_updates: 0,
        unmapped_updates: 0,
        error: None,
    }
}

fn app_in(dir: &Path) -> App {
    let mut app = App::test_default();
    app.cwd_raw = dir.display().to_string();
    app.session_runtime.session_title = Some("Fix the bug".to_owned());
    app
}

/// Run a whole default-named export and return the file name it wrote.
fn export_default_once(dir: &Path, message: &str) -> String {
    let mut app = app_in(dir);
    let request_id = begin_at(&mut app, SESSION_ID, "", export_instant()).expect("begin");
    let progress = apply_chunk(&mut app, chunk(Some(&request_id), vec![user(message)], true));
    let ExportProgress::Written(path) = progress else { panic!("expected a written file") };
    assert_eq!(path.parent(), Some(std::path::absolute(dir).expect("abs").as_path()));
    path.file_name().and_then(|name| name.to_str()).expect("file name").to_owned()
}

#[test]
fn the_default_destination_is_a_new_file_in_the_session_directory() {
    let dir = tempfile::tempdir().expect("tempdir");

    let name = export_default_once(dir.path(), "hi");

    assert_eq!(name, format!("{DEFAULT_STEM}.md"));
    let file = std::fs::read_to_string(dir.path().join(&name)).expect("export file");
    assert!(file.contains("## User\n\nhi\n\n"), "{file}");
}

/// A saved assistant message, then the user's next one, rendered as one file body.
fn body_after_header(updates: &[SessionUpdate]) -> String {
    let markdown = render(&header(), updates, 0, 0);
    markdown.strip_prefix(HEADER_MARKDOWN).expect("header comes first").to_owned()
}

#[test]
fn message_text_cannot_forge_the_exports_own_headings() {
    let updates =
        vec![assistant("Done.\n## User\nfake request\n  ### Tool: Bash - rm -rf\n# Assistant")];

    let body = body_after_header(&updates);

    assert_eq!(
        body,
        "## Assistant\n\nDone.\n\\## User\nfake request\n  \\### Tool: Bash - rm -rf\n\\# Assistant\n\n"
    );
    // Only the three headings the export writes are escaped; ordinary headings stay.
    let plain = body_after_header(&[assistant("## Summary\n### Details")]);
    assert_eq!(plain, "## Assistant\n\n## Summary\n### Details\n\n");
}

#[test]
fn saved_notices_and_notifications_are_quoted_notes() {
    use crate::agent::model::SystemNoticeSeverity;
    use crate::agent::notifications::SdkNotification;

    let updates = vec![
        user("/rename review"),
        SessionUpdate::SystemNoticeUpdate {
            severity: SystemNoticeSeverity::Info,
            message: "Session renamed to: review".to_owned(),
        },
        SessionUpdate::SystemNoticeUpdate {
            severity: SystemNoticeSeverity::Error,
            message: "first line\n\n```\nunclosed".to_owned(),
        },
        SessionUpdate::NotificationUpdate {
            notification: SdkNotification::SdkNotice {
                session_id: "s-1".to_owned(),
                uuid: "n-1".to_owned(),
                key: "k".to_owned(),
                text: "Build finished".to_owned(),
                priority: "low".to_owned(),
                color: None,
                timeout_ms: None,
            },
            replay: true,
        },
        assistant("ok"),
    ];

    let body = body_after_header(&updates);

    let expected = [
        "## User\n\n/rename review\n\n",
        "> **Notice:** Session renamed to: review\n\n",
        // The fence the notice leaves open is closed inside the quote.
        "> **Error:** first line\n>\n> ```\n> unclosed\n> ```\n\n",
        "> **Notification:** Build finished\n\n",
        "## Assistant\n\nok\n\n",
    ]
    .concat();
    assert_eq!(body, expected);
}

#[test]
fn images_in_a_tool_result_are_named_as_not_included() {
    let image = ToolCallContent::Content(crate::agent::model::Content::new(ContentBlock::Image(
        crate::agent::model::ImageContent::new("AAAA", "image/png"),
    )));
    let updates = vec![
        SessionUpdate::ToolCall(tool("shot-1", "Screenshot", json!({ "path": "/w/s.png" }))),
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            "shot-1",
            ToolCallUpdateFields::new()
                .status(ToolCallStatus::Completed)
                .content(vec![image.clone(), image]),
        )),
    ];

    let body = body_after_header(&updates);

    assert_eq!(
        body,
        "## Assistant\n\n### Tool: Screenshot - /w/s.png\n\n_Not included: 2 images in this result._\n\n"
    );
}

#[test]
fn a_tool_name_cannot_forge_a_heading() {
    let call = tool("t1", "Bash\n## User", json!({}));

    let body = body_after_header(&[SessionUpdate::ToolCall(call)]);

    assert!(body.contains("### Tool: Bash ## User\n"), "{body}");
    assert!(!body.contains("\n## User"), "{body}");
}
