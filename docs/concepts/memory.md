# Memory

yoclaw gives your agent persistent long-term memory. The agent can store facts, preferences, decisions, and context that survive across conversations and restarts.

## How it works

Memory is stored in SQLite with an FTS5 full-text search index. The agent has two tools:

- **`memory_store`** — Save a memory with optional key, tags, category, and importance
- **`memory_search`** — Query memories by keyword, returning the most relevant results

### Memory entry fields

| Field | Description |
|-------|------------|
| `key` | Optional unique identifier for upsert behavior |
| `content` | The memory text |
| `tags` | Comma-separated tags for organization |
| `source` | Where this memory came from (e.g., session ID) |
| `category` | One of: `fact`, `preference`, `decision`, `task`, `context`, `event`, `reflection` |
| `importance` | 1-10 scale (default: 5) |
| `access_count` | How many times this memory has been retrieved |

## Categories and decay

Not all memories age equally. yoclaw applies **temporal decay** — newer memories score higher, but the decay rate depends on category:

| Category | Half-life | Example |
|----------|-----------|---------|
| `task` | 7 days | "Deploy the new feature by Friday" |
| `context` | 14 days | Compacted conversation summaries |
| `event` | 14 days | "Met with the team on Monday" |
| `fact` | 30 days | "The database runs on port 5432" |
| `reflection` | 60 days | "The refactoring improved response times" |
| `preference` | 90 days | "User prefers concise responses" |
| `decision` | Never | "We chose PostgreSQL over MySQL" |

The decay formula: `score × 0.5^(age_days / half_life)`

This means a task memory from a week ago scores half as much as a task created today. But a decision from six months ago retains its full relevance.

## Search: FTS5 + vector

### FTS5 (default)

Full-text search uses SQLite's FTS5 extension with prefix matching. Queries are tokenized and each token is prefix-matched:

```
Query: "database migration"
FTS5:  "database"* AND "migration"*
```

This matches "database", "databases", "migration", "migrations", etc.

### Vector search (optional)

Enable the `semantic` feature flag to add vector-based similarity search:

```bash
cargo install yoclaw --features semantic
```

This uses embedding-gemma-300m (300M parameter model) to generate embeddings locally — no API calls needed. Vectors are stored in SQLite via sqlite-vec for KNN (k-nearest-neighbor) search.

### Result fusion

When both FTS5 and vector search are available, results are merged using **Reciprocal Rank Fusion (RRF)**:

```
RRF_score = 1/(k + rank_fts) + 1/(k + rank_vec)
```

Then temporal decay is applied to the fused scores, and results are truncated to the requested limit. This gives you the precision of keyword search combined with the semantic understanding of embeddings.

The search pipeline over-fetches 3x the requested limit, applies decay-weighted re-ranking, then truncates — ensuring the final results are truly the most relevant.

## Cortex maintenance

The **cortex** is an automated memory maintenance system that runs periodically (default: every 6 hours). It performs four tasks:

### 1. Stale cleanup

Removes memory entries that haven't been accessed in **90+ days** and have **importance <= 3**. Decisions (category `"decision"`) are never cleaned up regardless of age or importance.

### 2. Deduplication

Finds memories with identical content and removes duplicates, keeping the most recently created entry.

### 3. Consolidation

Scans sessions updated in the last 24 hours (with at least 4 messages) and uses an LLM to extract 1-3 durable facts from each conversation. Extracted facts are stored as memories with category `"fact"` and importance 6. Each session is only consolidated once.

### 4. Session indexing

Summarizes recent sessions (updated in the last 24 hours, at least 2 messages) into 1-2 sentence summaries stored as `"reflection"` category memories. This makes past conversations searchable by topic. Each session is only indexed once.

### Cortex configuration

```toml
[scheduler]
enabled = true

[scheduler.cortex]
interval_hours = 6                          # How often to run
model = "claude-haiku-4-5-20251001"         # Model for consolidation/indexing
```

The cortex uses an inexpensive model (Haiku by default) since its tasks are straightforward summarization and extraction.
