// SPDX-License-Identifier: Apache-2.0

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
/// Clipboard encoding emits canonical standard base64. This outgoing boundary also
/// accepts surrounding whitespace, but requires nonempty decoded data and correct padding.
pub fn is_valid_base64(data: &str) -> bool {
    use base64::Engine as _;

    if data.trim().is_empty() {
        return false;
    }
    // The strict decoder checks alphabet and padding after the length check.
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

    // --- is_valid_base64 ---

    // --- validate_image ---

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

        assert_eq!(attachment.mime_type, "image/png");
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
    fn attachment_validation_rejects_blank_data_and_preserves_valid_prompt_neighbors() {
        use crate::agent::wire::BridgeCommand;
        for (mime, data, valid, cause) in [
            ("image/jpeg", "AA==", true, ""),
            ("image/png", "AAA=", true, ""),
            ("image/gif", "AAAA", true, ""),
            ("image/webp", "aGVsbG8=", true, ""),
            ("image/bmp", "AAAA", false, "type"),
            ("text/plain", "AAAA", false, "type"),
            ("", "AAAA", false, "type"),
            ("image/png", "", false, "base64"),
            ("image/png", " \t\r\n", false, "base64"),
            ("image/png", "A", false, "base64"),
            ("image/png", "AAA!", false, "base64"),
            ("image/png", "A===", false, "base64"),
            ("image/png", "=AAA", false, "base64"),
            ("image/png", "AA=A", false, "base64"),
            ("image/png", "AA", false, "base64"),
            ("image/png", "A A==", false, "base64"),
            ("image/png", "AAAA====", false, "base64"),
            // The supported clipboard encoder emits canonical base64; surrounding ASCII whitespace
            // is tolerated at this boundary and by the bridge, but cleaned-empty data is not.
            ("image/png", " \nAA==\t", true, ""),
        ] {
            let result = validate_image(data, mime);
            assert_eq!(result.is_ok(), valid, "{mime} {data:?}");
            if !valid {
                assert!(result.unwrap_err().contains(cause));
            }
            let (connection, mut commands) = crate::agent::client::AgentConnection::test_channel();
            let images = vec![
                ImageAttachment { data: "AQID".into(), mime_type: "image/gif".into() },
                ImageAttachment { data: data.into(), mime_type: mime.into() },
                ImageAttachment { data: "BAUG".into(), mime_type: "image/webp".into() },
            ];
            connection
                .prompt_with_images_and_pastes(
                    "session-1".into(),
                    "image-contract".into(),
                    "literal source".into(),
                    images,
                    vec!["paste source".into()],
                )
                .expect("accepted prompt");
            let BridgeCommand::Prompt { session_id, chunks, inline_pastes, .. } =
                commands.try_recv().expect("actual Prompt").command
            else {
                panic!("expected Prompt")
            };
            assert_eq!(session_id, "session-1");
            let actual: Vec<_> = chunks
                .iter()
                .filter(|c| c.kind == "image")
                .map(|c| {
                    (c.value["mime_type"].as_str().unwrap(), c.value["data"].as_str().unwrap())
                })
                .collect();
            let mut expected = vec![("image/gif", "AQID")];
            if valid {
                expected.push((mime, data));
            }
            expected.push(("image/webp", "BAUG"));
            assert_eq!(actual, expected);
            assert_eq!(
                chunks
                    .iter()
                    .filter(|c| c.kind == "text")
                    .map(|c| c.value.clone())
                    .collect::<Vec<_>>(),
                [serde_json::json!("literal source")]
            );
            assert_eq!(inline_pastes, ["paste source"]);
        }
    }

    #[test]
    fn clipboard_limit_applies_to_png_bytes_not_raw_pixels_or_base64_text() {
        use base64::Engine as _;
        const LIMIT: usize = 10 * 1024 * 1024;
        for (width, height, rgba, accepted) in [
            (1700, 1700, crate::app::test_support::clipboard_noise(1700, 1700), false),
            (2000, 1400, [12, 34, 56, 128].repeat(2000 * 1400), true),
            (1500, 1500, crate::app::test_support::clipboard_noise(1500, 1500), true),
        ] {
            let fixture = image::RgbaImage::from_raw(width, height, rgba.clone()).unwrap();
            let mut png = Vec::new();
            fixture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
            if !accepted {
                assert!(png.len() > LIMIT);
            } else if width == 2000 {
                assert!(rgba.len() > LIMIT);
                assert!(png.len() < LIMIT);
            } else {
                assert!(png.len() < LIMIT);
                assert!(base64::engine::general_purpose::STANDARD.encode(&png).len() > LIMIT);
            }
            let result = encode_clipboard_image(clipboard_pixels(
                width as usize,
                height as usize,
                rgba.clone(),
            ));
            if accepted {
                let attachment = result.expect("PNG within byte limit");
                assert_eq!(attachment.mime_type, "image/png");
                let bytes =
                    base64::engine::general_purpose::STANDARD.decode(&attachment.data).unwrap();
                let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                    .unwrap()
                    .into_rgba8();
                assert_eq!((decoded.width(), decoded.height()), (width, height));
                assert_eq!(decoded.into_raw(), rgba);
            } else {
                assert_eq!(result, Err(ClipboardImageError::TooLarge));
            }
        }
    }
}
