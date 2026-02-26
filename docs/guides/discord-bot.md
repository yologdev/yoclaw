# Discord Bot Setup

Complete walkthrough for connecting yoclaw to Discord.

## 1. Create a Discord application

1. Go to the [Discord Developer Portal](https://discord.com/developers/applications)
2. Click **New Application**, give it a name
3. Go to the **Bot** section in the left sidebar
4. Click **Add Bot** (or it may already exist)
5. Copy the bot token

### Enable Message Content Intent

This is required for yoclaw to read message content:

1. In the **Bot** section, scroll down to **Privileged Gateway Intents**
2. Enable **Message Content Intent**
3. Save changes

Without this, the bot receives messages but the content field is empty.

## 2. Invite the bot to your server

1. Go to the **OAuth2** section, then **URL Generator**
2. Select scopes: `bot`
3. Select bot permissions:
   - Send Messages
   - Read Message History
   - Read Messages/View Channels
4. Copy the generated URL and open it in your browser
5. Select your server and authorize

## 3. Get your server and user IDs

Enable Developer Mode in Discord:
- Settings → Advanced → Developer Mode → On

Then right-click to copy IDs:
- Right-click your server name → Copy Server ID
- Right-click your username → Copy User ID

## 4. Configure yoclaw

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[channels.discord]
bot_token = "${DISCORD_BOT_TOKEN}"
allowed_guilds = [398994758158254080]   # Your server ID
allowed_users = []                       # Empty = all users in allowed guilds
debounce_ms = 2000
```

### Access control

- `allowed_guilds` — Which Discord servers the bot responds in. **Required.** Set at startup; requires restart to change.
- `allowed_users` — Which users can interact. Empty means all users in allowed guilds.

## 5. Run

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export DISCORD_BOT_TOKEN="MTIz..."
yoclaw
```

The bot appears online in your server and responds to messages in channels it can see.

## Channel routing

Route specific Discord channels to dedicated workers:

```toml
[channels.discord.routing.coding-help]
worker = "coding"

[channels.discord.routing.research]
worker = "research"

# Define the workers
[agent.workers.coding]
model = "claude-sonnet-4-20250514"
system_prompt = "You are a coding assistant. Write clean, tested code."
max_turns = 10

[agent.workers.research]
model = "claude-sonnet-4-20250514"
system_prompt = "You are a research assistant. Provide thorough, sourced answers."
max_turns = 15
```

Messages in `#coding-help` go directly to the `coding` worker. Messages in `#research` go to the `research` worker. All other channels use the main agent.

The channel name in the config must match the Discord channel name exactly (lowercase, hyphens for spaces).

## Session IDs

Discord sessions use the format `dc-{channel_id}`:

```
#general       → dc-1234567890123456
#coding-help   → dc-9876543210987654
```

Each channel has its own conversation history. DMs also get unique session IDs.

## Message limits

Discord limits messages to 2,000 characters. yoclaw splits longer responses automatically at newline boundaries.

## Typing indicator

yoclaw shows a "typing..." indicator in the Discord channel while processing messages.

## Troubleshooting

### Bot is online but doesn't respond

1. Check that **Message Content Intent** is enabled in the Developer Portal
2. Verify the server ID is in `allowed_guilds`
3. Check that the bot has permission to read and send messages in the channel
4. Enable debug logging: `RUST_LOG=yoclaw=debug yoclaw`

### Bot responds in some channels but not others

Channel routing may be sending messages to a worker that doesn't exist. Check that all `worker` names in routing rules match defined workers.

### "Privileged intent is not enabled" error

Go to Discord Developer Portal → Bot → Privileged Gateway Intents → Enable **Message Content Intent**.
