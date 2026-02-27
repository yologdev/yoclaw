-- Dynamic worker definitions saved for reuse
CREATE TABLE IF NOT EXISTS saved_workers (
    name TEXT PRIMARY KEY,
    system_prompt TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
