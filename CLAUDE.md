# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```bash
cargo build                    # Build
cargo test                     # All 145 tests
cargo test config              # Tests in a specific module
cargo test test_parse_minimal  # Single test by name
cargo clippy -- -D warnings    # Lint (CI-style)
cargo fmt --check              # Check formatting
```

Run with debug logging:
```bash
RUST_LOG=yoclaw=debug cargo run
```

## Architecture

yoclaw is a single-binary AI agent orchestrator built on **yoagent v0.5.0** (path dep: `../yoagent-repo`). It connects LLMs to messaging platforms with SQLite persistence.

### Core flow

```
Channel (Telegram/Discord/Slack) → MessageCoalescer (debounce) → Queue (SQLite) → Conductor → Agent (yoagent) → Response → Channel
```

### Key constraint

`Agent::prompt()` takes `&mut self` — only one session processes at a time. The Conductor switches sessions by saving/loading conversation state to the tape table (`save_messages` → `clear_messages` → `restore_messages`). This means concurrent messages are queued and processed sequentially. Fine for personal/small-team use, but does not scale horizontally. Scaling would require running multiple yoclaw instances, each with its own agent.

### Module responsibilities

- **conductor/** — Owns the yoagent `Agent`. Handles session switching, drains `AgentEvent` stream, persists to tape. `resolve_provider()` returns `DynProvider(Box<dyn StreamProvider>)` to support multiple LLM providers (anthropic, openai, google, vertex, azure, bedrock, openai_responses). `delegate.rs` builds `SubAgentTool` workers from config. `tools.rs` implements `MemorySearchTool`/`MemoryStoreTool`. `direct_workers` HashMap enables direct worker delegation bypassing the main agent.
- **channels/** — `ChannelAdapter` trait (`Send + Sync`, stored as `Arc<dyn ChannelAdapter>`) for messaging platforms. `telegram.rs` (teloxide), `discord.rs` (serenity), `slack.rs` (Socket Mode). `coalesce.rs` debounces rapid messages per session with per-channel configurable debounce.
- **db/** — `Db` wraps `Arc<Mutex<Connection>>`. All methods use `spawn_blocking` for async safety. Tables: tape, queue, memory (+ FTS5), audit, state, cron_jobs, cron_runs. `vector.rs` (behind `semantic` feature flag) provides `EmbeddingEngine` (embedding-gemma-300m) and sqlite-vec KNN search; `memory.rs` uses RRF (Reciprocal Rank Fusion) to merge FTS5 and vector results, then applies temporal decay weighted by RRF scores.
- **scheduler/** — Unified scheduler for cortex maintenance and cron jobs. `cortex.rs` handles memory dedup, stale cleanup, consolidation, session indexing. `cron.rs` runs due jobs via ephemeral or persistent agents based on session mode. `tools.rs` provides `CronScheduleTool` for conversational cron management.
- **security/** — `SecureToolWrapper` wraps every `AgentTool`, checks `SecurityPolicy` before delegating. `BudgetTracker` uses `AtomicU64` for sync compatibility with yoagent's `on_before_turn` callback.
- **skills/** — Loads `SKILL.md` files, parses `tools` from YAML frontmatter, filters out skills requiring disabled tools.
- **web/** — Embedded web UI via rust-embed (`web/dist/`). Axum server with REST API (`/api/sessions`, `/api/queue`, `/api/budget`, `/api/audit`) and SSE (`/api/events`).
- **config.rs** — TOML parsing with `${ENV_VAR}` expansion and `~` tilde expansion.
- **migrate.rs** — Migration from OpenClaw installations (persona, skills, memories).

### yoagent integration

- `AgentEvent` is NOT `Serialize` — tape stores `Vec<AgentMessage>` snapshots (which IS Serialize)
- `on_before_turn` / `on_after_turn` callbacks are **sync** — hence `AtomicU64` for budget tracking
- `AgentTool::execute()` uses v0.5.0 signature: `execute(params, ctx: ToolContext)` where `ToolContext` bundles `tool_call_id`, `tool_name`, `cancel`, `on_update`, `on_progress`
- Workers use `SubAgentTool` (ephemeral: fresh `agent_loop` per invocation)
- Direct worker delegation: `delegate_to_worker` calls `SubAgentTool::execute` directly, persists exchange to tape, invalidates session
- Ephemeral agents: `run_ephemeral_prompt()` in `scheduler/mod.rs` uses `agent_loop` directly for cron/cortex tasks; `AgentLoopConfig` requires `input_filters` field
- Persistent agents: `run_persistent_prompt()` loads prior conversation from tape, runs `agent_loop` (max 5 turns), saves back — used by cron jobs with `session_mode = "persistent"`
- Default tools from `yoagent::tools::default_tools()` are wrapped with `SecureToolWrapper`
- Direct workers are NOT wrapped in `SecureToolWrapper` — their inner tools are already secured; wrapping the SubAgentTool itself would audit under the worker name, not a real tool name

### Async/sync bridge

The `Db` struct wraps rusqlite (sync) for tokio (async):
```rust
pub async fn exec<F, T>(&self, f: F) -> Result<T, DbError>  // spawn_blocking
pub fn exec_sync<F, T>(&self, f: F) -> Result<T, DbError>   // direct, for tests
```

**Important:** `exec_sync` is designed for tests and sync callbacks. When calling from an async context (e.g. `on_after_turn`), wrap in `tokio::task::block_in_place()` to avoid blocking the tokio worker thread.

### Cron delivery

Cron jobs use `target_channel` (a session_id like `"tg-514133400"`) to route delivery. `channel_from_session_id()` in `scheduler/cron.rs` maps session_id prefixes to adapter names (`"tg-"` → `"telegram"`, `"dc-"` → `"discord"`, `"slack-"` → `"slack"`). `OutgoingMessage.channel` must match `adapter.name()`, while `session_id` carries the actual routing info (e.g. chat_id).

### Config hot-reload

The watcher reloads config on file changes, but not all settings are hot-reloadable:
- **Hot-reloadable:** budget limits, security policy (deny patterns, tool permissions), debounce timings
- **Requires restart:** agent provider/model/api_key, injection detection config, Discord allowlist/routing, workers, skills

### Config location

`~/.yoclaw/config.toml` — persona at `~/.yoclaw/persona.md`, skills in `~/.yoclaw/skills/`, DB at `~/.yoclaw/yoclaw.db`.

## Testing patterns

- `Db::open_memory()` for in-memory SQLite (no files)
- `MockProvider::text("response")` / `MockProvider::texts(vec![...])` from yoagent for LLM simulation
- `tempfile::TempDir` for skill loading tests
- Test conductor helper in `conductor/mod.rs` builds a full Conductor with MockProvider

## Conventions

- Error types via `thiserror` per module (`DbError`, `ConfigError`, `SecurityDenied`, `SkillError`)
- `anyhow` at the binary boundary (main.rs)
- Security tool name mapping: yoagent's `bash` → config's `shell`, `edit_file` → `write_file`
- Session IDs: `tg-{chat_id}` for Telegram, `dc-{channel_id}` for Discord, `slack-{channel}` / `slack-{channel}-{thread_ts}` for Slack, `cron-{job_name}` for scheduled jobs
- SQL migrations via `include_str!` in `db/mod.rs`, tracked by `schema_version` table
- String splitting/truncation must use `is_char_boundary()` to avoid panicking on multi-byte UTF-8 (see `split_message` in `channels/mod.rs`)
- Cron config uses `[[scheduler.cron.jobs]]` (TOML array-of-tables), NOT `[scheduler.cron.job_name]`
- `allowed_paths` in security config only applies to file tools (`read_file`, `write_file`, `edit_file`, `list_files`, `search`), not `bash`/`shell`
- Empty responses must be avoided — Telegram and Discord reject empty message bodies. Early-return paths (injection block, budget exceeded) must return a canned message.
- Discord adapter requires **Message Content Intent** enabled in the Discord Developer Portal
