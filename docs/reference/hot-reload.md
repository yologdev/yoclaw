# Hot Reload

yoclaw watches its config file for changes and applies safe updates without restarting. The file is checked every 5 seconds.

## What's hot-reloadable

These settings take effect within 5 seconds of saving the config file:

| Setting | Section |
|---------|---------|
| Daily token budget | `[agent.budget]` |
| Per-session turn limit | `[agent.budget]` |
| Shell deny patterns | `[security]` |
| Tool permissions (enable/disable, paths, hosts) | `[security.tools.*]` |
| Debounce timing per channel | `[channels.*.debounce_ms]` |

### Example: tighten budget on the fly

Edit `config.toml`:

```toml
[agent.budget]
max_tokens_per_day = 100_000    # Was 1_000_000 — reduce immediately
```

Within 5 seconds, the new limit takes effect. No restart needed.

### Example: block a shell command

```toml
[security]
shell_deny_patterns = ["rm -rf", "sudo", "chmod 777", "curl | bash"]
#                                                      ^^^^^^^^^^^^
#                                           Added new pattern — takes effect immediately
```

## What requires restart

These settings are only read at startup. Changing them requires stopping and restarting yoclaw:

| Setting | Why |
|---------|-----|
| Agent provider/model/api_key | Agent is constructed once at startup |
| Workers configuration | SubAgentTools are built at startup |
| Skills | Loaded into system prompt at startup |
| Injection detection config | Patterns compiled at startup |
| Discord `allowed_guilds` | Set in serenity Handler at startup |
| Discord channel routing | Routes built at startup |
| Scheduler/cron configuration | Scheduler reads config once |
| Web UI enable/port/bind | Axum server binds at startup |
| Database path | Database opened at startup |
| Persona file | Read and injected at startup |

## How it works

yoclaw uses a polling-based config watcher (not filesystem events) to maximize cross-platform compatibility:

1. Every 5 seconds, the watcher checks the config file's modification timestamp
2. If changed, it parses the new config
3. It diffs the old and new configs to find what changed
4. For hot-reloadable fields, it applies the changes directly:
   - Budget limits → updates `BudgetTracker`
   - Security policy → swaps the `Arc<RwLock<SecurityPolicy>>`
   - Debounce timing → updates the shared debounce map

Changes to non-reloadable fields are logged as warnings suggesting a restart.

## Watching for changes

With debug logging enabled, you can see reload events:

```bash
RUST_LOG=yoclaw=debug yoclaw
```

```
DEBUG yoclaw: Config changed, reloading...
DEBUG yoclaw: Hot-reload: budget max_tokens_per_day 1000000 → 500000
DEBUG yoclaw: Hot-reload: security policy updated (2 tool permissions changed)
```
