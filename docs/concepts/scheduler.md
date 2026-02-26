# Scheduler

The scheduler runs periodic tasks: cortex memory maintenance and user-defined cron jobs.

## Enabling the scheduler

```toml
[scheduler]
enabled = true
tick_interval_secs = 60     # How often to check for due tasks (default: 60)
```

## Cron jobs

Define recurring tasks that your agent executes on a schedule:

```toml
[[scheduler.cron.jobs]]
name = "morning-briefing"
schedule = "0 9 * * *"          # 9 AM daily
prompt = "Check my calendar and summarize today's schedule"
target = "tg-514133400"         # Deliver response to this session
session = "isolated"            # "isolated" or "persistent"

[[scheduler.cron.jobs]]
name = "weekly-review"
schedule = "0 18 * * 5"         # 6 PM every Friday
prompt = "Summarize what we accomplished this week based on your memories"
target = "tg-514133400"
session = "persistent"
```

> Note the `[[scheduler.cron.jobs]]` syntax — this is TOML's array-of-tables notation.

### Cron fields

| Field | Required | Description |
|-------|----------|------------|
| `name` | Yes | Unique job identifier |
| `schedule` | Yes | Cron expression (5-field: `min hour dom month dow`) |
| `prompt` | Yes | The message sent to the agent |
| `target` | No | Session ID for delivery (e.g., `tg-514133400`) |
| `session` | No | `"isolated"` (default) or `"persistent"` |

### Cron expressions

Standard 5-field cron expressions:

```
┌───────────── minute (0-59)
│ ┌───────────── hour (0-23)
│ │ ┌───────────── day of month (1-31)
│ │ │ ┌───────────── month (1-12)
│ │ │ │ ┌───────────── day of week (0-7, 0=Sun)
│ │ │ │ │
* * * * *
```

Examples:

| Expression | Meaning |
|-----------|---------|
| `0 9 * * *` | Every day at 9:00 AM |
| `*/30 * * * *` | Every 30 minutes |
| `0 18 * * 1-5` | Weekdays at 6:00 PM |
| `0 0 1 * *` | First of every month at midnight |

yoclaw automatically normalizes 5-field expressions to the 6/7-field format required by the cron library (prepends `0 ` for seconds).

### Session modes

- **`isolated`** (default) — Each execution is a fresh, ephemeral agent. No conversation history. Good for independent tasks.
- **`persistent`** — The agent remembers previous executions. Conversation history is loaded from and saved to the tape. Good for ongoing tasks that build on previous runs (max 5 turns per execution).

### Delivery

Cron job responses are delivered to channel adapters based on the `target` session ID:

| Session ID prefix | Channel |
|-------------------|---------|
| `tg-` | Telegram |
| `dc-` | Discord |
| `slack-` | Slack |

The `target` must be a valid session ID like `tg-514133400` (your Telegram chat ID). The response is sent as a regular message through the corresponding channel adapter.

### Conversational cron management

The agent also has a `cron_schedule` tool that lets users create, list, and delete cron jobs through conversation:

```
User: "Remind me to check deployments every weekday at 5pm"
Agent: [Uses cron_schedule tool to create job]
Agent: "Done! I've scheduled a daily reminder at 5 PM on weekdays."
```

Jobs created conversationally automatically use the current session as the delivery target.

## Cortex

The cortex is the automated memory maintenance system. See [Memory](memory.md) for details on what it does.

```toml
[scheduler.cortex]
interval_hours = 6                          # Run every 6 hours (default)
model = "claude-haiku-4-5-20251001"         # Model for LLM-powered tasks
```

Cortex tasks run as ephemeral agents using the specified model. They handle:

1. **Stale cleanup** — Remove decayed memories
2. **Deduplication** — Merge similar memories
3. **Consolidation** — Summarize related memory groups
4. **Session indexing** — Extract key facts from recent conversations

## Scheduler configuration requires restart

The scheduler configuration (cron jobs, cortex settings) requires a restart to take effect. Jobs created via the `cron_schedule` tool take effect immediately since they're stored in the database.
