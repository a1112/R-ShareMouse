//! Clipboard data structures and management

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// MIME types for clipboard content
pub mod mime {
    pub const TEXT_PLAIN: &str = "text/plain";
    pub const TEXT_HTML: &str = "text/html";
    pub const TEXT_URI_LIST: &str = "text/uri-list";
    pub const IMAGE_PNG: &str = "image/png";
    pub const IMAGE_JPEG: &str = "image/jpeg";
    pub const IMAGE_BMP: &str = "image/bmp";
    pub const APPLICATION_OCTET_STREAM: &str = "application/octet-stream";
}

/// Clipboard content type category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Image,
    Other,
}

/// Clipboard content
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClipboardContent {
    Text(String),
    Html { html: String, text: String },
    Image {
        format: ImageFormat,
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
    FileList(Vec<String>),
    Other { mime: String, data: Vec<u8> },
    Empty,
}

/// Image format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Bmp,
}

impl ImageFormat {
    pub fn mime_type(&self) -> &'static str {
        match self {
            ImageFormat::Png => mime::IMAGE_PNG,
            ImageFormat::Jpeg => mime::IMAGE_JPEG,
            ImageFormat::Bmp => mime::IMAGE_BMP,
        }
    }

    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            mime::IMAGE_PNG => Some(ImageFormat::Png),
            mime::IMAGE_JPEG => Some(ImageFormat::Jpeg),
            mime::IMAGE_BMP => Some(ImageFormat::Bmp),
            _ => None,
        }
    }
}

impl ClipboardContent {
    /// Create text content
    pub fn from_text(s: String) -> Self {
        Self::Text(s)
    }

    /// Create empty content
    pub fn empty() -> Self {
        Self::Empty
    }

    /// Get the MIME type of this content
    pub fn mime_type(&self) -> String {
        match self {
            ClipboardContent::Text(_) => mime::TEXT_PLAIN.to_string(),
            ClipboardContent::Html { .. } => mime::TEXT_HTML.to_string(),
            ClipboardContent::Image { format, .. } => format.mime_type().to_string(),
            ClipboardContent::FileList(_) => mime::TEXT_URI_LIST.to_string(),
            ClipboardContent::Other { mime, .. } => mime.clone(),
            ClipboardContent::Empty => mime::TEXT_PLAIN.to_string(),
        }
    }

    /// Get the size in bytes
    pub fn size(&self) -> usize {
        match self {
            ClipboardContent::Text(s) => s.len(),
            ClipboardContent::Html { html, text } => html.len() + text.len(),
            ClipboardContent::Image { data, .. } => data.len(),
            ClipboardContent::FileList(files) => files.iter().map(|f| f.len()).sum::<usize>() + files.len(),
            ClipboardContent::Other { data, .. } => data.len(),
            ClipboardContent::Empty => 0,
        }
    }

    /// Check if content is empty
    pub fn is_empty(&self) -> bool {
        matches!(self, ClipboardContent::Empty)
    }

    /// Get text content if available
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ClipboardContent::Text(s) => Some(s.as_str()),
            ClipboardContent::Html { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    /// Get text content (alias for as_text)
    pub fn text(&self) -> Option<&str> {
        self.as_text()
    }

    /// Get the content type category
    pub fn content_type(&self) -> ContentType {
        match self {
            ClipboardContent::Text(_) | ClipboardContent::Html { .. } => ContentType::Text,
            ClipboardContent::Image { .. } => ContentType::Image,
            ClipboardContent::FileList(_) | ClipboardContent::Other { .. } | ClipboardContent::Empty => ContentType::Other,
        }
    }

    /// Convert to protocol message format
    pub fn to_message(self) -> crate::protocol::Message {
        match self {
            ClipboardContent::Text(s) => crate::protocol::Message::ClipboardData {
                mime_type: mime::TEXT_PLAIN.to_string(),
                data: s.into_bytes(),
            },
            ClipboardContent::Html { html, .. } => crate::protocol::Message::ClipboardData {
                mime_type: mime::TEXT_HTML.to_string(),
                data: html.into_bytes(),
            },
            ClipboardContent::Image { data, .. } => crate::protocol::Message::ClipboardData {
                mime_type: mime::IMAGE_PNG.to_string(),
                data,
            },
            ClipboardContent::FileList(files) => {
                let uri_list = files.join("\r\n");
                crate::protocol::Message::ClipboardData {
                    mime_type: mime::TEXT_URI_LIST.to_string(),
                    data: uri_list.into_bytes(),
                }
            }
            ClipboardContent::Other { mime, data } => crate::protocol::Message::ClipboardData {
                mime_type: mime,
                data,
            },
            ClipboardContent::Empty => crate::protocol::Message::ClipboardData {
                mime_type: mime::TEXT_PLAIN.to_string(),
                data: Vec::new(),
            },
        }
    }

    /// Create from protocol message
    pub fn from_message(mime_type: String, data: Vec<u8>) -> Self {
        match mime_type.as_str() {
            mime::TEXT_PLAIN => {
                let text = String::from_utf8(data).unwrap_or_default();
                if text.is_empty() {
                    ClipboardContent::Empty
                } else {
                    ClipboardContent::Text(text)
                }
            }
            mime::TEXT_HTML => {
                let html = String::from_utf8(data).unwrap_or_default();
                // Strip HTML tags to get plain text
                let text = strip_html_tags(&html);
                ClipboardContent::Html { html, text }
            }
            mime::IMAGE_PNG => ClipboardContent::Image {
                format: ImageFormat::Png,
                data,
                width: 0,
                height: 0,
            },
            mime::IMAGE_JPEG => ClipboardContent::Image {
                format: ImageFormat::Jpeg,
                data,
                width: 0,
                height: 0,
            },
            mime::IMAGE_BMP => ClipboardContent::Image {
                format: ImageFormat::Bmp,
                data,
                width: 0,
                height: 0,
            },
            mime::TEXT_URI_LIST => {
                let text = String::from_utf8(data).unwrap_or_default();
                let files = text
                    .lines()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                ClipboardContent::FileList(files)
            }
            _ => ClipboardContent::Other { mime: mime_type, data },
        }
    }
}

/// Simple HTML tag stripper (for converting HTML to plain text)
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut chars = html.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                // Add space after block tags
                if let Some(&next) = chars.peek() {
                    if next == '<' {
                        result.push(' ');
                    }
                }
            }
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }

    // Clean up multiple spaces
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Clipboard entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardEntry {
    pub content: ClipboardContent,
    pub timestamp: u64,
    pub source_device: Option<crate::protocol::DeviceId>,
}

impl ClipboardEntry {
    pub fn new(content: ClipboardContent) -> Self {
        Self {
            content,
            timestamp: timestamp_ms(),
            source_device: None,
        }
    }

    pub fn with_source(mut self, device_id: crate::protocol::DeviceId) -> Self {
        self.source_device = Some(device_id);
        self
    }
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Clipboard history for synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardHistory {
    entries: Vec<ClipboardEntry>,
    max_size: usize,
}

impl Default for ClipboardHistory {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            max_size: 100,
        }
    }
}

impl ClipboardHistory {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_size,
        }
    }

    /// Add an entry to history
    pub fn push(&mut self, entry: ClipboardEntry) {
        // Avoid duplicates
        if let Some(last) = self.entries.last() {
            if last.content.mime_type() == entry.content.mime_type()
                && last.content.size() == entry.content.size()
            {
                return;
            }
        }

        self.entries.push(entry);
        if self.entries.len() > self.max_size {
            self.entries.remove(0);
        }
    }

    /// Get the latest entry
    pub fn latest(&self) -> Option<&ClipboardEntry> {
        self.entries.last()
    }

    /// Clear history
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Get all entries
    pub fn entries(&self) -> &[ClipboardEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_text() {
        let content = ClipboardContent::from_text("Hello, World!".to_string());
        assert_eq!(content.mime_type(), mime::TEXT_PLAIN);
        assert_eq!(content.as_text(), Some("Hello, World!"));
        assert!(!content.is_empty());
    }

    #[test]
    fn test_clipboard_empty() {
        let content = ClipboardContent::empty();
        assert!(content.is_empty());
        assert_eq!(content.size(), 0);
    }

    #[test]
    fn test_image_format() {
        assert_eq!(ImageFormat::Png.mime_type(), mime::IMAGE_PNG);
        assert_eq!(ImageFormat::from_mime(mime::IMAGE_JPEG), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::from_mime("unknown"), None);
    }

    #[test]
    fn test_strip_html() {
        let html = "<p>Hello <b>World</b>!</p>";
        let text = strip_html_tags(html);
        assert_eq!(text, "Hello World!");
    }

    #[test]
    fn test_clipboard_history() {
        let mut history = ClipboardHistory::new(10);

        history.push(ClipboardEntry::new(ClipboardContent::from_text("First".to_string())));
        history.push(ClipboardEntry::new(ClipboardContent::from_text("Second".to_string())));

        assert_eq!(history.entries().len(), 2);
        assert_eq!(history.latest().unwrap().content.as_text(), Some("Second"));

        history.clear();
        assert_eq!(history.entries().len(), 0);
    }

    #[test]
    fn test_clipboard_size_limit() {
        let mut history = ClipboardHistory::new(3);

        // Use different lengths to avoid deduplication
        let texts = vec!["A", "BB", "CCC", "DDDD", "EEEEE"];
        for text in texts {
            history.push(ClipboardEntry::new(ClipboardContent::from_text(text.to_string())));
        }

        assert_eq!(history.entries().len(), 3);
        assert_eq!(history.entries()[0].content.as_text(), Some("CCC"));
        assert_eq!(history.entries()[2].content.as_text(), Some("EEEEE"));
    }
}
