-- Cron jobs table for user-defined scheduled tasks
CREATE TABLE IF NOT EXISTS cron_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    schedule TEXT NOT NULL,
    prompt TEXT NOT NULL,
    target_channel TEXT,
    session_mode TEXT DEFAULT 'isolated',
    enabled INTEGER DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Cron run history
CREATE TABLE IF NOT EXISTS cron_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL REFERENCES cron_jobs(id),
    status TEXT NOT NULL,
    result TEXT,
    tokens_used INTEGER DEFAULT 0,
    started_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_cron_runs_job ON cron_runs(job_id);
