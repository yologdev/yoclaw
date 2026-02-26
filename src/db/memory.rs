use super::{now_ms, Db, DbError};
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: Option<i64>,
    pub key: Option<String>,
    pub content: String,
    pub tags: Option<String>,
    pub source: Option<String>,
    pub category: String,
    pub importance: i32,
    pub last_accessed: Option<u64>,
    pub access_count: i32,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Memory categories and their temporal decay half-lives in days.
/// Returns None for categories that never decay (e.g., decisions).
pub fn decay_half_life(category: &str) -> Option<f64> {
    match category {
        "task" => Some(7.0),
        "context" => Some(14.0), // compacted conversation context
        "event" => Some(14.0),
        "fact" => Some(30.0),
        "reflection" => Some(60.0),
        "preference" => Some(90.0),
        "decision" => None, // never decays
        _ => Some(30.0),    // unknown categories decay like facts
    }
}

/// Apply temporal decay multiplier to a score.
/// Formula: score * 0.5^(age_days / half_life)
pub fn apply_decay(score: f64, age_days: f64, category: &str) -> f64 {
    match decay_half_life(category) {
        Some(half_life) => score * (-0.693 * age_days / half_life).exp(),
        None => score, // no decay
    }
}

impl Db {
    /// Store a memory entry with optional category and importance.
    /// If a key is provided and exists, update it.
    pub async fn memory_store(
        &self,
        key: Option<&str>,
        content: &str,
        tags: Option<&str>,
        source: Option<&str>,
    ) -> Result<i64, DbError> {
        self.memory_store_with_meta(key, content, tags, source, "fact", 5)
            .await
    }

    /// Store a memory entry with full metadata.
    pub async fn memory_store_with_meta(
        &self,
        key: Option<&str>,
        content: &str,
        tags: Option<&str>,
        source: Option<&str>,
        category: &str,
        importance: i32,
    ) -> Result<i64, DbError> {
        let key = key.map(|s| s.to_string());
        let content = content.to_string();
        let tags = tags.map(|s| s.to_string());
        let source = source.map(|s| s.to_string());
        let category = category.to_string();
        let ts = now_ms();
        self.exec(move |conn| {
            memory_store_sync(
                conn,
                key.as_deref(),
                &content,
                tags.as_deref(),
                source.as_deref(),
                &category,
                importance,
                ts,
            )
        })
        .await
    }

    /// Full-text search over memory with temporal decay applied.
    pub async fn memory_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, DbError> {
        let query = query.to_string();
        self.exec(move |conn| memory_search_sync(conn, &query, limit))
            .await
    }

    /// Get a memory entry by key.
    pub async fn memory_get(&self, key: &str) -> Result<Option<MemoryEntry>, DbError> {
        let key = key.to_string();
        self.exec(move |conn| memory_get_sync(conn, &key)).await
    }

    /// Delete a memory entry by ID.
    pub async fn memory_delete(&self, id: i64) -> Result<(), DbError> {
        self.exec(move |conn| {
            conn.execute("DELETE FROM memory WHERE id = ?1", rusqlite::params![id])?;

            // Clean up vector embedding if semantic feature is enabled
            #[cfg(feature = "semantic")]
            {
                if super::vector::vec_table_exists(conn) {
                    super::vector::vec_delete(conn, id).ok();
                }
            }

            Ok(())
        })
        .await
    }

    /// Store compacted conversation context as a memory entry (sync, for compaction).
    /// Called from `CompactionStrategy::compact()` which is sync. Uses `block_in_place`
    /// to signal the tokio runtime before blocking on the connection mutex.
    pub fn memory_store_compacted(
        &self,
        content: &str,
        source: &str,
        dropped_count: usize,
    ) -> Result<i64, DbError> {
        let ts = now_ms();
        let tags = format!("compaction,dropped:{}", dropped_count);
        tokio::task::block_in_place(|| {
            self.exec_sync(|conn| {
                memory_store_sync(
                    conn,
                    Some(source),
                    content,
                    Some(&tags),
                    Some(source),
                    "context",
                    3,
                    ts,
                )
            })
        })
    }

    /// Update access tracking for a set of memory IDs (called after search results are returned).
    pub async fn memory_touch(&self, ids: Vec<i64>) -> Result<(), DbError> {
        let ts = now_ms();
        self.exec(move |conn| {
            let mut stmt = conn.prepare(
                "UPDATE memory SET last_accessed = ?1, access_count = access_count + 1 WHERE id = ?2",
            )?;
            for id in ids {
                stmt.execute(rusqlite::params![ts as i64, id])?;
            }
            Ok(())
        })
        .await
    }
}

#[allow(clippy::too_many_arguments)]
fn memory_store_sync(
    conn: &Connection,
    key: Option<&str>,
    content: &str,
    tags: Option<&str>,
    source: Option<&str>,
    category: &str,
    importance: i32,
    ts: u64,
) -> Result<i64, DbError> {
    // If key exists, update
    if let Some(key) = key {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM memory WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok();
        if let Some(id) = existing {
            conn.execute(
                "UPDATE memory SET content = ?1, tags = ?2, source = ?3, category = ?4, importance = ?5, updated_at = ?6 WHERE id = ?7",
                rusqlite::params![content, tags, source, category, importance, ts as i64, id],
            )?;

            // Update embedding on content change
            #[cfg(feature = "semantic")]
            {
                if super::vector::vec_table_exists(conn) {
                    if let Ok(engine) = super::vector::EmbeddingEngine::global() {
                        match engine.embed(&[content]) {
                            Ok(embeddings) if !embeddings.is_empty() => {
                                super::vector::vec_insert(conn, id, &embeddings[0]).ok();
                            }
                            _ => {}
                        }
                    }
                }
            }

            return Ok(id);
        }
    }
    // Insert new
    conn.execute(
        "INSERT INTO memory (key, content, tags, source, category, importance, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        rusqlite::params![key, content, tags, source, category, importance, ts as i64],
    )?;
    let id = conn.last_insert_rowid();

    // Store embedding for vector search if semantic feature is enabled
    #[cfg(feature = "semantic")]
    {
        if super::vector::vec_table_exists(conn) {
            if let Ok(engine) = super::vector::EmbeddingEngine::global() {
                match engine.embed(&[content]) {
                    Ok(embeddings) if !embeddings.is_empty() => {
                        if let Err(e) = super::vector::vec_insert(conn, id, &embeddings[0]) {
                            tracing::warn!("Failed to store embedding for memory {}: {}", id, e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to compute embedding for memory {}: {}", id, e);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(id)
}

fn memory_search_sync(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>, DbError> {
    let fetch_limit = limit * 3; // over-fetch for re-ranking

    // 1. FTS5 search (with LIKE fallback)
    let safe_query = format!("\"{}\"", query.replace('"', "\"\""));
    let fts_entries = match memory_search_fts(conn, &safe_query, fetch_limit) {
        Ok(entries) => entries,
        Err(_) => memory_search_like(conn, query, fetch_limit)?,
    };

    // 2. Optionally run vector KNN search and merge with RRF
    #[cfg(feature = "semantic")]
    let (mut entries, rrf_scores) = {
        if super::vector::vec_table_exists(conn) {
            if let Ok(engine) = super::vector::EmbeddingEngine::global() {
                if let Ok(emb) = engine.embed(&[query]) {
                    if let Ok(vec_results) = super::vector::vec_search(conn, &emb[0], fetch_limit) {
                        // Build ranked lists: (id, rank)
                        let fts_ranked: Vec<(i64, usize)> = fts_entries
                            .iter()
                            .enumerate()
                            .filter_map(|(rank, e)| e.id.map(|id| (id, rank)))
                            .collect();
                        let vec_ranked: Vec<(i64, usize)> = vec_results
                            .iter()
                            .enumerate()
                            .map(|(rank, &(id, _))| (id, rank))
                            .collect();

                        // RRF merge
                        let merged = rrf_merge(&fts_ranked, &vec_ranked, 60.0);

                        // Build a lookup of existing FTS entries
                        let mut entry_map: HashMap<i64, MemoryEntry> = fts_entries
                            .into_iter()
                            .filter_map(|e| e.id.map(|id| (id, e)))
                            .collect();

                        // Load any vector-only results not in FTS
                        for &(id, _) in &merged {
                            if !entry_map.contains_key(&id) {
                                if let Ok(Some(entry)) = memory_get_by_id_sync(conn, id) {
                                    entry_map.insert(id, entry);
                                }
                            }
                        }

                        // Build RRF score lookup for decay weighting
                        let rrf_scores: HashMap<i64, f64> =
                            merged.iter().map(|&(id, score)| (id, score)).collect();

                        // Reorder by RRF score
                        let results: Vec<_> = merged
                            .into_iter()
                            .filter_map(|(id, _)| entry_map.remove(&id))
                            .collect();
                        (results, rrf_scores)
                    } else {
                        (fts_entries, HashMap::new())
                    }
                } else {
                    (fts_entries, HashMap::new())
                }
            } else {
                (fts_entries, HashMap::new())
            }
        } else {
            (fts_entries, HashMap::new())
        }
    };

    #[cfg(not(feature = "semantic"))]
    let mut entries = fts_entries;

    // 3. Apply temporal decay and re-rank (using RRF scores as base when available)
    let now = now_ms();
    entries.sort_by(|a, b| {
        let age_a = (now.saturating_sub(a.updated_at)) as f64 / (1000.0 * 60.0 * 60.0 * 24.0);
        let age_b = (now.saturating_sub(b.updated_at)) as f64 / (1000.0 * 60.0 * 60.0 * 24.0);
        #[cfg(feature = "semantic")]
        let (base_a, base_b) = (
            a.id.and_then(|id| rrf_scores.get(&id).copied())
                .unwrap_or(1.0),
            b.id.and_then(|id| rrf_scores.get(&id).copied())
                .unwrap_or(1.0),
        );
        #[cfg(not(feature = "semantic"))]
        let (base_a, base_b) = (1.0, 1.0);
        let score_a = apply_decay(base_a, age_a, &a.category);
        let score_b = apply_decay(base_b, age_b, &b.category);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    entries.truncate(limit);

    // Update access tracking for returned results
    let ids: Vec<i64> = entries.iter().filter_map(|e| e.id).collect();
    if !ids.is_empty() {
        let ts = now as i64;
        let mut stmt = conn.prepare(
            "UPDATE memory SET last_accessed = ?1, access_count = access_count + 1 WHERE id = ?2",
        )?;
        for id in &ids {
            stmt.execute(rusqlite::params![ts, id])?;
        }
    }

    Ok(entries)
}

fn memory_search_like(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>, DbError> {
    let pattern = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT id, key, content, tags, source, category, importance, last_accessed, access_count, created_at, updated_at
         FROM memory WHERE content LIKE ?1 ORDER BY updated_at DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![pattern, limit as i64], |row| {
            Ok(MemoryEntry {
                id: Some(row.get(0)?),
                key: row.get(1)?,
                content: row.get(2)?,
                tags: row.get(3)?,
                source: row.get(4)?,
                category: row
                    .get::<_, Option<String>>(5)?
                    .unwrap_or_else(|| "fact".to_string()),
                importance: row.get::<_, Option<i32>>(6)?.unwrap_or(5),
                last_accessed: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                access_count: row.get::<_, Option<i32>>(8)?.unwrap_or(0),
                created_at: row.get::<_, i64>(9)? as u64,
                updated_at: row.get::<_, i64>(10)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn memory_search_fts(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.key, m.content, m.tags, m.source, m.category, m.importance, m.last_accessed, m.access_count, m.created_at, m.updated_at
         FROM memory m
         JOIN memory_fts f ON m.id = f.rowid
         WHERE memory_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![query, limit as i64], |row| {
            Ok(MemoryEntry {
                id: Some(row.get(0)?),
                key: row.get(1)?,
                content: row.get(2)?,
                tags: row.get(3)?,
                source: row.get(4)?,
                category: row
                    .get::<_, Option<String>>(5)?
                    .unwrap_or_else(|| "fact".to_string()),
                importance: row.get::<_, Option<i32>>(6)?.unwrap_or(5),
                last_accessed: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                access_count: row.get::<_, Option<i32>>(8)?.unwrap_or(0),
                created_at: row.get::<_, i64>(9)? as u64,
                updated_at: row.get::<_, i64>(10)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(feature = "semantic")]
fn memory_get_by_id_sync(conn: &Connection, id: i64) -> Result<Option<MemoryEntry>, DbError> {
    let result = conn.query_row(
        "SELECT id, key, content, tags, source, category, importance, last_accessed, access_count, created_at, updated_at
         FROM memory WHERE id = ?1",
        rusqlite::params![id],
        |row| {
            Ok(MemoryEntry {
                id: Some(row.get(0)?),
                key: row.get(1)?,
                content: row.get(2)?,
                tags: row.get(3)?,
                source: row.get(4)?,
                category: row.get::<_, Option<String>>(5)?.unwrap_or_else(|| "fact".to_string()),
                importance: row.get::<_, Option<i32>>(6)?.unwrap_or(5),
                last_accessed: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                access_count: row.get::<_, Option<i32>>(8)?.unwrap_or(0),
                created_at: row.get::<_, i64>(9)? as u64,
                updated_at: row.get::<_, i64>(10)? as u64,
            })
        },
    );
    match result {
        Ok(entry) => Ok(Some(entry)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Reciprocal Rank Fusion: merge two ranked lists into a single score map.
/// `k` is the RRF constant (typically 60). Each list entry is (id, rank) where rank is 0-based.
/// Returns a map of id → RRF score, sorted descending by score.
pub fn rrf_merge(
    fts_ranked: &[(i64, usize)],
    vec_ranked: &[(i64, usize)],
    k: f64,
) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for &(id, rank) in fts_ranked {
        *scores.entry(id).or_default() += 1.0 / (k + rank as f64);
    }
    for &(id, rank) in vec_ranked {
        *scores.entry(id).or_default() += 1.0 / (k + rank as f64);
    }
    let mut result: Vec<(i64, f64)> = scores.into_iter().collect();
    result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    result
}

fn memory_get_sync(conn: &Connection, key: &str) -> Result<Option<MemoryEntry>, DbError> {
    let result = conn.query_row(
        "SELECT id, key, content, tags, source, category, importance, last_accessed, access_count, created_at, updated_at
         FROM memory WHERE key = ?1",
        rusqlite::params![key],
        |row| {
            Ok(MemoryEntry {
                id: Some(row.get(0)?),
                key: row.get(1)?,
                content: row.get(2)?,
                tags: row.get(3)?,
                source: row.get(4)?,
                category: row.get::<_, Option<String>>(5)?.unwrap_or_else(|| "fact".to_string()),
                importance: row.get::<_, Option<i32>>(6)?.unwrap_or(5),
                last_accessed: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                access_count: row.get::<_, Option<i32>>(8)?.unwrap_or(0),
                created_at: row.get::<_, i64>(9)? as u64,
                updated_at: row.get::<_, i64>(10)? as u64,
            })
        },
    );
    match result {
        Ok(entry) => Ok(Some(entry)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_get() {
        let db = Db::open_memory().unwrap();
        db.memory_store(Some("user_name"), "Alice", Some("preference"), Some("user"))
            .await
            .unwrap();

        let entry = db.memory_get("user_name").await.unwrap().unwrap();
        assert_eq!(entry.content, "Alice");
        assert_eq!(entry.tags.as_deref(), Some("preference"));
        assert_eq!(entry.category, "fact"); // default
        assert_eq!(entry.importance, 5); // default
    }

    #[tokio::test]
    async fn test_store_with_category() {
        let db = Db::open_memory().unwrap();
        db.memory_store_with_meta(
            Some("deploy_date"),
            "Deploy by Friday",
            Some("work"),
            Some("agent"),
            "task",
            8,
        )
        .await
        .unwrap();

        let entry = db.memory_get("deploy_date").await.unwrap().unwrap();
        assert_eq!(entry.content, "Deploy by Friday");
        assert_eq!(entry.category, "task");
        assert_eq!(entry.importance, 8);
    }

    #[tokio::test]
    async fn test_upsert_by_key() {
        let db = Db::open_memory().unwrap();
        db.memory_store(Some("k"), "v1", None, None).await.unwrap();
        db.memory_store(Some("k"), "v2", None, None).await.unwrap();

        let entry = db.memory_get("k").await.unwrap().unwrap();
        assert_eq!(entry.content, "v2");
    }

    #[tokio::test]
    async fn test_search() {
        let db = Db::open_memory().unwrap();
        db.memory_store(None, "The quick brown fox jumps", Some("animals"), None)
            .await
            .unwrap();
        db.memory_store(None, "A lazy dog sleeps", Some("animals"), None)
            .await
            .unwrap();
        db.memory_store(None, "Rust programming language", Some("tech"), None)
            .await
            .unwrap();

        let results = db.memory_search("fox", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("fox"));

        let results = db.memory_search("animals", 10).await.unwrap();
        assert!(results.len() >= 1);
    }

    #[tokio::test]
    async fn test_search_updates_access_tracking() {
        let db = Db::open_memory().unwrap();
        let id = db
            .memory_store(None, "The quick brown fox", None, None)
            .await
            .unwrap();

        // Before search, access_count should be 0
        let count_before = db
            .exec(move |conn| {
                let count: i32 = conn.query_row(
                    "SELECT access_count FROM memory WHERE id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )?;
                Ok(count)
            })
            .await
            .unwrap();
        assert_eq!(count_before, 0);

        // Search triggers access tracking update
        let results = db.memory_search("fox", 10).await.unwrap();
        assert_eq!(results.len(), 1);

        // Verify access_count was incremented in the database
        let count_after = db
            .exec(move |conn| {
                let count: i32 = conn.query_row(
                    "SELECT access_count FROM memory WHERE id = ?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )?;
                Ok(count)
            })
            .await
            .unwrap();
        assert_eq!(count_after, 1);
    }

    #[tokio::test]
    async fn test_delete() {
        let db = Db::open_memory().unwrap();
        let id = db
            .memory_store(Some("temp"), "temporary", None, None)
            .await
            .unwrap();
        db.memory_delete(id).await.unwrap();
        let entry = db.memory_get("temp").await.unwrap();
        assert!(entry.is_none());
    }

    #[tokio::test]
    async fn test_search_empty() {
        let db = Db::open_memory().unwrap();
        let results = db.memory_search("nonexistent", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_decay_half_lives() {
        assert_eq!(decay_half_life("task"), Some(7.0));
        assert_eq!(decay_half_life("context"), Some(14.0));
        assert_eq!(decay_half_life("preference"), Some(90.0));
        assert_eq!(decay_half_life("decision"), None);
    }

    #[test]
    fn test_apply_decay() {
        // A task 7 days old should decay to ~50%
        let score = apply_decay(1.0, 7.0, "task");
        assert!((score - 0.5).abs() < 0.01);

        // A decision never decays
        let score = apply_decay(1.0, 365.0, "decision");
        assert_eq!(score, 1.0);

        // A preference 90 days old should decay to ~50%
        let score = apply_decay(1.0, 90.0, "preference");
        assert!((score - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_rrf_merge() {
        // FTS returns docs [A=10, B=20, C=30] ranked 0,1,2
        // Vec returns docs [B=20, D=40, A=10] ranked 0,1,2
        let fts = vec![(10, 0), (20, 1), (30, 2)];
        let vec = vec![(20, 0), (40, 1), (10, 2)];

        let merged = rrf_merge(&fts, &vec, 60.0);

        // B (id=20) appears in both lists at ranks 1 and 0 → highest RRF score
        assert_eq!(merged[0].0, 20);

        // A (id=10) appears in both lists at ranks 0 and 2
        assert_eq!(merged[1].0, 10);

        // C and D each appear in only one list
        assert!(merged.len() == 4);

        // Verify RRF scores are positive
        for &(_, score) in &merged {
            assert!(score > 0.0);
        }
    }
}
