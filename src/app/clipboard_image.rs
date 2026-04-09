// Copyright 2026 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

//! Clipboard image reading: extracts image data from the system clipboard
//! and converts it to a base64-encoded PNG for sending to the agent.

/// MIME types supported by the Anthropic Vision API.
/// NOTE: Keep in sync with `SUPPORTED_IMAGE_MIME_TYPES` in
/// `agent-sdk/src/bridge/message_handlers.ts`.
pub const SUPPORTED_IMAGE_MIME_TYPES: &[&str] =
    &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// A pending image attachment: base64-encoded data and its MIME type.
#[derive(Debug, Clone)]
pub struct ImageAttachment {
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardImageError {
    InvalidDimensions,
    InvalidPixelBuffer,
    EncodeFailed,
    TooLarge,
}

impl ClipboardImageError {
    #[must_use]
    pub fn user_message(self) -> &'static str {
        match self {
            ClipboardImageError::InvalidDimensions | ClipboardImageError::InvalidPixelBuffer => {
                "Clipboard image data is invalid and could not be attached."
            }
            ClipboardImageError::EncodeFailed => {
                "Clipboard image could not be converted to PNG for upload."
            }
            ClipboardImageError::TooLarge => {
                "Clipboard image is too large to attach. Keep images under 10 MiB."
            }
        }
    }
}

/// Returns `true` if `mime_type` is a supported image MIME type.
pub fn is_supported_image_type(mime_type: &str) -> bool {
    SUPPORTED_IMAGE_MIME_TYPES.contains(&mime_type)
}

/// Returns `true` if `data` is non-empty, correctly padded, and decodes as
/// valid standard base64.
///
/// NOTE: This is intentionally strict (requires padding) to match the
/// `isValidBase64` check in `agent-sdk/src/bridge/message_handlers.ts`.
pub fn is_valid_base64(data: &str) -> bool {
    use base64::Engine as _;

    if data.is_empty() {
        return false;
    }
    // Quick structural check matching the TS regex: length must be a multiple
    // of 4, only valid charset, padding only at end (max 2 '=' chars).
    let clean = data.trim();
    if !clean.len().is_multiple_of(4) {
        return false;
    }
    // Verify it actually decodes with the strict (padded) engine.
    base64::engine::general_purpose::STANDARD.decode(clean).is_ok()
}

/// Validate an image attachment before sending to the API.
/// Returns `Ok(())` or an error description.
pub fn validate_image(data: &str, mime_type: &str) -> Result<(), String> {
    if !is_supported_image_type(mime_type) {
        return Err(format!(
            "unsupported image type \"{mime_type}\"; expected one of: {}",
            SUPPORTED_IMAGE_MIME_TYPES.join(", ")
        ));
    }
    if !is_valid_base64(data) {
        return Err("image data is not valid base64".to_owned());
    }
    Ok(())
}

/// Find `[Image #N]` badge spans in a line.
///
/// Returns `(byte_start, byte_end, 1-based_index)` for each badge found.
/// Shared by both the input editing logic and the UI highlighting code.
pub fn find_image_badge_spans(line: &str) -> Vec<(usize, usize, usize)> {
    let mut spans = Vec::new();
    let mut search_from = 0;
    while let Some(start) = line[search_from..].find("[Image #") {
        let abs_start = search_from + start;
        if let Some(end_rel) = line[abs_start..].find(']') {
            let abs_end = abs_start + end_rel + 1;
            let inner = &line[abs_start + 8..abs_start + end_rel];
            if !inner.is_empty()
                && inner.chars().all(|c| c.is_ascii_digit())
                && let Ok(idx) = inner.parse::<usize>()
            {
                spans.push((abs_start, abs_end, idx));
            }
            search_from = abs_end;
        } else {
            break;
        }
    }
    spans
}

/// Encode already-retrieved clipboard image data to a base64 PNG.
///
/// Accepts the `arboard::ImageData` obtained from a clipboard that the caller
/// has already opened, avoiding a redundant `Clipboard::new()` call.
///
/// Returns an encoded attachment or a typed failure reason for UI/logging.
#[cfg(not(test))]
pub fn encode_clipboard_image(
    img_data: arboard::ImageData<'_>,
) -> Result<ImageAttachment, ClipboardImageError> {
    use base64::Engine as _;

    const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

    // Convert RGBA pixel data to PNG using the `image` crate.
    let width =
        u32::try_from(img_data.width).map_err(|_| ClipboardImageError::InvalidDimensions)?;
    let height =
        u32::try_from(img_data.height).map_err(|_| ClipboardImageError::InvalidDimensions)?;
    let rgba_bytes: Vec<u8> = img_data.bytes.into_owned();

    let img_buf = image::RgbaImage::from_raw(width, height, rgba_bytes)
        .ok_or(ClipboardImageError::InvalidPixelBuffer)?;
    let mut png_bytes: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut png_bytes);
    if let Err(e) = img_buf.write_to(&mut cursor, image::ImageFormat::Png) {
        tracing::warn!("clipboard_image: failed to encode PNG: {e}");
        return Err(ClipboardImageError::EncodeFailed);
    }

    if png_bytes.len() > MAX_IMAGE_BYTES {
        tracing::warn!(
            size = png_bytes.len(),
            max = MAX_IMAGE_BYTES,
            "clipboard_image: image too large, ignoring"
        );
        return Err(ClipboardImageError::TooLarge);
    }

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

    tracing::debug!(
        width,
        height,
        png_bytes = png_bytes.len(),
        base64_len = base64_data.len(),
        "clipboard_image: successfully read image from clipboard"
    );

    Ok(ImageAttachment { data: base64_data, mime_type: "image/png".to_owned() })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_supported_image_type ---

    #[test]
    fn supported_types_accepted() {
        for mime in SUPPORTED_IMAGE_MIME_TYPES {
            assert!(is_supported_image_type(mime), "{mime} should be supported");
        }
    }

    #[test]
    fn unsupported_types_rejected() {
        assert!(!is_supported_image_type("image/bmp"));
        assert!(!is_supported_image_type("text/plain"));
        assert!(!is_supported_image_type(""));
    }

    // --- is_valid_base64 ---

    #[test]
    fn valid_base64_accepted() {
        assert!(is_valid_base64("aGVsbG8=")); // "hello"
        assert!(is_valid_base64("AAAA"));
        assert!(is_valid_base64("AA=="));
    }

    #[test]
    fn invalid_base64_rejected() {
        assert!(!is_valid_base64(""));
        assert!(!is_valid_base64("A")); // bad length
        assert!(!is_valid_base64("AAA!")); // invalid char
        assert!(!is_valid_base64("AAA")); // not padded (length % 4 != 0)
        assert!(!is_valid_base64("A=AA")); // padding in the middle
        assert!(!is_valid_base64("====")); // all padding, no data
    }

    // --- validate_image ---

    #[test]
    fn validate_image_rejects_bad_mime() {
        let err = validate_image("AAAA", "image/bmp").unwrap_err();
        assert!(err.contains("unsupported image type"));
    }

    #[test]
    fn validate_image_rejects_bad_base64() {
        let err = validate_image("!!!", "image/png").unwrap_err();
        assert!(err.contains("not valid base64"));
    }

    #[test]
    fn validate_image_accepts_valid() {
        assert!(validate_image("aGVsbG8=", "image/png").is_ok());
        assert!(validate_image("aGVsbG8=", "image/jpeg").is_ok());
        assert!(validate_image("aGVsbG8=", "image/gif").is_ok());
        assert!(validate_image("aGVsbG8=", "image/webp").is_ok());
    }

    // --- find_image_badge_spans ---

    #[test]
    fn find_badge_spans_basic() {
        let spans = find_image_badge_spans("hello [Image #1] world [Image #2]");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].2, 1);
        assert_eq!(spans[1].2, 2);
    }

    #[test]
    fn find_badge_spans_ignores_non_numeric() {
        let spans = find_image_badge_spans("[Image #abc] [Image #1]");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].2, 1);
    }

    #[test]
    fn find_badge_spans_empty() {
        assert!(find_image_badge_spans("no badges here").is_empty());
    }

    #[test]
    fn clipboard_image_error_messages_are_stable() {
        assert_eq!(
            ClipboardImageError::InvalidDimensions.user_message(),
            "Clipboard image data is invalid and could not be attached."
        );
        assert_eq!(
            ClipboardImageError::InvalidPixelBuffer.user_message(),
            "Clipboard image data is invalid and could not be attached."
        );
        assert_eq!(
            ClipboardImageError::EncodeFailed.user_message(),
            "Clipboard image could not be converted to PNG for upload."
        );
        assert_eq!(
            ClipboardImageError::TooLarge.user_message(),
            "Clipboard image is too large to attach. Keep images under 10 MiB."
        );
    }
}
