use super::{split_message, ChannelAdapter, IncomingMessage, OutgoingMessage};
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

                        let is_group = msg.chat.is_group() || msg.chat.is_supergroup();
                        let incoming = IncomingMessage {
                            channel: "telegram".into(),
                            sender_id: sender_id.to_string(),
                            sender_name: msg.from.as_ref().map(|u| u.first_name.clone()),
                            session_id: format!("tg-{}", msg.chat.id.0),
                            content: text,
                            reply_to: msg.reply_to_message().map(|m| m.id.0.to_string()),
                            timestamp: now_ms(),
                            worker_hint: None,
                            is_group,
                        };

                        let _ = tx.send(incoming);
                        respond(())
                    }
                },
            );

            Dispatcher::builder(bot, handler).build().dispatch().await;
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
            self.bot.send_message(ChatId(chat_id), &chunk).await?;
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "telegram"
    }

    fn start_typing(&self, session_id: &str) -> Option<tokio::task::JoinHandle<()>> {
        let chat_id: i64 = session_id
            .strip_prefix("tg-")
            .and_then(|s| s.parse().ok())?;
        let bot = self.bot.clone();
        Some(tokio::spawn(async move {
            loop {
                let _ = bot
                    .send_chat_action(ChatId(chat_id), teloxide::types::ChatAction::Typing)
                    .await;
                // Telegram typing indicator lasts ~5s, refresh every 4s
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            }
        }))
    }
}
