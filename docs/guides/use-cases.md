# Use Cases

Six creative configurations showing what yoclaw can do. Each includes a complete `config.toml` you can adapt.

---

## 1. Personal Knowledge Vault

A Telegram bot that remembers everything you tell it. Cortex consolidates memories overnight. Search your own brain.

**The idea:** You dump thoughts, links, ideas, and facts into a Telegram chat throughout the day. The agent stores them as memories. The cortex runs overnight to deduplicate, consolidate, and index. Days later, you search "what was that restaurant?" and get an answer.

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"
persona = "persona.md"

[agent.budget]
max_tokens_per_day = 500_000
max_turns_per_session = 30

[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = [514133400]
debounce_ms = 3000

[security]
shell_deny_patterns = ["rm -rf", "sudo"]

[security.tools.shell]
enabled = false

[scheduler]
enabled = true

[scheduler.cortex]
interval_hours = 8
model = "claude-haiku-4-5-20251001"
```

**Persona (`~/.yoclaw/persona.md`):**

```markdown
You are a personal knowledge vault. Your primary job is to remember things.

When the user tells you something — a fact, preference, idea, link, or decision:
1. Acknowledge it briefly
2. Store it using memory_store with appropriate category and tags
3. Use descriptive keys so memories are easy to find later

When the user asks a question:
1. Always search your memory first
2. If you find relevant memories, use them to answer
3. If not, say you don't have that stored

Categories to use:
- "fact" for information and knowledge
- "preference" for likes, dislikes, choices
- "decision" for choices made and their reasoning
- "task" for things to do
- "event" for things that happened

Be concise. Don't over-explain. This is a quick-capture tool.
```

---

## 2. Team DevOps War Room

A Discord server with channel-routed workers: `#incidents` routes to an on-call agent, `#deploys` routes to a deployment agent, `#general` goes to the main agent.

**The idea:** Your small team uses a Discord server. Different channels need different agent personalities and tool access. Incidents need cautious handling. Deploys need specific runbook knowledge. General chat gets the friendly helper.

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[agent.budget]
max_tokens_per_day = 2_000_000
max_turns_per_session = 40

# Worker defaults
[agent.workers]
provider = "anthropic"

[agent.workers.oncall]
model = "claude-opus-4-6"
system_prompt = """You are an incident response assistant. You help diagnose and resolve production issues.

Rules:
- Never run destructive commands without explicit confirmation
- Always check current state before making changes
- Log every action you take with timestamps
- Escalate if you're unsure — say "I recommend getting a human to review this"
"""
max_turns = 20

[agent.workers.deployer]
model = "claude-sonnet-4-20250514"
system_prompt = """You are a deployment assistant. You help with CI/CD pipelines, container builds, and rollouts.

You have access to shell and file tools. Use them to check deployment status, read logs, and verify configurations. Never deploy without the user explicitly requesting it.
"""
max_turns = 15

[channels.discord]
bot_token = "${DISCORD_BOT_TOKEN}"
allowed_guilds = [398994758158254080]
debounce_ms = 1500

[channels.discord.routing.incidents]
worker = "oncall"

[channels.discord.routing.deploys]
worker = "deployer"

[security]
shell_deny_patterns = ["rm -rf /", "sudo rm", "chmod 777", ":(){ :|:& };:"]

[security.tools.shell]
enabled = true

[security.tools.http]
enabled = true
allowed_hosts = ["api.github.com", "api.pagerduty.com"]

[web]
enabled = true
port = 19898
```

---

## 3. Daily Standup Bot

A Slack cron job that pings every morning, asking what you're working on. Persistent sessions track responses across days. Weekly summaries happen automatically on Friday.

**The idea:** At 9 AM, the bot asks "What are you working on today?" in your Slack channel. You reply throughout the day. On Friday at 5 PM, it summarizes the week's progress from the persistent conversation.

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[agent.budget]
max_tokens_per_day = 300_000

[channels.slack]
bot_token = "${SLACK_BOT_TOKEN}"
app_token = "${SLACK_APP_TOKEN}"
allowed_channels = ["standup"]
debounce_ms = 2000

[security]
shell_deny_patterns = ["rm -rf", "sudo"]

[security.tools.shell]
enabled = false

[scheduler]
enabled = true

[scheduler.cortex]
interval_hours = 12
model = "claude-haiku-4-5-20251001"

[[scheduler.cron.jobs]]
name = "daily-standup"
schedule = "0 9 * * 1-5"
prompt = "Good morning! What are you working on today? Any blockers?"
target = "slack-C03947L0E"
session = "persistent"

[[scheduler.cron.jobs]]
name = "weekly-summary"
schedule = "0 17 * * 5"
prompt = "It's Friday! Based on this week's standups, write a brief summary of what was accomplished, what's in progress, and any outstanding blockers."
target = "slack-C03947L0E"
session = "persistent"

[[scheduler.cron.jobs]]
name = "eod-reminder"
schedule = "0 16 * * 1-5"
prompt = "Friendly reminder: anything you want to log before end of day? Any decisions made or problems solved worth remembering?"
target = "slack-C03947L0E"
session = "persistent"
```

---

## 4. Research Agent

A web-enabled agent with strict security: shell disabled, HTTP allowlisted to specific domains, budget-capped. Safe to let it browse within boundaries.

**The idea:** You want an agent that can research things online, but you don't trust it with shell access or arbitrary web requests. Lock it down to specific API endpoints and give it a tight budget.

```toml
[agent]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key = "${ANTHROPIC_API_KEY}"

[agent.budget]
max_tokens_per_day = 200_000
max_turns_per_session = 25

[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = [514133400]
debounce_ms = 2000

[security]
shell_deny_patterns = []

# Disable dangerous tools entirely
[security.tools.shell]
enabled = false

[security.tools.write_file]
enabled = false

[security.tools.edit_file]
enabled = false

# Allow read-only file access to specific directories
[security.tools.read_file]
enabled = true
allowed_paths = ["/home/user/research/", "/tmp/"]

# Allow HTTP only to specific APIs
[security.tools.http]
enabled = true
allowed_hosts = [
    "api.github.com",
    "en.wikipedia.org",
    "api.semanticscholar.org",
    "arxiv.org",
    "news.ycombinator.com",
]

[security.injection]
enabled = true
action = "warn"

[scheduler]
enabled = true

[scheduler.cortex]
interval_hours = 6
model = "claude-haiku-4-5-20251001"
```

---

## 5. Multi-Model Router

Opus for deep reasoning, Haiku for quick lookups, Sonnet for code. Different workers handle different tasks, each running on the most appropriate model.

**The idea:** You have one primary agent (Opus) that can delegate to specialized workers. Complex reasoning goes to Opus directly. Quick factual questions get delegated to Haiku (fast and cheap). Code review and generation go to Sonnet.

```toml
[agent]
provider = "anthropic"
model = "claude-opus-4-6"
api_key = "${ANTHROPIC_API_KEY}"

[agent.budget]
max_tokens_per_day = 1_000_000
max_turns_per_session = 50

[agent.workers]
provider = "anthropic"

[agent.workers.quick]
model = "claude-haiku-4-5-20251001"
system_prompt = "You are a fast-answer assistant. Give brief, direct answers. No unnecessary elaboration."
max_turns = 3

[agent.workers.coder]
model = "claude-sonnet-4-20250514"
system_prompt = """You are a senior software engineer. When asked to:
- Review code: be thorough, check for bugs, security issues, and style
- Write code: produce clean, tested, well-documented code
- Debug: trace the issue systematically, explain root cause
Use shell and file tools as needed."""
max_turns = 15

[agent.workers.researcher]
model = "claude-sonnet-4-20250514"
system_prompt = "You are a research analyst. Use HTTP to gather information from multiple sources. Cross-reference facts. Provide citations."
max_turns = 20

[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = [514133400]
debounce_ms = 2000

[security]
shell_deny_patterns = ["rm -rf", "sudo", "chmod 777"]

[security.tools.http]
enabled = true

[web]
enabled = true
```

**With this setup, a conversation might look like:**

```
You: "What's the capital of France?"
Agent: [Delegates to 'quick' worker → instant "Paris" response]

You: "Review this Python function for security issues: [code]"
Agent: [Delegates to 'coder' worker → thorough security review]

You: "What are the tradeoffs between gRPC and REST for microservices?"
Agent: [Handles directly with Opus → nuanced analysis]
```

---

## 6. Paranoid Mode

Maximum lockdown: injection blocked, shell disabled, tight path allowlist, 10K token/day budget, full audit trail. For environments where security is paramount.

**The idea:** You want an AI assistant that can answer questions and remember things, but absolutely cannot execute commands, write files, or make web requests. Perfect for shared environments or when you're just not sure what the agent might do.

```toml
[agent]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"     # Cheap model for constrained usage
api_key = "${ANTHROPIC_API_KEY}"

[agent.budget]
max_tokens_per_day = 10_000             # Very tight budget
max_turns_per_session = 5               # Short conversations only

[channels.telegram]
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_senders = [514133400]           # Only you
debounce_ms = 2000

[security]
shell_deny_patterns = ["*"]             # Block everything

# Disable all dangerous tools
[security.tools.shell]
enabled = false

[security.tools.write_file]
enabled = false

[security.tools.edit_file]
enabled = false

[security.tools.read_file]
enabled = false

[security.tools.list_files]
enabled = false

[security.tools.search]
enabled = false

[security.tools.http]
enabled = false

# Block injection attempts
[security.injection]
enabled = true
action = "block"
extra_patterns = [
    "ignore previous",
    "disregard instructions",
    "system prompt",
    "you are now",
    "pretend you are",
]

[web]
enabled = true          # Keep monitoring on
port = 19898
```

**What's left enabled:** memory_store, memory_search, cron_schedule, send_message. The agent can remember things and schedule reminders, but can't touch the filesystem, run commands, or make network requests.

This is the safest possible configuration — the agent is essentially a smart notepad with a scheduling feature.
