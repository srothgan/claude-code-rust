// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

//! Drift check between the `claude-rs` CLI and the CLI Options table in the manual.
//!
//! Only long flag names and their order are enforced. Prose in the Purpose column
//! is free to diverge, so documenting a flag well is never a test failure.

use clap::CommandFactory;
use claude_code_rust::Cli;

/// Long flags clap generates on its own. They are intentionally not in the table.
const IMPLICIT_ARGS: &[&str] = &["help", "version"];

/// Extract the long flag from the first column of a CLI Options row.
///
/// The cell may carry a short alias and a value placeholder, as in
/// `` `-C, --dir <DIR>` ``, and a placeholder may itself contain `|`, which
/// splits the cell early. Scanning for the first `--` token survives both.
fn long_flag_in_cell(cell: &str) -> Option<String> {
    let after_dashes = &cell[cell.find("--")? + 2..];
    let end = after_dashes
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .unwrap_or(after_dashes.len());
    let flag = &after_dashes[..end];
    (!flag.is_empty()).then(|| flag.to_owned())
}

/// Collect the long flags listed in the CLI Options table of the manual.
///
/// The manual is read from disk rather than with `include_str!` so docs are not
/// embedded in any shipped artifact.
fn documented_long_flags() -> Vec<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/usage.md");
    let markdown = std::fs::read_to_string(path);
    assert!(markdown.is_ok(), "failed to read {path}");
    let markdown = markdown.unwrap_or_default();

    let mut flags = Vec::new();
    let mut in_options_section = false;

    for line in markdown.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            in_options_section = heading == "CLI Options";
            continue;
        }
        if !in_options_section || !line.starts_with('|') {
            continue;
        }
        let Some(cell) = line.split('|').nth(1) else {
            continue;
        };
        let cell = cell.trim();
        // The header and separator rows carry no backticked flag.
        if !cell.starts_with('`') {
            continue;
        }
        if let Some(flag) = long_flag_in_cell(cell) {
            flags.push(flag);
        }
    }

    flags
}

#[test]
fn docs_cli_options_table_matches_clap() {
    let command = Cli::command();
    let expected: Vec<String> = command
        .get_arguments()
        .filter(|arg| !IMPLICIT_ARGS.contains(&arg.get_id().as_str()))
        .filter_map(|arg| arg.get_long().map(str::to_owned))
        .collect();

    let documented = documented_long_flags();

    for flag in &expected {
        assert!(
            documented.contains(flag),
            "--{flag} is a `claude-rs` option but is missing from the CLI Options table in docs/src/usage.md"
        );
    }
    for flag in &documented {
        assert!(
            expected.contains(flag),
            "--{flag} is listed in the CLI Options table in docs/src/usage.md but is not a `claude-rs` option"
        );
    }
    assert_eq!(
        documented, expected,
        "the CLI Options table in docs/src/usage.md must list flags in `Cli` declaration order"
    );
}
