pub mod coalesce;
pub mod discord;
pub mod slack;
pub mod telegram;

use async_trait::async_trait;
use tokio::sync::mpsc;

/// An incoming message from any channel.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub channel: String,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub session_id: String,
    pub content: String,
    pub reply_to: Option<String>,
    pub timestamp: u64,
    /// If set, route this message directly to a named worker instead of the main conductor.
    pub worker_hint: Option<String>,
}

/// An outgoing message to send back through a channel.
#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    pub channel: String,
    pub session_id: String,
    pub content: String,
    pub reply_to: Option<String>,
}

/// Channel adapter trait. Implement for each messaging platform.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Start listening for messages. Incoming messages are sent to `tx`.
    /// This should spawn background tasks and return immediately.
    async fn start(&self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Result<(), anyhow::Error>;

    /// Send a message through this channel.
    async fn send(&self, msg: OutgoingMessage) -> Result<(), anyhow::Error>;

    /// Channel name (e.g. "telegram", "discord").
    fn name(&self) -> &str;
}

/// Split a message into chunks at newline boundaries, respecting max length.
pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_len).min(text.len());
        // Ensure we don't split in the middle of a UTF-8 character
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        let split_at = if end < text.len() {
            // Try to split at a newline
            text[start..end]
                .rfind('\n')
                .map(|p| start + p + 1)
                .unwrap_or(end)
        } else {
            end
        };
        chunks.push(text[start..split_at].to_string());
        start = split_at;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_short_message() {
        let chunks = split_message("hello", 4096);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn test_split_long_message() {
        let text = "line1\nline2\nline3\nline4";
        let chunks = split_message(text, 12);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "line1\nline2\n");
        assert_eq!(chunks[1], "line3\nline4");
    }

    #[test]
    fn test_split_multibyte_chars() {
        // Each emoji is 4 bytes; this tests that we don't panic on multi-byte boundaries
        let text = "Hello 🌍🌎🌏 World";
        let chunks = split_message(text, 10);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks.join(""), text);
    }

    #[test]
    fn test_split_no_newlines() {
        let text = "a".repeat(100);
        let chunks = split_message(&text, 40);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 40);
        assert_eq!(chunks[1].len(), 40);
        assert_eq!(chunks[2].len(), 20);
    }
}
