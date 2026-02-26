# Configuration Reference

Complete reference for `~/.yoclaw/config.toml`. All fields, defaults, and examples.

## Environment variable expansion

Any value can reference environment variables with `${VAR_NAME}`:

```toml
api_key = "${ANTHROPIC_API_KEY}"
```

If the variable is not set, yoclaw exits with an error at startup.

## Tilde expansion

Path values support `~` for the home directory:

```toml
db_path = "~/.yoclaw/yoclaw.db"
```

---

## `[agent]`

Core agent configuration.

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `provider` | string | `"anthropic"` | LLM provider name |
| `model` | string | **required** | Model ID (e.g., `"claude-sonnet-4-20250514"`) |
| `api_key` | string | **required** | API key for the provider |
| `persona` | string | `None` | Path to persona file (relative to config dir or absolute) |
| `skills_dirs` | string[] | `["~/.yoclaw/skills"]` | Directories to scan for skills |
| `max_tokens` | integer | provider default | Max tokens per LLM response |
| `thinking` | string | `None` | Thinking level: `"off"`, `"low"`, `"medium"`, `"high"` |

### Supported providers

| Provider value | Service |
|---------------|---------|
| `"anthropic"` | Anthropic (Claude) |
| `"openai"` | OpenAI (GPT) |
| `"google"` | Google AI (Gemini) |
| `"vertex"` | Vertex AI (Gemini via Google Cloud) |
| `"azure"` | Azure OpenAI |
| `"bedrock"` | AWS Bedrock |
| `"openai_responses"` | OpenAI Responses API |

### Example

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"
persona = "persona.md"
max_tokens = 8192
thinking = "medium"
skills_dirs = ["~/.yoclaw/skills", "~/work/skills"]
```

---

## `[agent.budget]`

Token and turn limits.

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `max_tokens_per_day` | integer | `None` (unlimited) | Daily token limit across all sessions |
| `max_turns_per_session` | integer | `None` (unlimited) | Max agent turns per message processing |

```toml
[agent.budget]
max_tokens_per_day = 1_000_000
max_turns_per_session = 50
```

---

## `[agent.context]`

Context window management.

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `max_context_tokens` | integer | `None` | Max context tokens before compaction |
| `keep_recent` | integer | `None` | Messages to keep during compaction |
| `tool_output_max_lines` | integer | `None` | Truncate tool output to this many lines |
| `max_group_catchup_messages` | integer | `50` | Max messages to load for group chat context |

```toml
[agent.context]
max_context_tokens = 180000
keep_recent = 4
tool_output_max_lines = 50
max_group_catchup_messages = 50
```

---

## `[agent.workers]`

Worker sub-agent configuration. See [Workers](../concepts/workers.md) for details.

### Default settings

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `provider` | string | main agent's provider | Default provider for workers |
| `model` | string | main agent's model | Default model for workers |
| `max_tokens` | integer | `None` | Default max tokens for workers |

### Named workers

```toml
[agent.workers.research]
provider = "anthropic"          # Optional: override default
model = "claude-sonnet-4-20250514"
api_key = "${OPENAI_API_KEY}"   # Optional: override main key
system_prompt = "You are a research assistant."
max_tokens = 4096
max_turns = 15
```

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `provider` | string | workers default | LLM provider |
| `model` | string | workers default | Model ID |
| `api_key` | string | main agent's key | API key |
| `system_prompt` | string | `None` | Worker's system prompt |
| `max_tokens` | integer | workers default | Max tokens per response |
| `max_turns` | integer | `None` (unlimited) | Max agent turns per invocation |

---

## `[channels.telegram]`

Telegram adapter. See [Telegram Bot Guide](../guides/telegram-bot.md).

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `bot_token` | string | **required** | Telegram bot token |
| `allowed_senders` | integer[] | `[]` (all) | Allowed Telegram user IDs |
| `debounce_ms` | integer | `2000` | Message debounce in milliseconds |

```toml
[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = [514133400]
debounce_ms = 2000
```

---

## `[channels.discord]`

Discord adapter. See [Discord Bot Guide](../guides/discord-bot.md).

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `bot_token` | string | **required** | Discord bot token |
| `allowed_guilds` | integer[] | `[]` | Allowed Discord server IDs |
| `allowed_users` | integer[] | `[]` (all in guilds) | Allowed Discord user IDs |
| `debounce_ms` | integer | `2000` | Message debounce in milliseconds |

### Channel routing

```toml
[channels.discord.routing.channel-name]
worker = "worker-name"
```

| Field | Type | Description |
|-------|------|------------|
| `worker` | string | Name of the worker to route messages to |

---

## `[channels.slack]`

Slack adapter (Socket Mode). See [Slack Bot Guide](../guides/slack-bot.md).

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `bot_token` | string | **required** | Bot token (`xoxb-...`) |
| `app_token` | string | **required** | App-level token (`xapp-...`) |
| `allowed_channels` | string[] | `[]` (all) | Allowed channel names |
| `allowed_users` | string[] | `[]` (all) | Allowed Slack user IDs |
| `debounce_ms` | integer | `2000` | Message debounce in milliseconds |

```toml
[channels.slack]
bot_token = "${SLACK_BOT_TOKEN}"
app_token = "${SLACK_APP_TOKEN}"
allowed_channels = ["general", "ai-chat"]
allowed_users = ["U12345678"]
debounce_ms = 2000
```

---

## `[persistence]`

Database configuration.

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `db_path` | string | `"~/.yoclaw/yoclaw.db"` | Path to SQLite database file |

```toml
[persistence]
db_path = "~/.yoclaw/yoclaw.db"
```

---

## `[security]`

Security policy. See [Security](../concepts/security.md).

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `shell_deny_patterns` | string[] | `[]` | Substring patterns to block in shell commands |

### Tool permissions

```toml
[security.tools.tool-name]
enabled = true                      # Enable/disable the tool
allowed_paths = ["/home/user/"]     # Path prefixes (file tools only)
allowed_hosts = ["api.github.com"]  # Hostnames (http tool only)
requires_approval = false           # Log as requiring approval
```

### Injection detection

```toml
[security.injection]
enabled = false                     # Enable injection detection
action = "warn"                     # "warn", "block", or "log"
extra_patterns = []                 # Additional patterns to detect
```

---

## `[web]`

Web UI and API. See [Web UI](../concepts/web-ui.md).

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `enabled` | bool | `false` | Enable the web server |
| `port` | integer | `19898` | Port to listen on |
| `bind` | string | `"127.0.0.1"` | Address to bind to |

```toml
[web]
enabled = true
port = 19898
bind = "127.0.0.1"
```

---

## `[scheduler]`

Scheduler for cortex and cron jobs. See [Scheduler](../concepts/scheduler.md).

| Field | Type | Default | Description |
|-------|------|---------|------------|
| `enabled` | bool | `false` | Enable the scheduler |
| `tick_interval_secs` | integer | `60` | How often to check for due tasks |

### Cortex

```toml
[scheduler.cortex]
interval_hours = 6                          # Hours between cortex runs
model = "claude-haiku-4-5-20251001"         # Model for cortex LLM tasks
```

### Cron jobs

```toml
[[scheduler.cron.jobs]]
name = "morning-briefing"           # Unique job name
schedule = "0 9 * * *"              # 5-field cron expression
prompt = "Good morning!"            # Message to the agent
target = "tg-514133400"             # Session ID for delivery
session = "isolated"                # "isolated" or "persistent"
```

> Use `[[scheduler.cron.jobs]]` (double brackets) for each job — this is TOML's array-of-tables syntax.
