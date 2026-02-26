# Architecture

yoclaw is a single-process agent orchestrator. Everything runs in one binary with one SQLite database.

## Core flow

```
Channel (Telegram/Discord/Slack)
    → MessageCoalescer (debounce)
    → Queue (SQLite)
    → Conductor
    → Agent (yoagent)
    → Response
    → Channel
```

Every message follows this path. There are no background queues, message brokers, or external services in the loop.

## The Conductor

The Conductor is the central component. It owns a single [yoagent](https://github.com/yologdev/yoagent) `Agent` instance and mediates all interactions with it.

```rust
pub struct Conductor {
    agent: Agent,
    db: Db,
    current_session: String,
    // ...
}
```

### Session switching

The key constraint: `Agent::prompt()` takes `&mut self`. Only one conversation can be active at a time. The Conductor handles multi-session support by saving and loading conversation state:

1. **Save** current session's messages to the `tape` table in SQLite
2. **Clear** the agent's in-memory messages
3. **Restore** the target session's messages from SQLite
4. **Process** the new message through the agent

This means concurrent messages from different sessions are queued and processed sequentially. This is fine for personal use or small teams — a typical agent turn takes 2-10 seconds, and the queue ensures nothing is lost.

### Message queue

Before the Conductor processes any message, it's persisted to the SQLite queue with status `pending`. Processing changes it to `processing`, and completion marks it `done` or `failed`.

If the process crashes during processing, the message remains in `processing` state. On next startup, `queue_requeue_stale()` automatically resets these back to `pending` for reprocessing.

## Message coalescing

When users type multiple messages quickly (common on mobile), the MessageCoalescer debounces them into a single prompt. Each channel has a configurable debounce window (default: 2000ms).

```
User: "Hey"          (t=0ms)
User: "can you"      (t=800ms)
User: "check the weather?"  (t=1500ms)
                      ↓ (debounce expires at t=3500ms)
Coalesced: "Hey\ncan you\ncheck the weather?"
```

This reduces unnecessary LLM calls and produces more coherent conversations.

## Tool execution

yoclaw's agent has access to yoagent's default tools plus yoclaw-specific tools:

| Tool | Description |
|------|------------|
| `bash` | Execute shell commands |
| `read_file` | Read file contents |
| `edit_file` | Edit files with search/replace |
| `write_file` | Write file contents |
| `list_files` | List directory contents |
| `search` | Search file contents with regex |
| `http` | Make HTTP requests |
| `memory_search` | Search long-term memory |
| `memory_store` | Store to long-term memory |
| `cron_schedule` | Manage scheduled jobs |
| `send_message` | Send messages to channels |

Every tool call passes through the `SecureToolWrapper`, which checks the security policy before execution and logs the call to the audit trail.

## Database schema

All state lives in a single SQLite file (`~/.yoclaw/yoclaw.db`) with WAL mode enabled:

| Table | Purpose |
|-------|---------|
| `tape` | Conversation history per session |
| `queue` | Message processing queue |
| `memory` | Long-term memory storage |
| `memory_fts` | FTS5 index for memory search |
| `audit` | Tool call audit log |
| `state` | Key-value state (cortex timestamps, etc.) |
| `cron_jobs` | Scheduled job definitions |
| `cron_runs` | Cron execution history |
| `schema_version` | Migration tracking |

### Async/sync bridge

SQLite (via rusqlite) is synchronous. yoclaw runs on tokio (async). The `Db` struct bridges this with `spawn_blocking`:

```rust
impl Db {
    pub async fn exec<F, T>(&self, f: F) -> Result<T, DbError>
    // Runs `f` inside spawn_blocking

    pub fn exec_sync<F, T>(&self, f: F) -> Result<T, DbError>
    // Direct execution — for tests and sync callbacks
}
```

When `exec_sync` is called from an async context (like yoagent's sync `on_after_turn` callback), it must be wrapped in `tokio::task::block_in_place()` to avoid blocking the tokio worker thread.

## Scaling

yoclaw is designed for personal and small-team use. A single instance handles one message at a time — messages from different sessions are queued and processed sequentially.

For horizontal scaling, run multiple yoclaw instances, each with its own config, database, and set of sessions. There is no built-in clustering or shared state between instances.
