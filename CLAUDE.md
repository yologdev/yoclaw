# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```bash
cargo build                    # Build
cargo test                     # All 61 tests
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

yoclaw is a single-binary AI agent orchestrator built on **yoagent** (path dep: `../yoagent-repo`). It connects LLMs to messaging platforms with SQLite persistence.

### Core flow

```
Channel (Telegram) → MessageCoalescer (debounce) → Queue (SQLite) → Conductor → Agent (yoagent) → Response → Channel
```

### Key constraint

`Agent::prompt()` takes `&mut self` — only one session processes at a time. The Conductor switches sessions by saving/loading conversation state to the tape table (`save_messages` → `clear_messages` → `restore_messages`).

### Module responsibilities

- **conductor/** — Owns the yoagent `Agent`. Handles session switching, drains `AgentEvent` stream, persists to tape. `delegate.rs` builds `SubAgentTool` workers from config. `tools.rs` implements `MemorySearchTool`/`MemoryStoreTool`.
- **channels/** — `ChannelAdapter` trait for messaging platforms. `telegram.rs` uses teloxide. `coalesce.rs` debounces rapid messages per session via `tokio::select!`.
- **db/** — `Db` wraps `Arc<Mutex<Connection>>`. All methods use `spawn_blocking` for async safety. Tables: tape, queue, memory (+ FTS5), audit, state.
- **security/** — `SecureToolWrapper` wraps every `AgentTool`, checks `SecurityPolicy` before delegating. `BudgetTracker` uses `AtomicU64` for sync compatibility with yoagent's `on_before_turn` callback.
- **skills/** — Loads `SKILL.md` files, parses `tools` from YAML frontmatter, filters out skills requiring disabled tools.
- **config.rs** — TOML parsing with `${ENV_VAR}` expansion and `~` tilde expansion.

### yoagent integration

- `AgentEvent` is NOT `Serialize` — tape stores `Vec<AgentMessage>` snapshots (which IS Serialize)
- `on_before_turn` / `on_after_turn` callbacks are **sync** — hence `AtomicU64` for budget tracking
- Workers use `SubAgentTool` (ephemeral: fresh `agent_loop` per invocation)
- Default tools from `yoagent::tools::default_tools()` are wrapped with `SecureToolWrapper`

### Async/sync bridge

The `Db` struct wraps rusqlite (sync) for tokio (async):
```rust
pub async fn exec<F, T>(&self, f: F) -> Result<T, DbError>  // spawn_blocking
pub fn exec_sync<F, T>(&self, f: F) -> Result<T, DbError>   // direct, for tests
```

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
- Session IDs: `tg-{chat_id}` for Telegram
- SQL migrations via `include_str!` in `db/mod.rs`, tracked by `schema_version` table
