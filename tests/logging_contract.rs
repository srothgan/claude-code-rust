// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::path::{Path, PathBuf};

fn collect_source_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_source_files(&path, files);
            continue;
        }
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if matches!(ext, "rs" | "ts") {
            files.push(path);
        }
    }
}

fn logging_source_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_source_files(Path::new("src"), &mut files);
    collect_source_files(Path::new("agent-sdk/src/bridge"), &mut files);
    files
}

#[test]
fn legacy_logging_markers_and_bridge_console_calls_are_removed() {
    let mut failures = Vec::new();
    let banned_rust_markers = [
        "legacy_bridge_stderr_line",
        "logPermissionDebug",
        "RENDER_SCROLLED",
        "RENDER_CULLED",
        "RENDER_VISIBLE_PREVIEW",
        "[sdk error]",
        "[sdk warn]",
        "[perm debug]",
    ];

    for path in logging_source_files() {
        let Ok(text) = fs::read_to_string(&path) else {
            failures.push(format!("failed to read {}", path.display()));
            continue;
        };

        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            for marker in banned_rust_markers {
                if text.contains(marker) {
                    failures.push(format!(
                        "{} contains banned legacy logging marker `{marker}`",
                        path.display()
                    ));
                }
            }
        }

        if path.extension().and_then(|ext| ext.to_str()) == Some("ts") {
            for console_pattern in
                ["console.error(", "console.warn(", "console.log(", "console.debug("]
            {
                if text.contains(console_pattern) {
                    failures.push(format!(
                        "{} contains banned bridge console logging `{console_pattern}`",
                        path.display()
                    ));
                }
            }
        }
    }

    assert!(failures.is_empty(), "legacy logging patterns remain:\n{}", failures.join("\n"));
}
