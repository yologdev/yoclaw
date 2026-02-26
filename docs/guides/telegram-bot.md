# Telegram Bot Setup

Complete walkthrough for connecting yoclaw to Telegram.

## 1. Create a bot with BotFather

Open Telegram and talk to [@BotFather](https://t.me/BotFather):

1. Send `/newbot`
2. Choose a display name (e.g., "My AI Assistant")
3. Choose a username ending in `bot` (e.g., `my_ai_assistant_bot`)
4. Copy the bot token — it looks like `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`

### Optional: configure your bot

While talking to BotFather, you can also:

- `/setdescription` — Bot description shown on profile
- `/setabouttext` — Short "About" text
- `/setuserpic` — Bot avatar
- `/setcommands` — Slash commands (yoclaw doesn't use these, but you can set them for UX)

## 2. Get your chat ID

You need your Telegram user ID to restrict who can talk to the bot. The easiest way:

1. Start your bot (send it `/start`)
2. Run yoclaw with debug logging:
   ```bash
   RUST_LOG=yoclaw=debug yoclaw
   ```
3. Send a message to your bot
4. Look for the log line:
   ```
   [telegram] YourName (tg-514133400): Hello
   ```
5. The number after `tg-` is your chat ID

## 3. Configure yoclaw

Edit `~/.yoclaw/config.toml`:

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = [514133400]       # Your chat ID
debounce_ms = 2000                  # 2 seconds
```

### allowed_senders

- Empty list `[]` — anyone can talk to your bot (use with caution)
- One or more IDs — only these users can interact with the bot

For a personal bot, always set your own ID.

## 4. Run

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export TELEGRAM_BOT_TOKEN="123456789:ABCdefGHIjklMNOpqrsTUVwxyz"
yoclaw
```

## Group chats

Your bot can participate in group chats:

1. Add the bot to a Telegram group
2. The bot responds when mentioned (`@my_ai_assistant_bot`) or replied to
3. Group messages use session ID `tg-{group_chat_id}` (negative number for groups)

### Group message handling

In group chats, yoclaw loads recent messages since the last assistant reply (up to `max_group_catchup_messages`, default 50). This gives the agent context about the ongoing conversation without loading the entire chat history.

```toml
[agent.context]
max_group_catchup_messages = 50
```

## Debouncing

The `debounce_ms` setting coalesces rapid messages. If you send three messages within 2 seconds, they're combined into one:

```
"Hey" + "can you" + "check the weather?" → "Hey\ncan you\ncheck the weather?"
```

Increase for mobile users who type slowly:

```toml
debounce_ms = 3000      # 3 seconds
```

Decrease for faster response times:

```toml
debounce_ms = 1000      # 1 second
```

## Typing indicator

yoclaw shows a "typing..." indicator in Telegram while processing messages. This is automatic and provides visual feedback that the bot is working.

## Message limits

Telegram limits messages to 4,096 characters. yoclaw automatically splits longer responses at newline boundaries, sending multiple messages in sequence.

## Running as a service

For persistent operation, use systemd (Linux) or launchd (macOS):

### systemd

Create `/etc/systemd/system/yoclaw.service`:

```ini
[Unit]
Description=yoclaw AI Agent
After=network.target

[Service]
Type=simple
User=youruser
Environment=ANTHROPIC_API_KEY=sk-ant-...
Environment=TELEGRAM_BOT_TOKEN=123456789:ABC...
ExecStart=/home/youruser/.cargo/bin/yoclaw
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable yoclaw
sudo systemctl start yoclaw
```

### launchd (macOS)

Create `~/Library/LaunchAgents/com.yoclaw.agent.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.yoclaw.agent</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.cargo/bin/yoclaw</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>ANTHROPIC_API_KEY</key>
        <string>sk-ant-...</string>
        <key>TELEGRAM_BOT_TOKEN</key>
        <string>123456789:ABC...</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/yoclaw.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/yoclaw.err</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.yoclaw.agent.plist
```
