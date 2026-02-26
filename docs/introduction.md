# Introduction

**yoclaw** is a secure, single-binary AI agent orchestrator. It connects large language models to messaging platforms — Telegram, Discord, and Slack — with persistent memory, tool use, and security policies. Built in Rust on top of [yoagent](https://github.com/yologdev/yoagent).

One binary. One config file. One SQLite database. No Docker, no Redis, no Kubernetes.

## Why yoclaw?

Most agent frameworks are designed for cloud deployments: microservices, message queues, container orchestrators. yoclaw takes the opposite approach. It's a single process that runs on your laptop, a $5 VPS, or a Raspberry Pi. Everything lives in one place.

### Future-proof

yoclaw supports **7 LLM providers** today:

| Provider | Example models |
|----------|---------------|
| Anthropic | Claude Opus, Sonnet, Haiku |
| OpenAI | GPT-4o, o3, o4-mini |
| Google | Gemini 2.5 Pro, Flash |
| Vertex AI | Gemini via Google Cloud |
| Azure OpenAI | GPT-4o via Azure |
| AWS Bedrock | Claude via AWS |
| OpenAI Responses | OpenAI Responses API |

Switching models is one line in your config:

```toml
model = "claude-opus-4-6"        # today
model = "claude-5-sonnet"        # tomorrow — change and restart
```

When the next generation of models drops, you don't rewrite your bot. You change `model =` and restart.

### Crash-proof

Every incoming message is written to an SQLite queue *before* processing begins. If the process is killed — `kill -9`, power outage, OOM — the queue survives. On restart, yoclaw automatically requeues stale messages and picks up where it left off.

No messages are lost. No state is corrupted. The WAL-mode SQLite database handles this for free.

### Security-first

Every tool call the agent makes passes through a security policy layer:

- **Shell deny patterns** — block dangerous commands (`rm -rf`, `sudo`, `chmod 777`)
- **Tool permissions** — enable/disable individual tools, require approval for sensitive ones
- **Path allowlists** — restrict file tools to specific directories
- **Host allowlists** — restrict HTTP tools to specific domains
- **Injection detection** — detect and block prompt injection attempts in incoming messages
- **Budget enforcement** — daily token limits and per-session turn limits
- **Full audit trail** — every tool call is logged with timestamps

### Memory that lasts

yoclaw stores memories in SQLite with FTS5 full-text search. Memories are categorized (fact, preference, decision, task, context) and each category has a temporal decay curve — recent memories matter more, but decisions never fade.

Optional semantic search (via the `semantic` feature flag) adds vector embeddings using embedding-gemma-300m. Results from FTS5 and vector search are merged using Reciprocal Rank Fusion for best-of-both-worlds retrieval.

The **cortex** — an automated maintenance scheduler — periodically deduplicates memories, cleans up stale entries, consolidates related memories, and indexes conversations into long-term storage.

## Feature summary

| Feature | What yoclaw provides |
|---------|---------------------|
| Deployment | Single binary, no external services |
| Persistence | SQLite with WAL mode — conversations, queue, memory, audit |
| Memory | FTS5 full-text search + optional vector embeddings |
| Security | Policy engine: tool permissions, shell deny patterns, path/host allowlists, injection detection |
| Crash recovery | Automatic queue reprocessing on restart |
| Channels | Telegram, Discord, Slack (simultaneously) |
| Workers | Sub-agent delegation with per-worker models and prompts |
| Scheduling | Built-in cron jobs with ephemeral or persistent sessions |
| Providers | Anthropic, OpenAI, Google, Vertex AI, Azure, Bedrock, OpenAI Responses |
| Budget | Daily token limits and per-session turn limits |
| Web UI | Embedded dashboard with REST API and SSE |

## What's in this book

- **Getting Started** — Install yoclaw and send your first message in under 5 minutes.
- **Concepts** — Deep dives into architecture, channels, memory, security, workers, skills, and scheduling.
- **Guides** — Step-by-step walkthroughs for each messaging platform and creative use-case configurations.
- **Reference** — Complete configuration reference, CLI documentation, and session ID formats.
