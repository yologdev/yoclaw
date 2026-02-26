# Channels

Channels are the messaging platforms yoclaw connects to. Each channel implements the `ChannelAdapter` trait:

```rust
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    async fn start(&self, tx: UnboundedSender<IncomingMessage>) -> Result<()>;
    async fn send(&self, msg: OutgoingMessage) -> Result<()>;
    fn name(&self) -> &str;
    fn start_typing(&self, session_id: &str) -> Option<JoinHandle<()>>;
}
```

yoclaw ships with three adapters: Telegram, Discord, and Slack. All three can run simultaneously.

## Telegram

Uses the [teloxide](https://github.com/teloxide/teloxide) library. Connects via Telegram's long-polling API.

```toml
[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = [514133400]    # Telegram user IDs (empty = allow all)
debounce_ms = 2000
```

- **Session IDs**: `tg-{chat_id}` (e.g., `tg-514133400`)
- **Typing indicator**: Shows "typing..." while processing
- **Message splitting**: Long responses are split at newline boundaries (max 4096 chars per message)
- **Group chats**: Supported — responds when mentioned or replied to

See [Telegram Bot Guide](../guides/telegram-bot.md) for full setup.

## Discord

Uses the [serenity](https://github.com/serenity-rs/serenity) library. Connects via Discord's Gateway WebSocket.

```toml
[channels.discord]
bot_token = "${DISCORD_BOT_TOKEN}"
allowed_guilds = [398994758158254080]    # Discord server IDs
allowed_users = []                        # User IDs (empty = allow all in allowed guilds)
debounce_ms = 2000

# Optional: route channels to specific workers
[channels.discord.routing.coding-help]
worker = "coding"

[channels.discord.routing.research]
worker = "research"
```

- **Session IDs**: `dc-{channel_id}` (e.g., `dc-1234567890`)
- **Message Content Intent**: Must be enabled in the Discord Developer Portal
- **Channel routing**: Messages in specific channels can be routed to named workers
- **Guild allowlist**: Set at startup, requires restart to change

See [Discord Bot Guide](../guides/discord-bot.md) for full setup.

## Slack

Uses [slack-morphism](https://github.com/abdolence/slack-morphism-rust) in Socket Mode. Requires both an app-level token and a bot token.

```toml
[channels.slack]
bot_token = "${SLACK_BOT_TOKEN}"       # xoxb-...
app_token = "${SLACK_APP_TOKEN}"       # xapp-...
allowed_channels = ["general"]
allowed_users = ["U12345"]
debounce_ms = 2000
```

- **Session IDs**: `slack-{channel}` or `slack-{channel}-{thread_ts}` for threads
- **Socket Mode**: No public URL needed — connects outbound via WebSocket
- **Thread support**: Thread replies get their own session, maintaining conversation context
- **DMs**: Require the Messages Tab enabled plus `im:history`, `im:read`, `im:write` scopes

See [Slack Bot Guide](../guides/slack-bot.md) for full setup.

## Message flow

All channels share the same message flow:

```
IncomingMessage {
    channel: "telegram",              # Adapter name
    sender_id: "514133400",           # Platform user ID
    sender_name: Some("Yuanhao"),     # Display name
    session_id: "tg-514133400",       # Unique session key
    content: "Hello!",                # Message text
    worker_hint: None,                # Optional worker routing
    is_group: false,                  # Group vs DM
}
```

Responses flow back through the same adapter:

```
OutgoingMessage {
    channel: "telegram",              # Must match adapter.name()
    session_id: "tg-514133400",       # Routing info (chat_id, channel_id, etc.)
    content: "Hi! How can I help?",   # Response text
    reply_to: None,                   # Optional reply reference
}
```

## Debouncing

Each channel has an independent debounce timer. When multiple messages arrive within the debounce window, they're concatenated with newlines and processed as a single message.

The debounce duration is configurable per channel and is hot-reloadable — you can change it without restarting yoclaw.

## Long messages

yoclaw automatically splits long responses to respect platform limits:

| Platform | Max message length |
|----------|-------------------|
| Telegram | 4,096 characters |
| Discord | 2,000 characters |
| Slack | 4,000 characters |

Splitting happens at newline boundaries when possible, with UTF-8 character boundary safety to avoid panicking on multi-byte characters (emoji, CJK, etc.).
