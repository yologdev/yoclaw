# Security

yoclaw wraps every tool call with security policy enforcement. No tool executes without passing through the `SecureToolWrapper`.

## Security policy

The security policy is defined in `config.toml` under the `[security]` section:

```toml
[security]
shell_deny_patterns = ["rm -rf", "sudo", "chmod 777", "curl | bash"]

[security.tools.shell]
enabled = true
requires_approval = true

[security.tools.read_file]
enabled = true
allowed_paths = ["/home/user/projects/", "/tmp/"]

[security.tools.write_file]
enabled = true
allowed_paths = ["/home/user/projects/"]

[security.tools.http]
enabled = true
allowed_hosts = ["api.github.com", "api.openai.com"]

[security.tools.edit_file]
enabled = true
allowed_paths = ["/home/user/projects/"]
```

## Tool permissions

Each tool can be individually configured:

| Setting | Description |
|---------|------------|
| `enabled` | Whether the tool is available at all (default: `true`) |
| `allowed_paths` | Restrict file operations to these directory prefixes |
| `allowed_hosts` | Restrict HTTP requests to these hostnames |
| `requires_approval` | Log as requiring approval (future feature) |

### Tool name mapping

yoagent uses internal tool names that differ from config names:

| yoagent name | Config name |
|-------------|-------------|
| `bash` | `shell` |
| `edit_file` | `write_file` |

### Path allowlists

`allowed_paths` only applies to file tools: `read_file`, `write_file`, `edit_file`, `list_files`, and `search`. It does **not** restrict the `bash`/`shell` tool — use `shell_deny_patterns` for that.

When `allowed_paths` is empty (the default), no path restrictions are applied.

## Shell deny patterns

Shell deny patterns are substring matches against the command the agent wants to execute:

```toml
shell_deny_patterns = ["rm -rf", "sudo", "chmod 777", "mkfs", "dd if="]
```

If any pattern appears anywhere in the command, execution is denied.

## Injection detection

yoclaw can detect prompt injection attempts in incoming messages:

```toml
[security.injection]
enabled = true
action = "block"                    # "warn", "block", or "log"
extra_patterns = ["ignore previous instructions"]
```

| Action | Behavior |
|--------|----------|
| `warn` | Appends a warning to the message, lets it through |
| `block` | Rejects the message entirely, returns a canned response |
| `log` | Passes through normally, logs to audit trail |

The injection detector uses a built-in set of patterns plus any `extra_patterns` you configure. When `action = "block"`, a canned response is returned instead of an empty string (Telegram and Discord reject empty messages).

## Budget enforcement

Budget limits prevent runaway token usage:

```toml
[agent.budget]
max_tokens_per_day = 500_000
max_turns_per_session = 30
```

- **`max_tokens_per_day`** — Total tokens (input + output) across all sessions in a 24-hour period
- **`max_turns_per_session`** — Maximum agent turns (LLM calls) per message processing

The `BudgetTracker` uses `AtomicU64` for thread-safe tracking, compatible with yoagent's synchronous `on_before_turn` callback. Budget limits are hot-reloadable.

## Audit trail

Every tool call is logged to the `audit` table:

```
[14:23:01] tool_call bash rm -rf /tmp/test...
[14:23:02] tool_denied shell Command blocked by deny pattern: rm -rf
[14:23:15] tool_call http GET https://api.github.com/...
```

View the audit log with:

```bash
yoclaw inspect                          # Recent 20 entries
yoclaw inspect --session tg-514133400   # Filter by session
```

## Hot-reloadable security

The security policy is hot-reloadable. Changes to `shell_deny_patterns`, tool permissions, and budget limits take effect within 5 seconds without restarting yoclaw.

Injection detection configuration requires a restart.
