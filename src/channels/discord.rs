use super::{split_message, ChannelAdapter, IncomingMessage, OutgoingMessage, SentMessage};
use crate::config::DiscordConfig;
use crate::db::now_ms;
use async_trait::async_trait;
use serenity::all::{
    ChannelId, Context, CreateMessage, EditMessage, EventHandler, GatewayIntents, Message,
    MessageId, Ready,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Discord channel adapter using serenity.
pub struct DiscordAdapter {
    config: DiscordConfig,
    http: Arc<RwLock<Option<Arc<serenity::http::Http>>>>,
}

impl DiscordAdapter {
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            http: Arc::new(RwLock::new(None)),
        }
    }
}

struct Handler {
    tx: mpsc::UnboundedSender<IncomingMessage>,
    allowed_guilds: Vec<u64>,
    allowed_users: Vec<u64>,
    routing: HashMap<String, String>, // channel_name → worker_name
    http_store: Arc<RwLock<Option<Arc<serenity::http::Http>>>>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore bot messages
        if msg.author.bot {
            return;
        }

        // Guild filtering
        if let Some(guild_id) = msg.guild_id {
            if !self.allowed_guilds.is_empty() && !self.allowed_guilds.contains(&guild_id.get()) {
                return;
            }
        }

        // User filtering
        if !self.allowed_users.is_empty() && !self.allowed_users.contains(&msg.author.id.get()) {
            return;
        }

        let content = msg.content.clone();
        if content.is_empty() {
            return;
        }

        let channel_id = msg.channel_id;

        // Determine worker hint from routing config
        let worker_hint = self.resolve_routing(&ctx, channel_id).await;

        let incoming = IncomingMessage {
            channel: "discord".into(),
            sender_id: msg.author.id.get().to_string(),
            sender_name: Some(msg.author.name.clone()),
            session_id: format!("dc-{}", channel_id.get()),
            content,
            reply_to: msg
                .referenced_message
                .as_ref()
                .map(|m| m.id.get().to_string()),
            timestamp: now_ms(),
            worker_hint,
            is_group: msg.guild_id.is_some(),
        };

        let _ = self.tx.send(incoming);
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!("Discord bot connected as {}", ready.user.name);
        let mut http = self.http_store.write().await;
        *http = Some(ctx.http.clone());
    }
}

impl Handler {
    async fn resolve_routing(&self, ctx: &Context, channel_id: ChannelId) -> Option<String> {
        if self.routing.is_empty() {
            return None;
        }

        // Try to get channel name for routing lookup
        let channel_name = match ctx.http.get_channel(channel_id).await {
            Ok(channel) => match channel.guild() {
                Some(gc) => Some(gc.name.clone()),
                None => None,
            },
            Err(_) => None,
        };

        channel_name.and_then(|name| self.routing.get(&name).cloned())
    }
}

#[async_trait]
impl ChannelAdapter for DiscordAdapter {
    async fn start(&self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Result<(), anyhow::Error> {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT
            | GatewayIntents::DIRECT_MESSAGES;

        let routing: HashMap<String, String> = self
            .config
            .routing
            .iter()
            .map(|(k, v)| (k.clone(), v.worker.clone()))
            .collect();

        let handler = Handler {
            tx,
            allowed_guilds: self.config.allowed_guilds.clone(),
            allowed_users: self.config.allowed_users.clone(),
            routing,
            http_store: self.http.clone(),
        };

        let mut client = serenity::Client::builder(&self.config.bot_token, intents)
            .event_handler(handler)
            .await?;

        tokio::spawn(async move {
            if let Err(e) = client.start().await {
                tracing::error!("Discord client error: {}", e);
            }
        });

        tracing::info!("Discord adapter started");
        Ok(())
    }

    async fn send(&self, msg: OutgoingMessage) -> Result<(), anyhow::Error> {
        let channel_id: u64 = msg
            .session_id
            .strip_prefix("dc-")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("Invalid discord session_id: {}", msg.session_id))?;

        let http = self.http.read().await;
        let http = http
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Discord HTTP client not ready"))?;

        let chunks = split_message(&msg.content, 2000);
        for chunk in chunks {
            let builder = CreateMessage::new().content(&chunk);
            ChannelId::new(channel_id)
                .send_message(http.as_ref(), builder)
                .await?;
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "discord"
    }

    async fn send_placeholder(&self, session_id: &str, text: &str) -> Option<SentMessage> {
        let channel_id: u64 = session_id
            .strip_prefix("dc-")
            .and_then(|s| s.parse().ok())?;
        let http = self.http.read().await;
        let http = http.as_ref()?;
        let builder = CreateMessage::new().content(text);
        match ChannelId::new(channel_id)
            .send_message(http.as_ref(), builder)
            .await
        {
            Ok(msg) => Some(SentMessage {
                channel: "discord".into(),
                session_id: session_id.to_string(),
                message_id: msg.id.get().to_string(),
            }),
            Err(e) => {
                tracing::warn!("Failed to send Discord placeholder: {}", e);
                None
            }
        }
    }

    async fn edit_message(
        &self,
        handle: &SentMessage,
        new_text: &str,
    ) -> Result<(), anyhow::Error> {
        let channel_id: u64 = handle
            .session_id
            .strip_prefix("dc-")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("Invalid discord session_id"))?;
        let message_id: u64 = handle
            .message_id
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid discord message_id"))?;
        let http = self.http.read().await;
        let http = http
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Discord HTTP client not ready"))?;

        // Discord max message length is 2000 — truncate if needed
        let text = if new_text.len() > 2000 {
            &new_text[..2000]
        } else {
            new_text
        };

        let builder = EditMessage::new().content(text);
        ChannelId::new(channel_id)
            .edit_message(http.as_ref(), MessageId::new(message_id), builder)
            .await?;
        Ok(())
    }
}

/// Parse a Discord session_id back to a channel_id.
pub fn parse_discord_session(session_id: &str) -> Option<u64> {
    session_id.strip_prefix("dc-").and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_discord_session() {
        assert_eq!(parse_discord_session("dc-123456789"), Some(123456789));
        assert_eq!(parse_discord_session("tg-123"), None);
        assert_eq!(parse_discord_session("dc-abc"), None);
        assert_eq!(parse_discord_session(""), None);
    }

    #[test]
    fn test_discord_message_split() {
        let text = "a".repeat(5000);
        let chunks = split_message(&text, 2000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 2000);
        assert_eq!(chunks[2].len(), 1000);
    }

    #[test]
    fn test_discord_short_message() {
        let chunks = split_message("hello discord", 2000);
        assert_eq!(chunks, vec!["hello discord"]);
    }
}
