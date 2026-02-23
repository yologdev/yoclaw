-- Tape: stores serialized conversation state per session
CREATE TABLE tape (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL UNIQUE,
    messages_json TEXT NOT NULL,
    message_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_tape_session ON tape(session_id);

-- Queue: crash-safe inbound message queue
CREATE TABLE queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    sender_id TEXT NOT NULL,
    sender_name TEXT,
    session_id TEXT NOT NULL,
    content TEXT NOT NULL,
    reply_to TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    error_msg TEXT,
    created_at INTEGER NOT NULL,
    processed_at INTEGER
);
CREATE INDEX idx_queue_status ON queue(status);
CREATE INDEX idx_queue_session ON queue(session_id);

-- Memory: long-term agent memory with full-text search
CREATE TABLE memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT,
    content TEXT NOT NULL,
    tags TEXT,
    source TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_memory_key ON memory(key);

-- FTS5 virtual table for memory search
CREATE VIRTUAL TABLE memory_fts USING fts5(
    content,
    tags,
    content=memory,
    content_rowid=id
);

-- Triggers to keep FTS in sync
CREATE TRIGGER memory_ai AFTER INSERT ON memory BEGIN
    INSERT INTO memory_fts(rowid, content, tags)
    VALUES (new.id, new.content, new.tags);
END;
CREATE TRIGGER memory_ad AFTER DELETE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, tags)
    VALUES ('delete', old.id, old.content, old.tags);
END;
CREATE TRIGGER memory_au AFTER UPDATE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, tags)
    VALUES ('delete', old.id, old.content, old.tags);
    INSERT INTO memory_fts(rowid, content, tags)
    VALUES (new.id, new.content, new.tags);
END;

-- Audit: security-relevant event log
CREATE TABLE audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT,
    event_type TEXT NOT NULL,
    tool_name TEXT,
    detail TEXT,
    tokens_used INTEGER DEFAULT 0,
    timestamp INTEGER NOT NULL
);
CREATE INDEX idx_audit_session ON audit(session_id);
CREATE INDEX idx_audit_type ON audit(event_type);
CREATE INDEX idx_audit_timestamp ON audit(timestamp);

-- State: key-value store for runtime state
CREATE TABLE state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
