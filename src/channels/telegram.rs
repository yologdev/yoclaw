use super::{ChannelAdapter, IncomingMessage, OutgoingMessage};
use crate::config::TelegramConfig;
use crate::db::now_ms;
use async_trait::async_trait;
use teloxide::prelude::*;
use tokio::sync::mpsc;

/// Telegram channel adapter using teloxide.
pub struct TelegramAdapter {
    bot: Bot,
    config: TelegramConfig,
}

impl TelegramAdapter {
    pub fn new(config: TelegramConfig) -> Self {
        let bot = Bot::new(&config.bot_token);
        Self { bot, config }
    }
}

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    async fn start(&self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Result<(), anyhow::Error> {
        let bot = self.bot.clone();
        let allowed = self.config.allowed_senders.clone();

        tokio::spawn(async move {
            let handler = Update::filter_message().endpoint(
                move |msg: teloxide::types::Message, _bot: Bot| {
                    let tx = tx.clone();
                    let allowed = allowed.clone();
                    async move {
                        // Sender allowlist
                        let sender_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
                        if !allowed.is_empty() && !allowed.contains(&sender_id) {
                            return respond(());
                        }

                        let text = msg.text().unwrap_or("").to_string();
                        if text.is_empty() {
                            return respond(());
                        }

                        let incoming = IncomingMessage {
                            channel: "telegram".into(),
                            sender_id: sender_id.to_string(),
                            sender_name: msg
                                .from
                                .as_ref()
                                .map(|u| u.first_name.clone()),
                            session_id: format!("tg-{}", msg.chat.id.0),
                            content: text,
                            reply_to: msg
                                .reply_to_message()
                                .map(|m| m.id.0.to_string()),
                            timestamp: now_ms(),
                        };

                        let _ = tx.send(incoming);
                        respond(())
                    }
                },
            );

            Dispatcher::builder(bot, handler)
                .build()
                .dispatch()
                .await;
        });

        tracing::info!("Telegram adapter started");
        Ok(())
    }

    async fn send(&self, msg: OutgoingMessage) -> Result<(), anyhow::Error> {
        let chat_id: i64 = msg
            .session_id
            .strip_prefix("tg-")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("Invalid telegram session_id: {}", msg.session_id))?;

        let chunks = split_message(&msg.content, 4096);
        for chunk in chunks {
            self.bot
                .send_message(ChatId(chat_id), &chunk)
                .await?;
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "telegram"
    }
}

/// Split a message into chunks at newline boundaries, respecting max length.
pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + max_len).min(text.len());
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
    fn test_split_no_newlines() {
        let text = "a".repeat(100);
        let chunks = split_message(&text, 40);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 40);
        assert_eq!(chunks[1].len(), 40);
        assert_eq!(chunks[2].len(), 20);
    }
}
