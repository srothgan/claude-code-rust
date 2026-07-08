// SPDX-License-Identifier: Apache-2.0
// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
}

impl TextContent {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageContent {
    pub data: String,
    pub mime_type: String,
}

impl ImageContent {
    #[must_use]
    pub fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self { data: data.into(), mime_type: mime_type.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentBlock {
    Text(TextContent),
    Image(ImageContent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub content: ContentBlock,
}

impl Content {
    #[must_use]
    pub fn new(content: ContentBlock) -> Self {
        Self { content }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentChunk {
    pub content: ContentBlock,
    pub source_message_uuid: Option<String>,
}

impl ContentChunk {
    #[must_use]
    pub fn new(content: ContentBlock) -> Self {
        Self { content, source_message_uuid: None }
    }

    #[must_use]
    pub fn source_message_uuid(mut self, source_message_uuid: Option<String>) -> Self {
        self.source_message_uuid = source_message_uuid.filter(|uuid| !uuid.trim().is_empty());
        self
    }
}
