// SPDX-License-Identifier: Apache-2.0

//! The system clipboard boundary shared by chat and integration dialogs.

pub(crate) fn copy_text(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|error| format!("Failed to access clipboard: {error}"))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|error| format!("Failed to copy to clipboard: {error}"))
}
