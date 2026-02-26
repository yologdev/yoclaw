# CLI Reference

yoclaw provides a simple command-line interface.

## Usage

```
yoclaw [OPTIONS] [COMMAND]
```

## Global options

| Option | Short | Description |
|--------|-------|------------|
| `--config <PATH>` | `-c` | Path to config file (default: `~/.yoclaw/config.toml`) |
| `--version` | `-V` | Print version |
| `--help` | `-h` | Print help |

## Commands

### `yoclaw` (no command)

Start the agent. This is the main operating mode — it loads config, connects to channels, and begins processing messages.

```bash
yoclaw
yoclaw -c /path/to/custom/config.toml
```

Environment variables:

| Variable | Description |
|----------|------------|
| `RUST_LOG` | Logging level (e.g., `yoclaw=debug`, `yoclaw=trace`) |
| `ANTHROPIC_API_KEY` | Anthropic API key (if using `${ANTHROPIC_API_KEY}` in config) |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token (if using `${TELEGRAM_BOT_TOKEN}` in config) |
| `DISCORD_BOT_TOKEN` | Discord bot token |
| `SLACK_BOT_TOKEN` | Slack bot token |
| `SLACK_APP_TOKEN` | Slack app-level token |

### `yoclaw init`

Initialize a new yoclaw configuration directory.

```bash
yoclaw init
yoclaw init -c /custom/path/config.toml
```

Creates:

| File | Description |
|------|------------|
| `~/.yoclaw/config.toml` | Default configuration with placeholder values |
| `~/.yoclaw/persona.md` | Default persona prompt |
| `~/.yoclaw/skills/` | Empty skills directory |

If the config file already exists, it's not overwritten.

### `yoclaw inspect`

Show the current state of the agent: queue, sessions, budget, and audit log.

```bash
yoclaw inspect                              # Overview
yoclaw inspect --session tg-514133400       # Filter by session
yoclaw inspect --skills                     # Show loaded skills
yoclaw inspect --workers                    # Show configured workers
```

| Option | Short | Description |
|--------|-------|------------|
| `--session <ID>` | `-s` | Filter audit log by session ID |
| `--skills` | | Show loaded skills and their tool requirements |
| `--workers` | | Show configured worker sub-agents |

#### Example output

```
=== Queue ===
Pending messages: 0

=== Sessions (3) ===
  tg-514133400 — 47 messages, last updated 2026-02-27 14:23:01
  dc-1234567890 — 12 messages, last updated 2026-02-27 10:15:30
  slack-C03947L0E — 8 messages, last updated 2026-02-26 18:00:00

=== Budget ===
Tokens used today: 45230
Daily limit: 1000000
Remaining: 954770

=== Recent Audit (5) ===
  [14:23:01] tool_call bash ls -la /tmp/...
  [14:22:58] tool_call memory_search database...
  [14:22:45] tool_call http GET https://api.github.com/...
  [10:15:30] tool_call memory_store key=project-status...
  [10:15:28] tool_call bash git status...
```

### `yoclaw migrate`

Migrate from an OpenClaw installation.

```bash
yoclaw migrate /path/to/openclaw/data
```

Imports persona, skills, and memories from an existing OpenClaw setup.

## Debug logging

Enable detailed logging to diagnose issues:

```bash
# Info level (default)
RUST_LOG=yoclaw=info yoclaw

# Debug level — shows message flow, tool calls, session switching
RUST_LOG=yoclaw=debug yoclaw

# Trace level — maximum verbosity
RUST_LOG=yoclaw=trace yoclaw
```

You can also filter by module:

```bash
# Only conductor debug logs
RUST_LOG=yoclaw::conductor=debug yoclaw

# Debug for channels, info for everything else
RUST_LOG=yoclaw=info,yoclaw::channels=debug yoclaw
```
