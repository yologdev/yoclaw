pub mod coalesce;
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
