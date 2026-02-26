# Quick Start

Get a working AI agent in under 5 minutes.

## 1. Initialize

```bash
yoclaw init
```

This creates `~/.yoclaw/` with:

```
~/.yoclaw/
├── config.toml     # Main configuration
├── persona.md      # Agent personality
└── skills/         # Skill directory (empty)
```

## 2. Configure

Edit `~/.yoclaw/config.toml`:

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[agent.budget]
max_tokens_per_day = 1_000_000
max_turns_per_session = 50

[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = []
debounce_ms = 2000

[security]
shell_deny_patterns = ["rm -rf", "sudo", "chmod 777"]
```

The `${VARIABLE}` syntax reads from environment variables at startup. You can also paste values directly.

### Get your API key

Pick your LLM provider and get an API key:

| Provider | Where to get a key |
|----------|-------------------|
| Anthropic | [console.anthropic.com](https://console.anthropic.com) |
| OpenAI | [platform.openai.com](https://platform.openai.com) |
| Google | [aistudio.google.com](https://aistudio.google.com) |

### Get your bot token

For Telegram, talk to [@BotFather](https://t.me/BotFather) on Telegram:

1. Send `/newbot`
2. Choose a name and username
3. Copy the token

For other platforms, see the [Telegram](../guides/telegram-bot.md), [Discord](../guides/discord-bot.md), or [Slack](../guides/slack-bot.md) guides.

## 3. Set your persona

Edit `~/.yoclaw/persona.md` to define your agent's personality:

```markdown
You are Jarvis, a sharp and efficient personal assistant.

You remember previous conversations and proactively use your memory tools
to store important facts, preferences, and decisions.

When asked to do tasks, use the available tools. Be concise.
```

## 4. Run

```bash
# Set environment variables
export ANTHROPIC_API_KEY="sk-ant-..."
export TELEGRAM_BOT_TOKEN="123456:ABC..."

# Start yoclaw
yoclaw
```

You should see:

```
INFO yoclaw: Database: /Users/you/.yoclaw/yoclaw.db
INFO yoclaw: Conductor initialized
INFO yoclaw: yoclaw running. Waiting for messages...
```

## 5. Talk to your bot

Open Telegram, find your bot, and send a message. yoclaw will process it through the LLM and respond.

Try:

- "Remember that my favorite color is blue"
- "What tools do you have available?"
- "Search your memory for my preferences"

## Debug logging

If something isn't working, enable debug logs:

```bash
RUST_LOG=yoclaw=debug yoclaw
```

This shows detailed logs for message processing, tool calls, session switching, and more.

## What's next

- **Add more channels** — Connect [Discord](../guides/discord-bot.md) or [Slack](../guides/slack-bot.md)
- **Configure security** — Set up [tool permissions and budgets](../concepts/security.md)
- **Add workers** — Delegate tasks to [specialized sub-agents](../concepts/workers.md)
- **Create skills** — Teach your agent new abilities with [skill files](../concepts/skills.md)
- **Schedule tasks** — Set up [cron jobs](../concepts/scheduler.md) for periodic agent actions
