use super::{split_message, ChannelAdapter, IncomingMessage, OutgoingMessage};
use crate::config::SlackConfig;
use crate::db::now_ms;
use async_trait::async_trait;
use slack_morphism::prelude::*;
use slack_morphism_hyper::*;
use std::sync::Arc;
use tokio::sync::mpsc;

/// State stored in SlackClientEventsUserState for the push events callback.
struct SlackAdapterState {
    tx: mpsc::UnboundedSender<IncomingMessage>,
    allowed_channels: Vec<String>,
    allowed_users: Vec<String>,
}

/// Slack channel adapter using slack-morphism with Socket Mode.
pub struct SlackAdapter {
    config: SlackConfig,
    client: Arc<SlackClient<SlackClientHyperHttpsConnector>>,
    bot_token: SlackApiToken,
}

impl SlackAdapter {
    pub fn new(config: SlackConfig) -> Self {
        let connector = SlackClientHyperConnector::new();
        let client = Arc::new(SlackClient::new(connector));
        let bot_token = SlackApiToken::new(SlackApiTokenValue(config.bot_token.clone()));
        Self {
            config,
            client,
            bot_token,
        }
    }
}

async fn push_events_handler(
    event: SlackPushEventCallback,
    _client: Arc<SlackClient<SlackClientHyperHttpsConnector>>,
    states: SlackClientEventsUserState,
) -> UserCallbackResult<()> {
    let states_r = states.read().await;
    let state = states_r.get_user_state::<Arc<SlackAdapterState>>().cloned();
    drop(states_r);

    if let Some(state) = state {
        handle_push_event(
            event,
            &state.tx,
            &state.allowed_channels,
            &state.allowed_users,
        );
    }
    Ok(())
}

fn error_handler(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackClient<SlackClientHyperHttpsConnector>>,
    _state: SlackClientEventsUserState,
) -> http::StatusCode {
    tracing::error!("Slack Socket Mode error: {:?}", err);
    http::StatusCode::OK
}

#[async_trait]
impl ChannelAdapter for SlackAdapter {
    async fn start(&self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Result<(), anyhow::Error> {
        let app_token = SlackApiToken::new(SlackApiTokenValue(self.config.app_token.clone()));

        let adapter_state = Arc::new(SlackAdapterState {
            tx,
            allowed_channels: self.config.allowed_channels.clone(),
            allowed_users: self.config.allowed_users.clone(),
        });

        let socket_mode_config = SlackClientSocketModeConfig::new().with_max_connections_count(2);

        let listener_env = Arc::new(
            SlackClientEventsListenerEnvironment::new(self.client.clone())
                .with_error_handler(error_handler)
                .with_user_state(adapter_state),
        );

        let callbacks =
            SlackSocketModeListenerCallbacks::new().with_push_events(push_events_handler);

        let listener =
            SlackClientSocketModeListener::new(&socket_mode_config, listener_env, callbacks);
        listener.listen_for(&app_token).await?;

        tokio::spawn(async move {
            listener.serve().await;
        });

        tracing::info!("Slack adapter started (Socket Mode)");
        Ok(())
    }

    async fn send(&self, msg: OutgoingMessage) -> Result<(), anyhow::Error> {
        let (channel_id, thread_ts) = parse_slack_session(&msg.session_id)
            .ok_or_else(|| anyhow::anyhow!("Invalid slack session_id: {}", msg.session_id))?;

        let session = self.client.open_session(&self.bot_token);
        let chunks = split_message(&msg.content, 4000);

        for chunk in chunks {
            let content = SlackMessageContent::new().with_text(chunk);
            let mut request =
                SlackApiChatPostMessageRequest::new(SlackChannelId(channel_id.clone()), content);

            if let Some(ref ts) = thread_ts {
                request = request.with_thread_ts(SlackTs(ts.clone()));
            }

            session.chat_post_message(&request).await?;
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "slack"
    }
}

fn handle_push_event(
    event: SlackPushEventCallback,
    tx: &mpsc::UnboundedSender<IncomingMessage>,
    allowed_channels: &[String],
    allowed_users: &[String],
) {
    let SlackPushEventCallback { event: inner, .. } = event;

    if let SlackEventCallbackBody::Message(msg_event) = inner {
        // Skip bot messages
        if msg_event.subtype.is_some() {
            return;
        }
        if msg_event.sender.bot_id.is_some() {
            return;
        }

        let sender_id = match &msg_event.sender.user {
            Some(user) => user.0.clone(),
            None => return,
        };

        // User filtering
        if !allowed_users.is_empty() && !allowed_users.contains(&sender_id) {
            return;
        }

        let channel_id = match &msg_event.origin.channel {
            Some(ch) => ch.0.clone(),
            None => return,
        };

        // Channel filtering
        if !allowed_channels.is_empty() && !allowed_channels.contains(&channel_id) {
            return;
        }

        let text = match &msg_event.content {
            Some(content) => match &content.text {
                Some(t) => t.clone(),
                None => return,
            },
            None => return,
        };

        if text.is_empty() {
            return;
        }

        // Build session_id: thread-aware
        let thread_ts = msg_event.origin.thread_ts.as_ref().map(|ts| ts.0.clone());
        let session_id = match &thread_ts {
            Some(ts) => format!("slack-{}-{}", channel_id, ts),
            None => format!("slack-{}", channel_id),
        };

        let incoming = IncomingMessage {
            channel: "slack".into(),
            sender_id,
            sender_name: None,
            session_id,
            content: text,
            reply_to: thread_ts,
            timestamp: now_ms(),
            worker_hint: None,
        };

        let _ = tx.send(incoming);
    }
}

/// Parse a Slack session_id back to (channel_id, optional thread_ts).
pub fn parse_slack_session(session_id: &str) -> Option<(String, Option<String>)> {
    let rest = session_id.strip_prefix("slack-")?;
    // If there's a thread_ts, format is "slack-{channel}-{ts}"
    // Thread ts looks like "1234567890.123456"
    // Channel IDs are alphanumeric like "C0123456789"
    if let Some(dash_pos) = rest.find('-') {
        let channel = &rest[..dash_pos];
        let thread_ts = &rest[dash_pos + 1..];
        if thread_ts.contains('.') {
            Some((channel.to_string(), Some(thread_ts.to_string())))
        } else {
            // Not a thread_ts, treat whole thing as channel
            Some((rest.to_string(), None))
        }
    } else {
        Some((rest.to_string(), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slack_session_channel() {
        let (ch, ts) = parse_slack_session("slack-C0123456789").unwrap();
        assert_eq!(ch, "C0123456789");
        assert_eq!(ts, None);
    }

    #[test]
    fn test_parse_slack_session_thread() {
        let (ch, ts) = parse_slack_session("slack-C0123456789-1234567890.123456").unwrap();
        assert_eq!(ch, "C0123456789");
        assert_eq!(ts, Some("1234567890.123456".to_string()));
    }

    #[test]
    fn test_parse_slack_session_invalid() {
        assert_eq!(parse_slack_session("tg-123"), None);
        assert_eq!(parse_slack_session("dc-123"), None);
        assert_eq!(parse_slack_session(""), None);
    }

    #[test]
    fn test_slack_message_split() {
        let text = "a".repeat(10000);
        let chunks = split_message(&text, 4000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 4000);
        assert_eq!(chunks[1].len(), 4000);
        assert_eq!(chunks[2].len(), 2000);
    }
}
