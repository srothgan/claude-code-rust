// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::path::Path;

const TARGET_STATE_IGNORE_REASON: &str =
    "clean.md target-state guardrail; enable when the corresponding cleanup slice lands";

const PRODUCTION_TRANSCRIPT_RENDER_FILES: &[&str] = &[
    "src/ui/inline_chat_rows.rs",
    "src/app/terminal_runtime/chat_session.rs",
    "src/app/terminal_runtime/chat_terminal.rs",
];

const TERMINAL_TRANSCRIPT_FILES: &[&str] =
    &["src/app/terminal_runtime/chat_session.rs", "src/app/terminal_runtime/chat_terminal.rs"];

const TRANSCRIPT_RENDER_STATE_FILES: &[&str] =
    &["src/app/state/chat_render.rs", "src/app/terminal_runtime/chat_session.rs"];

#[test]
fn obsolete_app_handoff_module_is_deleted() {
    assert!(
        !Path::new("src/app/handoff").exists(),
        "{TARGET_STATE_IGNORE_REASON}: app handoff must not return as a copied transcript source"
    );
}

#[test]
fn production_source_does_not_reference_app_handoff_state() {
    assert_patterns_absent_from_source(&[
        "handoff_shadow",
        "HandoffShadowState",
        "app::handoff",
        "crate::app::handoff",
        "super::handoff",
        "handoff::",
        "mod handoff",
    ]);
}

#[test]
fn production_source_does_not_allow_dead_code() {
    assert_patterns_absent_from_source(&["allow(dead_code)"]);
}

#[test]
fn production_transcript_rendering_does_not_import_handoff_projection() {
    assert_patterns_absent(
        PRODUCTION_TRANSCRIPT_RENDER_FILES,
        &["crate::app::handoff::projection", "handoff::projection"],
    );
}

#[test]
fn production_transcript_rendering_does_not_read_inline_output() {
    assert_patterns_absent(PRODUCTION_TRANSCRIPT_RENDER_FILES, &["handoff_shadow.inline_output"]);
}

#[test]
fn production_transcript_rendering_does_not_read_active_turn_display_shadow() {
    assert_patterns_absent(PRODUCTION_TRANSCRIPT_RENDER_FILES, &["handoff_shadow.active_turn"]);
}

#[test]
fn production_transcript_rendering_does_not_call_live_projection() {
    assert_patterns_absent(
        PRODUCTION_TRANSCRIPT_RENDER_FILES,
        &["inline_live_projection(", "inline_live_projection_after_static_insert("],
    );
}

#[test]
fn production_transcript_rendering_does_not_call_static_insert_plan() {
    assert_patterns_absent(PRODUCTION_TRANSCRIPT_RENDER_FILES, &["inline_static_insert_plan("]);
}

#[test]
fn production_transcript_rendering_does_not_call_history_replay_plan() {
    assert_patterns_absent(PRODUCTION_TRANSCRIPT_RENDER_FILES, &["inline_history_replay_plan("]);
}

#[test]
fn terminal_transcript_path_uses_insert_before_only_for_canonical_scrollback_sink() {
    let path = Path::new("src/app/terminal_runtime/chat_terminal.rs");
    let text = fs::read_to_string(path).expect("failed to read chat terminal source");

    assert!(
        text.contains("fn insert_scrollback_rows("),
        "{TARGET_STATE_IGNORE_REASON}: terminal scrollback insertion must stay in the canonical sink"
    );
    assert!(
        text.contains(".insert_before("),
        "{TARGET_STATE_IGNORE_REASON}: stable chat rows must be inserted into terminal scrollback"
    );
}

#[test]
fn terminal_transcript_path_does_not_keep_static_insert_or_replay_state() {
    assert_patterns_absent(
        TERMINAL_TRANSCRIPT_FILES,
        &[
            "pending_history",
            "ReplayState",
            "HistoryBatchKind",
            "PendingHistoryBatch",
            "queue_static_history",
            "start_replay",
            "flush_queued_history",
            "insert_history_batch",
        ],
    );
}

#[test]
fn production_transcript_rendering_does_not_use_history_in_sync() {
    assert_patterns_absent(TRANSCRIPT_RENDER_STATE_FILES, &["history_in_sync"]);
}

fn assert_patterns_absent(files: &[&str], patterns: &[&str]) {
    let mut failures = Vec::new();

    for file in files {
        let path = Path::new(file);
        let Ok(text) = fs::read_to_string(path) else {
            failures.push(format!("failed to read {}", path.display()));
            continue;
        };

        let compact_text = remove_whitespace(&text);
        for pattern in patterns {
            let compact_pattern = remove_whitespace(pattern);
            if text.contains(pattern) || compact_text.contains(&compact_pattern) {
                failures
                    .push(format!("{} contains banned source pattern `{pattern}`", path.display()));
            }
        }
    }

    assert!(failures.is_empty(), "{TARGET_STATE_IGNORE_REASON}:\n{}", failures.join("\n"));
}

fn assert_patterns_absent_from_source(patterns: &[&str]) {
    let mut files = Vec::new();
    collect_rust_files(Path::new("src"), &mut files);
    assert_patterns_absent_in_paths(&files, patterns);
}

fn assert_patterns_absent_in_paths(files: &[std::path::PathBuf], patterns: &[&str]) {
    let mut failures = Vec::new();

    for path in files {
        let Ok(text) = fs::read_to_string(path) else {
            failures.push(format!("failed to read {}", path.display()));
            continue;
        };

        let compact_text = remove_whitespace(&text);
        for pattern in patterns {
            let compact_pattern = remove_whitespace(pattern);
            if text.contains(pattern) || compact_text.contains(&compact_pattern) {
                failures
                    .push(format!("{} contains banned source pattern `{pattern}`", path.display()));
            }
        }
    }

    assert!(failures.is_empty(), "{TARGET_STATE_IGNORE_REASON}:\n{}", failures.join("\n"));
}

fn collect_rust_files(root: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn remove_whitespace(input: &str) -> String {
    input.chars().filter(|ch| !ch.is_whitespace()).collect()
}
