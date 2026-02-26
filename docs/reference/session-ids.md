# Session IDs

Session IDs uniquely identify a conversation. Each session has its own conversation history stored in the tape table.

## Format by channel

| Channel | Format | Example |
|---------|--------|---------|
| Telegram (DM) | `tg-{chat_id}` | `tg-514133400` |
| Telegram (group) | `tg-{group_chat_id}` | `tg--1001234567890` |
| Discord | `dc-{channel_id}` | `dc-1234567890123456` |
| Slack (channel) | `slack-{channel_id}` | `slack-C03947L0E` |
| Slack (thread) | `slack-{channel_id}-{thread_ts}` | `slack-C03947L0E-1772142005.877839` |
| Cron job | `cron-{job_name}` | `cron-morning-briefing` |

## Where session IDs are used

### Conversation isolation

Each session ID maps to a separate conversation in the tape. When the Conductor switches sessions, it saves the current conversation and restores the target session's history.

### Queue routing

Incoming messages carry a session ID. The queue stores it alongside the message, and the Conductor uses it to determine which conversation to process.

### Cron delivery

Cron jobs use `target` to specify where to deliver results. The target must be a valid session ID:

```toml
[[scheduler.cron.jobs]]
target = "tg-514133400"     # Deliver to Telegram DM
```

The session ID prefix determines which channel adapter receives the response:

| Prefix | Adapter |
|--------|---------|
| `tg-` | Telegram |
| `dc-` | Discord |
| `slack-` | Slack |

### Audit filtering

The inspect command can filter by session:

```bash
yoclaw inspect --session tg-514133400
```

### Memory source

Memories stored by the agent include the session ID as the `source` field, so you can trace where a memory came from.

## Finding your session ID

### Telegram

Your chat ID is visible in debug logs:

```bash
RUST_LOG=yoclaw=debug yoclaw
```

Send a message and look for:

```
[telegram] YourName (tg-514133400): Hello
```

The number after `tg-` is your chat ID.

### Discord

Enable Developer Mode (Settings → Advanced → Developer Mode), then right-click a channel → Copy Channel ID. Prefix with `dc-`.

### Slack

Channel IDs are visible in Slack's URL when viewing a channel, or in the channel details panel. Prefix with `slack-`.

## Group chat vs DM sessions

- **Telegram DMs**: `tg-{your_user_id}` (positive number)
- **Telegram groups**: `tg-{group_id}` (typically a negative number)
- **Discord**: Always `dc-{channel_id}` — DMs and server channels both use channel IDs
- **Slack**: `slack-{channel_id}` — DMs use `D` prefix channels, regular channels use `C` prefix
