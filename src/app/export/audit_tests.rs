// SPDX-License-Identifier: Apache-2.0

use super::*;
use pretty_assertions::assert_eq;
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use std::fmt::Write as _;

fn saved_body(path: &Path) -> String {
    let file = std::fs::read_to_string(path).expect("saved export");
    file.split_once("timing and other session metadata\n\n").expect("export header").1.to_owned()
}

fn complete(app: &mut App, request: &str, updates: Vec<SessionUpdate>, path: &Path) {
    assert_eq!(
        apply_chunk(app, chunk(Some(request), updates, true)),
        ExportProgress::Written(std::path::absolute(path).expect("absolute target"))
    );
}

#[test]
fn sanitized_export_preserves_readable_content_and_recovers_after_malformed_osc() {
    for (input, expected) in [
        ("\u{1b}[31mred\u{1b}[0m\u{1b}]0;title\u{7}\r\nnext\tcolumn\u{8}", "red\nnext\tcolumn"),
        ("ok\u{1b}]0;broken title\nsecond line\nthird line", "ok\nsecond line\nthird line"),
    ] {
        let body = body_after_header(&[
            user(input),
            SessionUpdate::ToolCall(tool("t", "Bash", json!({"command":"read log"}))),
            result("t", ToolCallStatus::Completed, input),
        ]);
        assert_eq!(
            body,
            format!(
                "## User\n\n{expected}\n\n## Assistant\n\n### Tool: Bash - read log\n\nResult:\n\n```\n{expected}\n```\n\n"
            )
        );
        assert!(!body.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')));
    }
}

fn result_body(source: &str) -> String {
    body_after_header(&[
        SessionUpdate::ToolCall(tool("bounded", "Bash", json!({"command":"read bounded log"}))),
        result("bounded", ToolCallStatus::Completed, source),
    ])
}

fn expect_bounded_result(source: &str, expected: &str, note: &str) {
    assert_eq!(
        result_body(source),
        format!(
            "## Assistant\n\n### Tool: Bash - read bounded log\n\nResult:\n\n```\n{expected}\n```\n\n{note}"
        )
    );
}

#[test]
fn result_budgets_preserve_the_identifiable_prefix_at_line_and_unicode_boundaries() {
    // Independent fixture segments: no production truncation or expected-window calculation.
    let first_59 = (1..=59).map(|n| format!("row-{n:02}")).collect::<Vec<_>>().join("\n");
    let first_60 = format!("{first_59}\nrow-60");
    for source in [&first_59, &first_60] {
        expect_bounded_result(source, source, "");
    }
    expect_bounded_result(
        &format!("{first_60}\nDISCARD"),
        &first_60,
        "_Output truncated: the first 419 of 427 characters are shown._\n\n",
    );
    let prefix = format!("BEGIN:{}", "0123456789".repeat(399)); // 3996 characters
    let below = format!("{prefix}abc");
    let exact = format!("{prefix}abc\u{754c}");
    for source in [&below, &exact] {
        expect_bounded_result(source, source, "");
    }
    expect_bounded_result(
        &format!("{exact}\u{1f642}"),
        &exact,
        "_Output truncated: the first 4000 of 4001 characters are shown._\n\n",
    );
    // Character limit wins although the line limit would also be exceeded later.
    expect_bounded_result(
        &format!("{exact}\n{first_60}"),
        &exact,
        "_Output truncated: the first 4000 of 4420 characters are shown._\n\n",
    );
    // Line limit wins before a later very long line can reach the character budget.
    expect_bounded_result(
        &format!("{first_60}\n{exact}"),
        &first_60,
        "_Output truncated: the first 419 of 4420 characters are shown._\n\n",
    );
}

#[test]
fn exported_diffs_have_their_own_two_hundred_line_budget() {
    let mut first_196 = String::new();
    let mut expected_196 = String::new();
    for n in 1..=196 {
        writeln!(first_196, "line-{n:03}").expect("fixture");
        writeln!(expected_196, "+line-{n:03}").expect("fixture");
    }
    let kept_lines = format!("{expected_196}+line-197\n");
    for (tail, count, cut) in
        [("", 196, false), ("line-197\n", 197, false), ("line-197\nTAIL-198\n", 198, true)]
    {
        let mut call = tool("diff", "Write", json!({"file_path":"new.txt"}));
        call.content =
            vec![ToolCallContent::Diff(Diff::new("new.txt", format!("{first_196}{tail}")))];
        let body = body_after_header(&[SessionUpdate::ToolCall(call)]);
        let lines = if count == 196 { expected_196.clone() } else { kept_lines.clone() };
        let note = if cut {
            "_Output truncated: the first 2013 of 2024 characters are shown._\n\n"
        } else {
            ""
        };
        assert_eq!(
            body,
            format!(
                "## Assistant\n\n### Tool: Write - new.txt\n\n```diff\n--- /dev/null\n+++ new.txt\n@@ -0,0 +1,{count} @@\n{lines}```\n\n{note}"
            )
        );
    }
}

fn assert_footer(footer: &str, omitted: u64, unmapped: u64) {
    if omitted == 0 && unmapped == 0 {
        assert_eq!(footer, "");
        return;
    }
    let notes = footer.strip_prefix("---\n\n").expect("footer separator");
    let lines: Vec<_> = notes.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), usize::from(omitted > 0) + usize::from(unmapped > 0));
    if unmapped > 0 {
        let line =
            lines.iter().find(|line| line.contains("could not be shown")).expect("mapping loss");
        assert!(line.contains(&format!("{unmapped} saved history entries")), "{line}");
    }
    if omitted > 0 {
        let line =
            lines.iter().find(|line| line.contains("session is very large")).expect("bridge limit");
        assert!(line.contains(&format!("{omitted} later history entries")), "{line}");
    }
}

#[test]
fn omission_notes_keep_the_two_loss_causes_separate_from_the_body() {
    for (omitted, unmapped) in [(12, 0), (0, 3), (12, 3), (0, 0)] {
        let file = render(&header(), &[user("before\n\n---\n\nafter")], omitted, unmapped);
        let footer = file
            .strip_prefix(HEADER_MARKDOWN)
            .expect("header")
            .strip_prefix("## User\n\nbefore\n\n---\n\nafter\n\n")
            .expect("unchanged body");
        assert_footer(footer, omitted, unmapped);
    }
}

#[test]
fn explicit_completion_refuses_late_claims_and_missing_parents_but_writes_a_fresh_target() {
    for case in ["claimed", "missing", "fresh"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_in(dir.path());
        let requested = if case == "missing" { "missing/out.md" } else { "out.md" };
        let path = dir.path().join(requested);
        let request = begin(&mut app, "s-1", requested).expect("begin while absent");
        if case == "claimed" {
            std::fs::write(&path, b"racer bytes").expect("claim");
        }
        let progress =
            apply_chunk(&mut app, chunk(Some(&request), vec![user("requested body")], true));
        if case == "fresh" {
            assert_eq!(
                progress,
                ExportProgress::Written(std::path::absolute(&path).expect("absolute"))
            );
            assert_eq!(saved_body(&path), "## User\n\nrequested body\n\n");
        } else {
            let ExportProgress::Failed(error) = progress else { panic!("must fail: {progress:?}") };
            assert!(
                error.contains(if case == "claimed" {
                    "already exists"
                } else {
                    "directory does not exist"
                }),
                "{error}"
            );
            if case == "claimed" {
                assert_eq!(std::fs::read(&path).expect("racer"), b"racer bytes");
            } else {
                assert!(!dir.path().join("missing").exists());
            }
        }
        assert!(app.history_export.is_none());
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(
            names,
            if case == "missing" { vec![] } else { vec![std::ffi::OsString::from("out.md")] }
        );
    }
}

#[test]
fn only_matching_chunks_reach_the_file_and_old_completions_cannot_affect_reuse() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let path = dir.path().join("out.md");
    let request = begin(&mut app, "s-1", "out.md").expect("begin");
    for (id, message) in [(Some("wrong"), "first valid"), (None, "second valid")] {
        assert_eq!(
            apply_chunk(&mut app, chunk(id, vec![user("foreign")], true)),
            ExportProgress::Ignored
        );
        assert_eq!(
            apply_chunk(&mut app, chunk(Some(&request), vec![user(message)], false)),
            ExportProgress::Collecting
        );
        assert!(!path.exists());
    }
    complete(&mut app, &request, vec![assistant("finished")], &path);
    assert_eq!(
        saved_body(&path),
        "## User\n\nfirst valid\n\nsecond valid\n\n## Assistant\n\nfinished\n\n"
    );
    let original = std::fs::read(&path).expect("first artifact");
    assert_eq!(
        apply_chunk(&mut app, chunk(Some(&request), vec![user("late")], true)),
        ExportProgress::Ignored
    );
    let fresh = begin(&mut app, "s-1", "fresh.md").expect("reuse");
    assert_eq!(
        apply_chunk(&mut app, chunk(Some(&request), vec![user("old request")], true)),
        ExportProgress::Ignored
    );
    assert!(!dir.path().join("fresh.md").exists());
    complete(&mut app, &fresh, vec![user("fresh request")], &dir.path().join("fresh.md"));
    assert_eq!(saved_body(&dir.path().join("fresh.md")), "## User\n\nfresh request\n\n");
    assert_eq!(std::fs::read(path).expect("unchanged first artifact"), original);
}

#[test]
fn bridge_failure_discards_collected_text_and_late_events_before_retry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let old = begin(&mut app, "s-1", "out.md").expect("begin");
    assert_eq!(
        apply_chunk(&mut app, chunk(Some(&old), vec![user("discard collected text")], false)),
        ExportProgress::Collecting
    );
    let failure = || HistoryChunk {
        error: Some("history read failed: sentinel".to_owned()),
        ..chunk(Some(&old), vec![], true)
    };
    assert_eq!(
        apply_chunk(&mut app, failure()),
        ExportProgress::Failed("history read failed: sentinel".to_owned())
    );
    assert!(app.history_export.is_none());
    assert!(!dir.path().join("out.md").exists());
    let retry = begin(&mut app, "s-1", "out.md").expect("retry same target");
    assert_eq!(apply_chunk(&mut app, failure()), ExportProgress::Ignored);
    assert_eq!(
        apply_chunk(&mut app, chunk(Some(&old), vec![user("late text")], true)),
        ExportProgress::Ignored
    );
    complete(&mut app, &retry, vec![assistant("only retry")], &dir.path().join("out.md"));
    assert_eq!(saved_body(&dir.path().join("out.md")), "## Assistant\n\nonly retry\n\n");
}

#[test]
fn default_exports_fill_the_first_gap_and_preserve_each_runs_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (suffix, bytes) in [("", "original base"), ("-3", "original third")] {
        std::fs::write(dir.path().join(format!("{DEFAULT_STEM}{suffix}.md")), bytes).expect("seed");
    }
    for (message, suffix) in [("second run", "-2"), ("fourth run", "-4")] {
        let name = export_default_once(dir.path(), message);
        assert_eq!(name, format!("{DEFAULT_STEM}{suffix}.md"));
        assert_eq!(saved_body(&dir.path().join(name)), format!("## User\n\n{message}\n\n"));
    }
    for (suffix, bytes) in [("", "original base"), ("-3", "original third")] {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(format!("{DEFAULT_STEM}{suffix}.md")))
                .expect("seed preserved"),
            bytes
        );
    }
    assert_eq!(
        saved_body(&dir.path().join(format!("{DEFAULT_STEM}-2.md"))),
        "## User\n\nsecond run\n\n"
    );
}

#[test]
fn a_late_default_collision_preserves_the_racer_and_writes_the_requested_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let request = begin_at(&mut app, SESSION_ID, "", export_instant()).expect("begin");
    let first = dir.path().join(format!("{DEFAULT_STEM}.md"));
    std::fs::write(&first, b"racer").expect("claim");
    let next = dir.path().join(format!("{DEFAULT_STEM}-2.md"));
    complete(&mut app, &request, vec![user("late request body")], &next);
    assert_eq!(std::fs::read(first).expect("racer"), b"racer");
    assert_eq!(saved_body(&next), "## User\n\nlate request body\n\n");
}

#[test]
fn occupied_explicit_files_and_directories_are_preserved_without_alternate_exports() {
    for directory in [false, true] {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mine.md");
        let sentinel = if directory {
            std::fs::create_dir(&path).expect("directory");
            path.join("child")
        } else {
            path.clone()
        };
        std::fs::write(&sentinel, b"independent bytes").expect("seed");
        let mut app = app_in(dir.path());
        let error = begin(&mut app, SESSION_ID, "mine.md").expect_err("occupied");
        assert!(
            error.contains(if directory { "is a directory" } else { "already exists" }),
            "{error}"
        );
        if directory {
            assert!(error.contains("file name"), "{error}");
        }
        assert!(app.history_export.is_none());
        assert_eq!(std::fs::read(sentinel).expect("preserved"), b"independent bytes");
        assert_eq!(
            std::fs::read_dir(dir.path())
                .expect("dir")
                .map(|e| e.expect("entry").path())
                .collect::<Vec<_>>(),
            vec![path]
        );
    }
}

fn assert_following_user_is_outside_raw_or_code(body: &str, expected_headings: &[&str]) {
    let mut headings = Vec::new();
    let mut heading = None;
    let mut paragraphs = Vec::new();
    let mut paragraph = None;
    for event in Parser::new(body) {
        match event {
            Event::Start(Tag::Heading { level: HeadingLevel::H2, .. }) => {
                heading = Some(String::new());
            }
            Event::End(TagEnd::Heading(HeadingLevel::H2)) => {
                headings.push(heading.take().expect("heading"));
            }
            Event::Start(Tag::Paragraph) => paragraph = Some(String::new()),
            Event::End(TagEnd::Paragraph) => paragraphs.push(paragraph.take().expect("paragraph")),
            Event::Text(text) => {
                if let Some(h) = &mut heading {
                    h.push_str(&text);
                }
                if let Some(p) = &mut paragraph {
                    p.push_str(&text);
                }
            }
            _ => {}
        }
    }
    assert_eq!(headings, expected_headings, "{body}");
    assert_eq!(paragraphs.last().map(String::as_str), Some("following real message"), "{body}");
}

#[test]
fn message_fences_preserve_literal_code_and_contain_the_next_role() {
    for (source, literal) in [
        ("Try this:\n```rust\nfn main() {", "Try this:\n```rust\nfn main() {\n```"),
        ("````md\n```\ninner\n```", "````md\n```\ninner\n```\n````"),
        ("~~~~\nopen", "~~~~\nopen\n~~~~"),
        (
            "Intro\n```\n## User\n```\n## Summary\ntext",
            "Intro\n```\n## User\n```\n## Summary\ntext",
        ),
    ] {
        let body = body_after_header(&[assistant(source), user("following real message")]);
        assert_eq!(
            body,
            format!("## Assistant\n\n{literal}\n\n## User\n\nfollowing real message\n\n")
        );
        // The ordinary Summary heading in the closed control is intentionally preserved.
        if source.contains("## Summary") {
            assert_following_user_is_outside_raw_or_code(&body, &["Assistant", "Summary", "User"]);
        } else {
            assert_following_user_is_outside_raw_or_code(&body, &["Assistant", "User"]);
        }
    }
}

#[test]
fn comments_and_raw_blocks_cannot_swallow_the_following_message() {
    // pulldown-cmark 0.13.5 searches raw closing tags case-sensitively. Use an
    // independent byte fixture for this valid, already-closed case instead of
    // requiring that parser limitation to change the user's source text.
    assert_eq!(
        body_after_header(&[assistant("<script>x</SCRIPT>"), user("following real message")]),
        "## Assistant\n\n<script>x</SCRIPT>\n\n## User\n\nfollowing real message\n\n"
    );

    for source in [
        "note <!-- not closed",
        "<pre>\nx",
        "a\n<SCRIPT type=x>\nx",
        "<style>\nx",
        "  <textarea>\nx",
        "<![CDATA[\nx",
        "<?php\nx",
        "<!DOCTYPE html\nx",
        "a ` <!-- b",
    ] {
        let body = body_after_header(&[assistant(source), user("following real message")]);
        assert!(body.starts_with(&format!("## Assistant\n\n{source}\n")), "{body}");
        assert_following_user_is_outside_raw_or_code(&body, &["Assistant", "User"]);
    }
    for source in [
        "a <!-- b --> c",
        "<pre>\nx\n</pre>\ntail",
        "<![CDATA[x]]>",
        "<?php x ?>",
        "<!DOCTYPE html>",
        "a <pre> inline mention",
        "<prefix>\nnot a block",
        "use `<!--` to start one",
    ] {
        let body = body_after_header(&[assistant(source), user("following real message")]);
        assert_eq!(
            body,
            format!("## Assistant\n\n{source}\n\n## User\n\nfollowing real message\n\n")
        );
        assert_following_user_is_outside_raw_or_code(&body, &["Assistant", "User"]);
    }
}

#[test]
fn chunk_assembly_preserves_the_body_and_separate_loss_totals() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let request = begin(&mut app, "s-1", "out.md").expect("begin");
    let mut first = chunk(Some(&request), vec![user("first body")], false);
    first.unmapped_updates = 2;
    assert_eq!(apply_chunk(&mut app, first), ExportProgress::Collecting);
    assert!(!dir.path().join("out.md").exists());
    let mut last = chunk(Some(&request), vec![assistant("second body")], true);
    last.unmapped_updates = 3;
    last.omitted_updates = 17;
    assert_eq!(
        apply_chunk(&mut app, last),
        ExportProgress::Written(std::path::absolute(dir.path().join("out.md")).expect("absolute"))
    );
    let body = saved_body(&dir.path().join("out.md"));
    let footer = body
        .strip_prefix("## User\n\nfirst body\n\n## Assistant\n\nsecond body\n\n")
        .expect("ordered content");
    assert_footer(footer, 17, 5);
}

#[test]
fn overlapping_tools_keep_their_own_results_statuses_and_call_order_across_chunks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_in(dir.path());
    let request = begin(&mut app, "s-1", "out.md").expect("begin");
    assert_eq!(
        apply_chunk(
            &mut app,
            chunk(
                Some(&request),
                vec![
                    user("check both"),
                    SessionUpdate::ToolCall(tool(
                        "first",
                        "Bash",
                        json!({"command":"check alpha"})
                    )),
                    SessionUpdate::ToolCall(tool(
                        "second",
                        "Bash",
                        json!({"command":"check beta"})
                    ))
                ],
                false
            )
        ),
        ExportProgress::Collecting
    );
    assert!(!dir.path().join("out.md").exists());
    complete(
        &mut app,
        &request,
        vec![
            result("second", ToolCallStatus::Failed, "beta denied"),
            result("first", ToolCallStatus::Completed, "alpha passed"),
            assistant("both checked"),
        ],
        &dir.path().join("out.md"),
    );
    assert_eq!(
        saved_body(&dir.path().join("out.md")),
        concat!(
            "## User\n\ncheck both\n\n## Assistant\n\n",
            "### Tool: Bash - check alpha\n\nResult:\n\n```\nalpha passed\n```\n\n",
            "### Tool: Bash - check beta\n\nError:\n\n```\nbeta denied\n```\n\n",
            "both checked\n\n"
        )
    );
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
    #[cfg(not(windows))]
    assert_eq!(
        resolve_destination(r"~\a.md", cwd, "export-x", Some(home)),
        Ok(explicit(r"/work/proj/~\a.md"))
    );
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
    let refusal = resolve_destination("C:notes.md", cwd, "export-x", None)
        .expect_err("drive relative refused");
    assert!(
        refusal.contains("C:notes.md")
            && refusal.contains("drive")
            && refusal.contains("full path"),
        "{refusal}"
    );
}
