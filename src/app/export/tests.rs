// SPDX-License-Identifier: Apache-2.0

use super::markdown::{Header, render};
use super::{
    Destination, ExportProgress, apply_chunk, begin, begin_at, default_stem, resolve_destination,
    write_new_file,
};
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
fn terminal_control_sequences_never_reach_the_file() {
    let updates = vec![
        user("plain \u{1b}[1mbold\u{1b}[0m text"),
        SessionUpdate::ToolCall(tool("bash-1", "Bash", json!({ "command": "ls" }))),
        result(
            "bash-1",
            ToolCallStatus::Completed,
            "\u{1b}[31mred\u{1b}[0m \u{1b}]0;title\u{7}plain\r\nnext\u{8}",
        ),
    ];

    let markdown = render(&header(), &updates, 0, 0);

    assert!(!markdown.contains('\u{1b}'), "escape byte in {markdown:?}");
    assert!(markdown.contains("plain bold text"));
    assert!(markdown.contains("```\nred plain\nnext\n```"));
}

#[test]
fn long_results_are_cut_at_sixty_lines_with_a_note() {
    // 70 one-character lines: 70 + 69 newlines = 139 characters; 60 lines keep 119.
    let output = vec!["x"; 70].join("\n");
    let updates = vec![
        SessionUpdate::ToolCall(tool("bash-1", "Bash", json!({ "command": "yes" }))),
        result("bash-1", ToolCallStatus::Completed, &output),
    ];

    let markdown = render(&header(), &updates, 0, 0);

    let kept = vec!["x"; 60].join("\n");
    let expected = [
        HEADER_MARKDOWN,
        "## Assistant\n\n### Tool: Bash - yes\n\nResult:\n\n",
        &format!("```\n{kept}\n```\n\n"),
        "_Output truncated: the first 119 of 139 characters are shown._\n\n",
    ]
    .concat();
    assert_eq!(markdown, expected);
}

#[test]
fn a_single_huge_line_is_cut_at_four_thousand_characters() {
    let updates = vec![
        SessionUpdate::ToolCall(tool("bash-1", "Bash", json!({ "command": "cat min.js" }))),
        result("bash-1", ToolCallStatus::Completed, &"y".repeat(5000)),
    ];

    let markdown = render(&header(), &updates, 0, 0);

    assert!(markdown.contains(&format!("```\n{}\n```", "y".repeat(4000))));
    assert!(markdown.contains("_Output truncated: the first 4000 of 5000 characters are shown._"));
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

#[test]
fn a_session_cut_by_the_bridge_says_how_much_is_missing() {
    let markdown = render(&header(), &[user("hi")], 12, 0);

    assert!(markdown.ends_with(
        "---\n\n_Export truncated: 12 later history entries were left out because the session is very large._\n"
    ));
}

const SESSION_ID: &str = "11d785e2-d277-406c-9f99-31d840a3705b";
const DEFAULT_STEM: &str = "export-11d785e2-d277-406c-9f99-31d840a3705b-20261013T123045Z";

fn export_instant() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-10-13T12:30:45Z").expect("fixed instant").to_utc()
}

#[test]
fn default_name_is_the_full_session_id_and_the_utc_basic_timestamp() {
    // Fixed by the upstream issue's example, not derived from the helper.
    assert_eq!(default_stem(SESSION_ID, export_instant()), DEFAULT_STEM);
}

#[test]
fn destinations_resolve_against_the_session_directory_and_home() {
    let cwd = Path::new("/work/proj");
    let home = Path::new("/home/me");
    let explicit = |path: &str| Destination::Explicit(Path::new(path).to_path_buf());
    let default = Destination::Default { dir: cwd.to_path_buf(), stem: "export-x".to_owned() };
    assert_eq!(resolve_destination("", cwd, "export-x", Some(home)), Ok(default.clone()));
    assert_eq!(resolve_destination("  ", cwd, "export-x", Some(home)), Ok(default));
    let cases = [
        ("notes.md", "/work/proj/notes.md"),
        ("sub/notes.md", "/work/proj/sub/notes.md"),
        ("/tmp/out.md", "/tmp/out.md"),
        ("~/out.md", "/home/me/out.md"),
        ("~", "/home/me"),
        ("my notes.md", "/work/proj/my notes.md"),
    ];
    for (requested, expected) in cases {
        assert_eq!(
            resolve_destination(requested, cwd, "export-x", Some(home)),
            Ok(explicit(expected)),
            "{requested:?}"
        );
    }
    // Without a home directory `~` is just a name.
    assert_eq!(
        resolve_destination("~/out.md", cwd, "export-x", None),
        Ok(explicit("/work/proj/~/out.md"))
    );
}

#[test]
fn an_existing_file_is_never_overwritten_and_no_directory_is_created() {
    let dir = tempfile::tempdir().expect("tempdir");
    let existing = dir.path().join("keep.md");
    std::fs::write(&existing, "original").expect("seed");

    let refused = write_new_file(&existing, "new content").expect_err("must refuse");

    assert!(refused.contains("already exists"), "{refused}");
    assert_eq!(std::fs::read_to_string(&existing).expect("read"), "original");

    let nested = dir.path().join("missing").join("out.md");
    let error = write_new_file(&nested, "x").expect_err("no directory is created");
    assert!(error.contains("directory does not exist"), "{error}");
    assert!(!dir.path().join("missing").exists());

    let fresh = dir.path().join("fresh.md");
    let written = write_new_file(&fresh, "hello").expect("write");
    assert_eq!(std::fs::read_to_string(written).expect("read"), "hello");
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

#[test]
fn begin_refuses_an_existing_target_before_any_history_is_requested() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("out.md"), "original").expect("seed");
    let mut app = app_in(dir.path());

    let refusal = begin(&mut app, "s-1", "out.md").expect_err("existing file");

    assert!(refusal.contains("already exists"), "{refusal}");
    assert!(app.history_export.is_none());
}

#[test]
fn chunks_are_collected_by_request_and_written_after_the_last() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let request_id = begin(&mut app, "s-1", "out.md").expect("begin");

    // A chunk from another request never joins this export.
    assert_eq!(
        apply_chunk(&mut app, chunk(Some("other"), vec![user("stale")], true)),
        ExportProgress::Ignored
    );
    assert_eq!(
        apply_chunk(&mut app, chunk(Some(&request_id), vec![user("first")], false)),
        ExportProgress::Collecting
    );
    assert!(!dir.path().join("out.md").exists(), "nothing is written before the last chunk");
    let progress = apply_chunk(&mut app, chunk(Some(&request_id), vec![assistant("second")], true));

    let path = dir.path().join("out.md");
    assert_eq!(progress, ExportProgress::Written(std::path::absolute(&path).expect("abs")));
    let file = std::fs::read_to_string(&path).expect("export file");
    assert!(file.starts_with("# Fix the bug\n\n- Session: s-1\n"), "{file}");
    assert!(file.contains("## User\n\nfirst\n\n## Assistant\n\nsecond\n\n"), "{file}");
    assert!(!file.contains("stale"));
    assert!(app.history_export.is_none());
}

#[test]
fn a_bridge_error_ends_the_export_without_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let request_id = begin(&mut app, "s-1", "out.md").expect("begin");

    let progress = apply_chunk(
        &mut app,
        HistoryChunk {
            error: Some("failed to load session history: boom".to_owned()),
            ..chunk(Some(&request_id), Vec::new(), true)
        },
    );

    assert_eq!(progress, ExportProgress::Failed("failed to load session history: boom".to_owned()));
    assert!(app.history_export.is_none());
    assert!(!dir.path().join("out.md").exists());
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

#[test]
fn a_taken_default_name_gets_the_next_free_numeric_suffix() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(format!("{DEFAULT_STEM}.md")), "first original").expect("seed");

    let second = export_default_once(dir.path(), "second run");
    assert_eq!(second, format!("{DEFAULT_STEM}-2.md"));
    let third = export_default_once(dir.path(), "third run");
    assert_eq!(third, format!("{DEFAULT_STEM}-3.md"));

    // Nothing earlier was touched: the seed is byte-identical and -2 still holds its own run.
    assert_eq!(
        std::fs::read(dir.path().join(format!("{DEFAULT_STEM}.md"))).expect("read"),
        b"first original"
    );
    let second_file = std::fs::read_to_string(dir.path().join(&second)).expect("read -2");
    assert!(second_file.contains("second run") && !second_file.contains("third run"));
    let third_file = std::fs::read_to_string(dir.path().join(&third)).expect("read -3");
    assert!(third_file.contains("third run"));
}

#[test]
fn a_default_name_taken_after_begin_still_never_overwrites() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let request_id = begin_at(&mut app, SESSION_ID, "", export_instant()).expect("begin");
    // Another export wins the name between the request and the last chunk.
    std::fs::write(dir.path().join(format!("{DEFAULT_STEM}.md")), "racer").expect("racer");

    let progress = apply_chunk(&mut app, chunk(Some(&request_id), vec![user("late")], true));

    let ExportProgress::Written(path) = progress else { panic!("expected a written file") };
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some(format!("{DEFAULT_STEM}-2.md").as_str())
    );
    assert_eq!(
        std::fs::read(dir.path().join(format!("{DEFAULT_STEM}.md"))).expect("read"),
        b"racer"
    );
}

#[test]
fn an_explicit_existing_path_stays_an_error_and_gets_no_suffixed_copy() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("mine.md"), "original").expect("seed");
    let mut app = app_in(dir.path());

    let refusal =
        begin_at(&mut app, SESSION_ID, "mine.md", export_instant()).expect_err("existing file");

    assert!(refusal.contains("already exists"), "{refusal}");
    assert!(app.history_export.is_none());
    assert_eq!(std::fs::read(dir.path().join("mine.md")).expect("read"), b"original");
    assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 1);
}

/// A saved assistant message, then the user's next one, rendered as one file body.
fn body_after_header(updates: &[SessionUpdate]) -> String {
    let markdown = render(&header(), updates, 0, 0);
    markdown.strip_prefix(HEADER_MARKDOWN).expect("header comes first").to_owned()
}

#[test]
fn an_unterminated_fence_is_closed_before_the_next_heading() {
    let updates = vec![assistant("Try this:\n```rust\nfn main() {"), user("and then?")];

    let body = body_after_header(&updates);

    let expected = [
        "## Assistant\n\n",
        "Try this:\n```rust\nfn main() {\n```\n\n",
        "## User\n\nand then?\n\n",
    ]
    .concat();
    assert_eq!(body, expected);
}

#[test]
fn a_fence_is_closed_with_its_own_marker_and_length() {
    let updates = vec![assistant("````md\n```\ninner\n```"), user("next"), assistant("~~~~\nopen")];

    let body = body_after_header(&updates);

    // The inner three-backtick lines do not close the four-backtick fence; the tilde fence is
    // closed with tildes of its own length.
    assert!(body.contains("````md\n```\ninner\n```\n````\n\n## User\n\nnext\n\n"), "{body}");
    assert!(body.ends_with("~~~~\nopen\n~~~~\n\n"), "{body}");
}

#[test]
fn a_closed_fence_and_its_contents_are_left_as_written() {
    let text = "Intro\n```\n## User\n```\n## Summary\ntext";

    let body = body_after_header(&[assistant(text)]);

    assert_eq!(body, format!("## Assistant\n\n{text}\n\n"));
}

#[test]
fn an_unterminated_html_comment_is_closed_before_the_next_heading() {
    let body = body_after_header(&[assistant("note <!-- not closed"), user("after")]);

    assert_eq!(
        body,
        "## Assistant\n\nnote <!-- not closed\n-->\n\n## User\n\nafter\n\n".to_owned()
    );
    // A comment the message closes itself gets nothing added.
    let closed = body_after_header(&[assistant("a <!-- b --> c")]);
    assert_eq!(closed, "## Assistant\n\na <!-- b --> c\n\n");
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
fn what_the_export_cannot_show_is_counted_in_a_closing_note() {
    let markdown = render(&header(), &[user("hi")], 12, 3);

    assert!(
        markdown.ends_with(
            "---\n\n_3 saved history entries could not be shown by this app and are not included._\n\n_Export truncated: 12 later history entries were left out because the session is very large._\n"
        ),
        "{markdown}"
    );
    assert!(!render(&header(), &[user("hi")], 0, 0).contains("---"));
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
fn a_stray_osc_start_does_not_swallow_the_rest_of_the_output() {
    let updates = vec![
        SessionUpdate::ToolCall(tool("bash-1", "Bash", json!({ "command": "cat log" }))),
        result(
            "bash-1",
            ToolCallStatus::Completed,
            "ok\u{1b}]0;broken title\nsecond line\nthird line",
        ),
    ];

    let body = body_after_header(&updates);

    assert!(body.contains("```\nok\nsecond line\nthird line\n```"), "{body}");
}

#[test]
fn unmapped_updates_are_summed_across_chunks_into_the_closing_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let request_id = begin(&mut app, "s-1", "out.md").expect("begin");

    let mut first = chunk(Some(&request_id), vec![user("first")], false);
    first.unmapped_updates = 1;
    let mut last = chunk(Some(&request_id), vec![assistant("second")], true);
    last.unmapped_updates = 2;
    apply_chunk(&mut app, first);
    apply_chunk(&mut app, last);

    let file = std::fs::read_to_string(dir.path().join("out.md")).expect("export file");
    assert!(
        file.ends_with(
            "_3 saved history entries could not be shown by this app and are not included._\n\n"
        ),
        "{file}"
    );
}

#[test]
fn an_existing_directory_is_named_as_a_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("sub")).expect("subdirectory");
    let mut app = app_in(dir.path());

    let refusal = begin(&mut app, "s-1", "sub").expect_err("directory target");

    assert!(refusal.contains("is a directory"), "{refusal}");
    assert!(!refusal.contains("overwritten"), "{refusal}");
    assert!(app.history_export.is_none());
}

#[test]
#[cfg(not(windows))]
fn a_backslash_tilde_is_an_ordinary_name_outside_windows() {
    let cwd = Path::new("/work/proj");

    let resolved = resolve_destination("~\\a.md", cwd, "export-x", Some(Path::new("/home/me")));

    assert_eq!(resolved, Ok(Destination::Explicit(cwd.join("~\\a.md"))));
}

#[test]
#[cfg(windows)]
fn windows_paths_resolve_by_their_own_rules() {
    let cwd = Path::new(r"C:\work\proj");
    let home = Path::new(r"C:\Users\me");
    let explicit = |path: &str| Ok(Destination::Explicit(Path::new(path).to_path_buf()));
    let cases = [
        (r"C:\a.md", r"C:\a.md"),
        (r"D:\out dir\a.md", r"D:\out dir\a.md"),
        (r"~\a.md", r"C:\Users\me\a.md"),
        ("~/a.md", r"C:\Users\me\a.md"),
        (r"\\server\share\a.md", r"\\server\share\a.md"),
        (r"sub\a.md", r"C:\work\proj\sub\a.md"),
    ];
    for (requested, expected) in cases {
        assert_eq!(
            resolve_destination(requested, cwd, "export-x", Some(home)),
            explicit(expected),
            "{requested:?}"
        );
    }
}

#[test]
#[cfg(windows)]
fn a_drive_relative_path_is_refused_instead_of_resolved_against_a_drive_directory() {
    let cwd = Path::new(r"C:\work\proj");

    let refusal = resolve_destination("C:notes.md", cwd, "export-x", None).expect_err("refused");

    assert!(refusal.contains("C:notes.md") && refusal.contains("drive"), "{refusal}");
}

#[test]
fn an_unterminated_raw_html_block_is_closed_before_the_next_heading() {
    let cases = [
        ("<pre>\nx", "</pre>"),
        ("a\n<SCRIPT type=x>\nx", "</pre>"),
        ("<style>\nx", "</pre>"),
        ("  <textarea>\nx", "</pre>"),
        ("<![CDATA[\nx", "]]>"),
        ("<?php\nx", "?>"),
        ("<!DOCTYPE html\nx", ">"),
    ];
    for (text, closer) in cases {
        let body = body_after_header(&[assistant(text), user("after")]);

        assert_eq!(body, format!("## Assistant\n\n{text}\n{closer}\n\n## User\n\nafter\n\n"));
    }
}

#[test]
fn a_raw_html_block_the_message_closes_itself_gets_nothing_added() {
    for text in [
        "<pre>\nx\n</pre>\ntail",
        "<script>x</SCRIPT>",
        "<![CDATA[x]]>",
        "<?php x ?>",
        "<!DOCTYPE html>",
        "a <pre> inline mention",
        "<prefix>\nnot a block",
    ] {
        let body = body_after_header(&[assistant(text), user("after")]);

        assert_eq!(body, format!("## Assistant\n\n{text}\n\n## User\n\nafter\n\n"), "{text}");
    }
}

#[test]
fn a_comment_opener_inside_inline_code_does_not_open_a_comment() {
    let text = "use `<!--` to start one";

    let body = body_after_header(&[assistant(text)]);

    assert_eq!(body, format!("## Assistant\n\n{text}\n\n"));
    // An unmatched backtick is literal, so the comment after it still counts.
    let open = body_after_header(&[assistant("a ` <!-- b")]);
    assert_eq!(open, "## Assistant\n\na ` <!-- b\n-->\n\n");
}

#[test]
fn a_tool_name_cannot_forge_a_heading() {
    let call = tool("t1", "Bash\n## User", json!({}));

    let body = body_after_header(&[SessionUpdate::ToolCall(call)]);

    assert!(body.contains("### Tool: Bash ## User\n"), "{body}");
    assert!(!body.contains("\n## User"), "{body}");
}
