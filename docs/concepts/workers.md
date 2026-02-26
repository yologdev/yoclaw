# Workers

Workers are specialized sub-agents that handle specific tasks. Instead of one agent doing everything, you can delegate to focused agents with different models, system prompts, and capabilities.

## Configuring workers

Workers are defined under `[agent.workers]` in your config:

```toml
# Default settings for all workers (optional)
[agent.workers]
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
max_tokens = 4096

# Named workers override defaults
[agent.workers.research]
model = "claude-sonnet-4-20250514"
system_prompt = "You are a research assistant. Search the web thoroughly and provide detailed, well-sourced answers."
max_turns = 15

[agent.workers.coding]
model = "claude-sonnet-4-20250514"
system_prompt = "You are a coding assistant. Write clean, well-tested code. Use the shell and file tools."
max_turns = 10

[agent.workers.quick]
model = "claude-haiku-4-5-20251001"
system_prompt = "You are a quick-answer bot. Be extremely concise."
max_turns = 3
```

### Worker settings

| Setting | Description | Default |
|---------|------------|---------|
| `provider` | LLM provider | Main agent's provider |
| `model` | Model ID | Workers default model or main model |
| `api_key` | API key | Main agent's key |
| `system_prompt` | Worker's persona/instructions | None |
| `max_tokens` | Max tokens per response | Workers default or provider default |
| `max_turns` | Max agent turns per invocation | No limit |

## How workers execute

Workers use `SubAgentTool` from yoagent. Each invocation is **ephemeral** — a fresh `agent_loop` runs for every request. The worker:

1. Receives the delegated task as input
2. Has access to the same tools as the main agent (with security wrapping)
3. Runs its agent loop up to `max_turns`
4. Returns the final response to the main agent

The main agent sees workers as tools it can call:

```
Agent: I'll delegate this research task to the research worker.
[Calls worker: research with "Find the latest Rust async patterns"]
Worker (research): [Searches web, reads docs, synthesizes answer]
Agent: Based on the research worker's findings...
```

## Direct worker delegation

Workers can also be invoked directly, bypassing the main agent entirely. This is used by Discord channel routing:

```toml
[channels.discord.routing.coding-help]
worker = "coding"
```

Messages in the `#coding-help` Discord channel go straight to the `coding` worker without the main agent seeing them. The worker's response is persisted to the tape and sent back to the channel.

## Multi-model strategies

Workers let you use different models for different tasks:

```toml
# Main agent: powerful model for complex reasoning
[agent]
model = "claude-opus-4-6"

# Research: mid-tier for web research
[agent.workers.research]
model = "claude-sonnet-4-20250514"

# Quick answers: fast and cheap
[agent.workers.quick]
model = "claude-haiku-4-5-20251001"
```

This gives you the intelligence of Opus where it matters, while keeping costs down for routine tasks.

## Cross-provider workers

Workers can use different providers than the main agent:

```toml
[agent]
provider = "anthropic"
model = "claude-opus-4-6"
api_key = "${ANTHROPIC_API_KEY}"

[agent.workers.gpt-coder]
provider = "openai"
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"
system_prompt = "You are a code review assistant."
```

This lets you leverage the strengths of different model families within a single agent setup.

## Worker configuration requires restart

Worker configuration is **not** hot-reloadable. Adding, removing, or modifying workers requires restarting yoclaw.
