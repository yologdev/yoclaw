# Installation

## From crates.io

The simplest way to install yoclaw:

```bash
cargo install yoclaw
```

This builds and installs the `yoclaw` binary to `~/.cargo/bin/`.

### With semantic memory

To enable vector-based memory search (uses embedding-gemma-300m for local embeddings):

```bash
cargo install yoclaw --features semantic
```

> The `semantic` feature adds ~200MB of model download on first run. FTS5 full-text search works without it and is sufficient for most use cases.

## From source

Clone the repository and build:

```bash
git clone https://github.com/yologdev/yoclaw.git
cd yoclaw
cargo build --release
```

The binary is at `target/release/yoclaw`. Copy it to your `$PATH` or run it directly.

## Requirements

- **Rust 1.75+** — yoclaw uses edition 2021 features
- **SQLite** — bundled automatically via rusqlite (no system SQLite needed)
- **An LLM API key** — Anthropic, OpenAI, Google, or any supported provider
- **A bot token** — from Telegram, Discord, or Slack (whichever platform you want to use)

## Verify installation

```bash
yoclaw --version
```

You should see output like:

```
yoclaw 1.1.1
```

## Next steps

Run `yoclaw init` to create your config directory, then head to [Quick Start](quick-start.md) to get your first bot running.
