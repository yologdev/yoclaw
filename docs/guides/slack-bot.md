# Slack Bot Setup

Complete walkthrough for connecting yoclaw to Slack via Socket Mode.

## 1. Create a Slack app

1. Go to [api.slack.com/apps](https://api.slack.com/apps)
2. Click **Create New App** → **From scratch**
3. Name your app and select your workspace

## 2. Enable Socket Mode

Socket Mode lets your bot connect via WebSocket — no public URL needed.

1. Go to **Settings** → **Socket Mode** in the left sidebar
2. Enable Socket Mode
3. Generate an app-level token with scope `connections:write`
4. Copy the token (starts with `xapp-`)

## 3. Configure event subscriptions

1. Go to **Features** → **Event Subscriptions**
2. Enable Events
3. Under **Subscribe to bot events**, add:
   - `message.channels` — Messages in public channels
   - `message.groups` — Messages in private channels
   - `message.im` — Direct messages
   - `message.mpim` — Group direct messages
4. Save changes

## 4. Set OAuth scopes

1. Go to **Features** → **OAuth & Permissions**
2. Under **Bot Token Scopes**, add:
   - `chat:write` — Send messages
   - `channels:history` — Read channel messages
   - `groups:history` — Read private channel messages
   - `im:history` — Read DMs
   - `im:read` — View DM info
   - `im:write` — Start DMs
   - `channels:read` — View channel info
   - `groups:read` — View private channel info

## 5. Install to workspace

1. Go to **Settings** → **Install App**
2. Click **Install to Workspace**
3. Authorize the permissions
4. Copy the Bot User OAuth Token (starts with `xoxb-`)

> If you modify scopes later, you must **reinstall** the app to your workspace for changes to take effect.

## 6. Configure yoclaw

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[channels.slack]
bot_token = "${SLACK_BOT_TOKEN}"       # xoxb-...
app_token = "${SLACK_APP_TOKEN}"       # xapp-...
allowed_channels = []                   # Empty = all channels
allowed_users = []                      # Empty = all users
debounce_ms = 2000
```

### Access control

- `allowed_channels` — Channel names (not IDs) the bot responds in. Empty = all.
- `allowed_users` — Slack user IDs (e.g., `U12345678`). Empty = all.

## 7. Run

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export SLACK_BOT_TOKEN="xoxb-..."
export SLACK_APP_TOKEN="xapp-..."
yoclaw
```

## Session IDs

Slack sessions vary by context:

| Context | Session ID format |
|---------|------------------|
| Channel message | `slack-C03947L0E` |
| Thread reply | `slack-C03947L0E-1772142005.877839` |
| Direct message | `slack-D04AB12CD` |

Thread replies get their own session ID, so the agent maintains separate conversation context per thread.

## Direct messages

For DM support:

1. In your Slack app settings, go to **Features** → **App Home**
2. Enable the **Messages Tab**
3. Ensure `im:history`, `im:read`, and `im:write` scopes are added
4. **Reinstall** the app to your workspace

> DMs may be blocked by workspace admin policy ("Sending messages to this app has been turned off"). Ask your workspace admin to allow it.

## Thread support

When a user replies in a thread, yoclaw creates a separate session for that thread. This means:

- Main channel messages have one conversation context
- Each thread has its own independent conversation
- The agent can maintain different contexts for different discussions simultaneously

## Message limits

Slack limits messages to approximately 4,000 characters. yoclaw splits longer responses automatically.

## Troubleshooting

### Bot doesn't respond

1. Verify Socket Mode is enabled
2. Check that the app-level token has `connections:write` scope
3. Ensure event subscriptions include the relevant `message.*` events
4. Check that the app is installed to the workspace
5. Enable debug logging: `RUST_LOG=yoclaw=debug yoclaw`

### "Not connected" errors

The `xapp-` token (app-level token) may be invalid or expired. Generate a new one in Socket Mode settings.

### DMs don't work

1. Check Messages Tab is enabled in App Home
2. Verify `im:history`, `im:read`, `im:write` scopes are added
3. Reinstall the app after scope changes
4. Check if workspace admin has blocked DMs to apps
