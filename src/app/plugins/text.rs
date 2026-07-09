// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang

use super::prelude::*;

pub(crate) fn display_label(raw: &str) -> String {
    let normalized = raw.replace('@', " from ").replace('-', " ");
    let mut result = String::with_capacity(normalized.len());
    let mut capitalize_next = true;

    for ch in normalized.chars() {
        if ch == ' ' {
            capitalize_next = true;
            result.push(ch);
            continue;
        }

        if capitalize_next {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.extend(ch.to_lowercase());
        }
    }

    result
}

pub(super) fn marketplace_overlay_description(entry: &MarketplaceSourceEntry) -> String {
    let mut parts = Vec::new();
    if let Some(source) = entry.source.as_deref() {
        parts.push(format!("Source: {source}"));
    }
    if let Some(repo) = entry.repo.as_deref() {
        parts.push(format!("Repo: {repo}"));
    }
    if parts.is_empty() {
        "Manage this configured marketplace.".to_owned()
    } else {
        parts.join("\n")
    }
}

pub(super) fn normalize_project_path(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase()
}

pub(super) fn normalize_single_line_input(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', " ")
}
