// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Simon Peter Rothgang

//! Clipboard image reading: extracts image data from the system clipboard
//! and converts it to a base64-encoded PNG for sending to the agent.

use crossterm::event::KeyEventKind;

/// MIME types supported by the Anthropic Vision API.
/// NOTE: Keep in sync with `SUPPORTED_IMAGE_MIME_TYPES` in
/// `agent-sdk/src/bridge/message_handlers.ts`.
pub const SUPPORTED_IMAGE_MIME_TYPES: &[&str] =
    &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// A pending image attachment: base64-encoded data and its MIME type.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// What the system clipboard held when Ctrl+V asked it for an image.
#[derive(Debug)]
pub enum ClipboardRead {
    Image(arboard::ImageData<'static>),
    NoImage,
    Unavailable,
}

/// The two platform edges of Ctrl+V image paste: which key event carries the
/// shortcut and where the pixels come from. Key handling reads both from here.
#[derive(Debug, Clone, Copy)]
pub struct ClipboardImagePaste {
    pub trigger: KeyEventKind,
    pub read: fn() -> ClipboardRead,
}

impl ClipboardImagePaste {
    /// The system clipboard, read on the Ctrl+V event this platform delivers.
    /// Windows consoles report both key edges, and Windows Terminal keeps the
    /// press for its own paste binding, so only the release reliably arrives.
    /// Other terminals report presses only: the app never requests release
    /// reporting (`REPORT_EVENT_TYPES`, see `terminal_runtime/modes.rs`), so
    /// waiting for a release there never attaches.
    #[must_use]
    pub fn system() -> Self {
        let trigger = if cfg!(windows) { KeyEventKind::Release } else { KeyEventKind::Press };
        Self { trigger, read: read_system_clipboard }
    }
}

fn read_system_clipboard() -> ClipboardRead {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            tracing::warn!("clipboard_image: failed to access system clipboard: {error}");
            return ClipboardRead::Unavailable;
        }
    };
    match clipboard.get_image() {
        Ok(image) => ClipboardRead::Image(image),
        Err(error) => {
            tracing::debug!("clipboard_image: no image on the clipboard: {error}");
            ClipboardRead::NoImage
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

/// Encode already-retrieved clipboard image data to a base64 PNG.
///
/// Returns an encoded attachment or a typed failure reason for UI/logging.
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

    // --- encode_clipboard_image ---

    fn clipboard_pixels(width: usize, height: usize, rgba: Vec<u8>) -> arboard::ImageData<'static> {
        arboard::ImageData { width, height, bytes: rgba.into() }
    }

    #[test]
    fn encoded_attachment_is_a_sendable_png_of_the_clipboard_pixels() {
        use base64::Engine as _;

        // 3x2 with distinct channels and alpha values, so a swapped channel,
        // dropped alpha, or transposed dimension changes the decoded result.
        let rgba: Vec<u8> = vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, // row 0
            9, 8, 7, 255, 1, 2, 3, 4, 250, 251, 252, 253, // row 1
        ];

        let attachment =
            encode_clipboard_image(clipboard_pixels(3, 2, rgba.clone())).expect("attachment");

        assert_eq!(validate_image(&attachment.data, &attachment.mime_type), Ok(()));
        let png = base64::engine::general_purpose::STANDARD
            .decode(&attachment.data)
            .expect("base64 payload");
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("PNG payload")
            .into_rgba8();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        assert_eq!(decoded.into_raw(), rgba);
    }

    #[test]
    fn pixel_buffer_that_does_not_fill_the_announced_size_is_rejected() {
        assert_eq!(
            encode_clipboard_image(clipboard_pixels(2, 2, vec![0; 15])),
            Err(ClipboardImageError::InvalidPixelBuffer)
        );
    }

    #[test]
    fn image_whose_png_exceeds_ten_mebibytes_is_rejected() {
        // Noise does not compress, so 1700x1700 RGBA (11.5 MB raw) stays over the limit.
        let mut state = 0x9E37_79B9_u32;
        let noise: Vec<u8> = (0..1700 * 1700 * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state.to_le_bytes()[1]
            })
            .collect();

        assert_eq!(
            encode_clipboard_image(clipboard_pixels(1700, 1700, noise)),
            Err(ClipboardImageError::TooLarge)
        );
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
