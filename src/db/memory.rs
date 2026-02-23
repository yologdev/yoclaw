use super::{now_ms, Db, DbError};
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: Option<i64>,
    pub key: Option<String>,
    pub content: String,
    pub tags: Option<String>,
    pub source: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Db {
    /// Store a memory entry. If a key is provided and exists, update it.
    pub async fn memory_store(
        &self,
        key: Option<&str>,
        content: &str,
        tags: Option<&str>,
        source: Option<&str>,
    ) -> Result<i64, DbError> {
        let key = key.map(|s| s.to_string());
        let content = content.to_string();
        let tags = tags.map(|s| s.to_string());
        let source = source.map(|s| s.to_string());
        let ts = now_ms();
        self.exec(move |conn| {
            memory_store_sync(conn, key.as_deref(), &content, tags.as_deref(), source.as_deref(), ts)
        })
        .await
    }

    /// Full-text search over memory.
    pub async fn memory_search(&self, query: &str, limit: usize) -> Result<Vec<MemoryEntry>, DbError> {
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
            Ok(())
        })
        .await
    }
}

fn memory_store_sync(
    conn: &Connection,
    key: Option<&str>,
    content: &str,
    tags: Option<&str>,
    source: Option<&str>,
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
                "UPDATE memory SET content = ?1, tags = ?2, source = ?3, updated_at = ?4 WHERE id = ?5",
                rusqlite::params![content, tags, source, ts as i64, id],
            )?;
            return Ok(id);
        }
    }
    // Insert new
    conn.execute(
        "INSERT INTO memory (key, content, tags, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        rusqlite::params![key, content, tags, source, ts as i64],
    )?;
    Ok(conn.last_insert_rowid())
}

fn memory_search_sync(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>, DbError> {
    // Wrap in double quotes for safe literal matching
    let safe_query = format!("\"{}\"", query.replace('"', "\"\""));
    let result = memory_search_fts(conn, &safe_query, limit);
    match result {
        Ok(entries) => Ok(entries),
        Err(_) => {
            // FTS5 query failed — fall back to LIKE search
            let pattern = format!("%{}%", query);
            let mut stmt = conn.prepare(
                "SELECT id, key, content, tags, source, created_at, updated_at
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
                        created_at: row.get::<_, i64>(5)? as u64,
                        updated_at: row.get::<_, i64>(6)? as u64,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }
    }
}

fn memory_search_fts(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryEntry>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.key, m.content, m.tags, m.source, m.created_at, m.updated_at
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
                created_at: row.get::<_, i64>(5)? as u64,
                updated_at: row.get::<_, i64>(6)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn memory_get_sync(conn: &Connection, key: &str) -> Result<Option<MemoryEntry>, DbError> {
    let result = conn.query_row(
        "SELECT id, key, content, tags, source, created_at, updated_at
         FROM memory WHERE key = ?1",
        rusqlite::params![key],
        |row| {
            Ok(MemoryEntry {
                id: Some(row.get(0)?),
                key: row.get(1)?,
                content: row.get(2)?,
                tags: row.get(3)?,
                source: row.get(4)?,
                created_at: row.get::<_, i64>(5)? as u64,
                updated_at: row.get::<_, i64>(6)? as u64,
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
        // FTS5 searches content and tags
        assert!(results.len() >= 1);
    }

    #[tokio::test]
    async fn test_delete() {
        let db = Db::open_memory().unwrap();
        let id = db.memory_store(Some("temp"), "temporary", None, None)
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
}
