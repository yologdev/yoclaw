-- Vector embeddings for semantic memory search (requires sqlite-vec extension)
-- This migration adds vector search support and enriches the memory table.
-- Vector table is created at runtime only when the semantic feature is enabled.

-- Enrich memory table with categories, importance, access tracking
ALTER TABLE memory ADD COLUMN category TEXT DEFAULT 'fact';
-- categories: fact, preference, decision, event, task, reflection

ALTER TABLE memory ADD COLUMN importance INTEGER DEFAULT 5;
-- 1-10 scale, used by cortex for pruning decisions

ALTER TABLE memory ADD COLUMN last_accessed INTEGER;
-- epoch milliseconds, updated on each search hit

ALTER TABLE memory ADD COLUMN access_count INTEGER DEFAULT 0;
-- incremented on each search hit
