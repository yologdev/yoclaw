pub mod audit;
pub mod memory;
pub mod queue;
pub mod tape;
#[cfg(feature = "semantic")]
pub mod vector;

use rusqlite::Connection;
use rusqlite::OptionalExtension;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Lock poisoned")]
    LockPoisoned,
    #[error("Join error: {0}")]
    JoinError(String),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Database handle. Clone-safe (wraps Arc<Mutex<Connection>>).
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Open a file-backed database with WAL mode.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)?;
        Self::configure_and_migrate(conn)
    }

    /// Open an in-memory database (for tests).
    pub fn open_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        Self::configure_and_migrate(conn)
    }

    fn configure_and_migrate(conn: Connection) -> Result<Self, DbError> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;

        // Load sqlite-vec extension and create vector table if available
        #[cfg(feature = "semantic")]
        {
            vector::load_sqlite_vec(&conn).ok();
            vector::create_vec_table(&conn).ok();
        }

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.run_migrations()?;
        Ok(db)
    }

    /// Execute a blocking DB operation on a spawn_blocking thread.
    pub async fn exec<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().map_err(|_| DbError::LockPoisoned)?;
            f(&conn)
        })
        .await
        .map_err(|e| DbError::JoinError(e.to_string()))?
    }

    /// Execute a blocking DB operation synchronously (for non-async contexts like tests).
    pub fn exec_sync<F, T>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> Result<T, DbError>,
    {
        let conn = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        f(&conn)
    }

    // -- Migrations --

    const MIGRATIONS: &[(&str, &str)] = &[
        (
            "001_initial",
            include_str!("../../migrations/001_initial.sql"),
        ),
        (
            "002_vector_memory",
            include_str!("../../migrations/002_vector_memory.sql"),
        ),
        (
            "003_scheduler",
            include_str!("../../migrations/003_scheduler.sql"),
        ),
        (
            "004_saved_workers",
            include_str!("../../migrations/004_saved_workers.sql"),
        ),
    ];

    fn run_migrations(&self) -> Result<(), DbError> {
        let conn = self.conn.lock().map_err(|_| DbError::LockPoisoned)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at INTEGER NOT NULL
            );",
        )?;
        let current: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )?;
        for (i, (name, sql)) in Self::MIGRATIONS.iter().enumerate() {
            let version = (i + 1) as i64;
            if version > current {
                conn.execute_batch(sql)?;
                conn.execute(
                    "INSERT INTO schema_version (version, name, applied_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![version, name, now_ms() as i64],
                )?;
                tracing::info!("Applied migration {}: {}", version, name);
            }
        }
        Ok(())
    }
}

// -- Saved workers --

/// A saved dynamic worker definition.
#[derive(Debug, Clone)]
pub struct SavedWorker {
    pub name: String,
    pub system_prompt: String,
    pub created_at: u64,
}

impl Db {
    /// List all saved workers.
    pub async fn saved_workers_list(&self) -> Result<Vec<SavedWorker>, DbError> {
        self.exec(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name, system_prompt, created_at FROM saved_workers ORDER BY name",
            )?;
            let workers = stmt
                .query_map([], |row| {
                    Ok(SavedWorker {
                        name: row.get(0)?,
                        system_prompt: row.get(1)?,
                        created_at: row.get::<_, i64>(2)? as u64,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(workers)
        })
        .await
    }

    /// Get a saved worker by name.
    pub async fn saved_workers_get(&self, name: &str) -> Result<Option<SavedWorker>, DbError> {
        let name = name.to_string();
        self.exec(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT name, system_prompt, created_at FROM saved_workers WHERE name = ?1",
            )?;
            let worker = stmt
                .query_row(rusqlite::params![name], |row| {
                    Ok(SavedWorker {
                        name: row.get(0)?,
                        system_prompt: row.get(1)?,
                        created_at: row.get::<_, i64>(2)? as u64,
                    })
                })
                .optional()?;
            Ok(worker)
        })
        .await
    }

    /// Upsert a saved worker.
    pub async fn saved_workers_upsert(
        &self,
        name: &str,
        system_prompt: &str,
    ) -> Result<(), DbError> {
        let name = name.to_string();
        let system_prompt = system_prompt.to_string();
        let now = now_ms() as i64;
        self.exec(move |conn| {
            conn.execute(
                "INSERT INTO saved_workers (name, system_prompt, created_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(name) DO UPDATE SET system_prompt = excluded.system_prompt",
                rusqlite::params![name, system_prompt, now],
            )?;
            Ok(())
        })
        .await
    }

    /// Remove a saved worker by name. Returns true if deleted.
    pub async fn saved_workers_remove(&self, name: &str) -> Result<bool, DbError> {
        let name = name.to_string();
        self.exec(move |conn| {
            let rows = conn.execute(
                "DELETE FROM saved_workers WHERE name = ?1",
                rusqlite::params![name],
            )?;
            Ok(rows > 0)
        })
        .await
    }
}

/// Current time in milliseconds since epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_memory() {
        let db = Db::open_memory().unwrap();
        // Verify tables exist
        db.exec_sync(|conn| {
            let tables: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            assert!(tables.contains(&"tape".to_string()));
            assert!(tables.contains(&"queue".to_string()));
            assert!(tables.contains(&"memory".to_string()));
            assert!(tables.contains(&"audit".to_string()));
            assert!(tables.contains(&"state".to_string()));
            assert!(tables.contains(&"schema_version".to_string()));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn test_migrations_idempotent() {
        let db = Db::open_memory().unwrap();
        // Running migrations again should be a no-op
        db.run_migrations().unwrap();
        db.exec_sync(|conn| {
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))?;
            assert_eq!(count, 4); // 001_initial + 002_vector_memory + 003_scheduler + 004_saved_workers
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn test_async_exec() {
        let db = Db::open_memory().unwrap();
        let result = db
            .exec(|conn| {
                let val: i64 = conn.query_row("SELECT 42", [], |r| r.get(0))?;
                Ok(val)
            })
            .await
            .unwrap();
        assert_eq!(result, 42);
    }
}
