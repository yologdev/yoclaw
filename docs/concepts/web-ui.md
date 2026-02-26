# Web UI

yoclaw includes an embedded web dashboard for monitoring your agent in real-time.

## Enabling the web UI

```toml
[web]
enabled = true
port = 19898            # Default port
bind = "127.0.0.1"     # Default: localhost only
```

Then visit `http://localhost:19898` in your browser.

To expose the web UI on all interfaces (e.g., for remote access):

```toml
[web]
enabled = true
bind = "0.0.0.0"
port = 8080
```

> Be careful exposing the web UI publicly — it has no authentication. Use a reverse proxy with auth if you need remote access.

## Dashboard features

The web UI is a single-page application embedded in the binary via rust-embed. It shows:

- **Active sessions** — All conversations with message counts and last activity
- **Message queue** — Pending, processing, and recently completed messages
- **Budget usage** — Token consumption today vs daily limit
- **Audit log** — Recent tool calls with timestamps and details

## REST API

The web server exposes a JSON API:

| Endpoint | Method | Description |
|----------|--------|------------|
| `/api/sessions` | GET | List all sessions with message counts |
| `/api/sessions/{id}/messages` | GET | Get conversation messages for a session |
| `/api/queue` | GET | Current queue state (pending count) |
| `/api/budget` | GET | Token usage and limits |
| `/api/audit` | GET | Recent audit log entries (supports `?session=` and `?limit=` query params) |

### Example: check budget

```bash
curl http://localhost:19898/api/budget
```

```json
{
  "tokens_used_today": 45230,
  "daily_limit": 1000000,
  "remaining": 954770
}
```

## Server-Sent Events (SSE)

The web UI uses SSE for real-time updates:

```
GET /api/events
```

Events are pushed when messages are processed:

```
event: message_processed
data: {"session_id":"tg-514133400","channel":"telegram"}
```

You can consume this from any SSE client:

```bash
curl -N http://localhost:19898/api/events
```

## Architecture

The web UI is a single HTML file at `web/dist/index.html`, embedded into the binary at compile time using rust-embed. The server is built on [axum](https://github.com/tokio-rs/axum) with tower-http for CORS support.

No build step is required for the frontend — it's plain HTML, CSS, and JavaScript shipped with the binary.
