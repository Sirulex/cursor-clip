use serde::{Deserialize, Serialize};
use indexmap::IndexMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItem {
    pub item_id: u64,
    pub content_preview: String,
    pub content_preview_type: ClipboardContentType,
    pub timestamp: u64, // Unix timestamp
    pub mime_data: IndexMap<String, Vec<u8>>, // kept internal / not sent in history
}

/// Lightweight version sent to the frontend in history listings (no payload bytes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardItemPreview {
    pub item_id: u64,
    pub content_preview: String,
    pub content_preview_type: ClipboardContentType,
    pub timestamp: u64, // Unix timestamp
}

impl From<&ClipboardItem> for ClipboardItemPreview {
    fn from(full: &ClipboardItem) -> Self {
        Self {
            item_id: full.item_id,
            content_preview: full.content_preview.clone(),
            content_preview_type: full.content_preview_type.clone(),
            timestamp: full.timestamp,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClipboardContentType {
    Text,
    Url,
    Code,
    Password,
    File,
    Image,
    Other,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum FrontendMessage {
    /// Request clipboard history
    GetHistory,
    /// Set clipboard content by ID
    SetClipboardById { id: u64 },
    /// Clear all clipboard history
    ClearHistory,
    /// Show frontend at coordinates
    ShowAt { x: f64, y: f64 },
    /// Close frontend
    Close,
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
    pub fn from_content(content: &str) -> Self {
        const PASSWORD_SPECIALS: &str = "!@#$%^&*()-_=+[]{};:,.<>?/\\|`~";
        if content.starts_with("http://") || content.starts_with("https://") {
            ClipboardContentType::Url
        } else if content.contains("fn ") || content.contains("impl ") || content.contains("struct ") {
            ClipboardContentType::Code
        } else if content.contains('/') && !content.contains(' ') && content.len() < 256 {
            ClipboardContentType::File
        } else if !content.is_empty() && content.len() < 50 && !content.contains(' ') && content.chars().any(|c| PASSWORD_SPECIALS.contains(c)) {
            ClipboardContentType::Password
        } else {
            ClipboardContentType::Text
        }
    }

    pub fn to_string(&self) -> &'static str {
        match self {
            ClipboardContentType::Text => "text",
            ClipboardContentType::Url => "url",
            ClipboardContentType::Code => "code",
            ClipboardContentType::Password => "password",
            ClipboardContentType::File => "file",
            ClipboardContentType::Image => "image",
            ClipboardContentType::Other => "other",
        }
    }

    pub fn get_icon(&self) -> &'static str {
        match self {
            ClipboardContentType::Text => "📝",
            ClipboardContentType::Url => "🔗",
            ClipboardContentType::Code => "💻",
            ClipboardContentType::Password => "🔒",
            ClipboardContentType::File => "📁",
            ClipboardContentType::Image => "🖼️",
            ClipboardContentType::Other => "📄",
        }
    }
}
