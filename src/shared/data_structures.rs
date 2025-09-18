use serde::{Deserialize, Serialize};
use indexmap::IndexMap;
use bytes::Bytes;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub item_id: u64,
    pub content_preview: String,
    pub content_type: ClipboardContentType,
    pub timestamp: u64, // Unix timestamp
    pub mime_data: IndexMap<String, Bytes>, // content type -> payload bytes
}

/// Lightweight version sent to the frontend in history listings (no payload bytes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItemPreview {
    pub item_id: u64,
    pub content_preview: String,
    pub content_type: ClipboardContentType,
    pub timestamp: u64, // Unix timestamp
}

impl From<&ClipboardItem> for ClipboardItemPreview {
    fn from(full: &ClipboardItem) -> Self {
        Self {
            item_id: full.item_id,
            content_preview: full.content_preview.clone(),
            content_type: full.content_type,
            timestamp: full.timestamp,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum ClipboardContentType {
    Text,
    Url,
    Code,
    Password,
    File,
    Image,
    Other,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub enum FrontendMessage {
    /// Request clipboard history
    GetHistory,
    /// Set clipboard content by ID
    SetClipboardById { id: u64 },
    /// Clear all clipboard history
    ClearHistory,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum BackendMessage {
    /// Response with clipboard history (previews only, no mime payloads)
    History { items: Vec<ClipboardItemPreview> },
    /// New clipboard item added (preview only)
    NewItem { item: ClipboardItemPreview },
    /// Clipboard content set successfully
    ClipboardSet,
    /// History cleared
    HistoryCleared,
    /// Error occurred
    Error { message: String },
}

impl ClipboardContentType {
    pub fn type_from_preview(content: &str) -> Self {
        const PASSWORD_SPECIALS: &str = "!@#$%^&*()-_=+[]{};:,.<>?/\\|`~";
        if content.starts_with("http://") || content.starts_with("https://") {
            Self::Url
        } else if content.contains("fn ") || content.contains("impl ") || content.contains("struct ") {
            Self::Code
        } else if content.contains('/') && !content.contains(' ') && content.len() < 256 {
            Self::File
        } else if !content.is_empty() && content.len() < 50 && !content.contains(' ') && content.chars().any(|c| PASSWORD_SPECIALS.contains(c)) {
            Self::Password
        } else {
            Self::Text
        }
    }

    // Return a static string representation of the content type (future multi-language support)
    pub const fn as_str(self) -> &'static str {
        match self {
            // Return capitalized labels directly so callers don't need to post-process
            Self::Text => "Text",
            Self::Url => "Url",
            Self::Code => "Code",
            Self::Password => "Password",
            Self::File => "File",
            Self::Image => "Image",
            Self::Other => "Other",
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Text => "📝",
            Self::Url => "🔗",
            Self::Code => "💻",
            Self::Password => "🔒",
            Self::File => "📁",
            Self::Image => "🖼️",
            Self::Other => "📄",
        }
    }
}
