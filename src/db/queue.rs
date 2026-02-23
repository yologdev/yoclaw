use super::{now_ms, Db, DbError};
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub id: Option<i64>,
    pub channel: String,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub session_id: String,
    pub content: String,
    pub reply_to: Option<String>,
    pub status: QueueStatus,
    pub error_msg: Option<String>,
    pub created_at: u64,
    pub processed_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueStatus {
    Pending,
    Processing,
    Done,
    Failed,
}

impl QueueStatus {
    fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "processing" => Self::Processing,
            "done" => Self::Done,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

impl Db {
    /// Enqueue an incoming message. Returns the queue entry ID.
    pub async fn queue_push(&self, entry: &QueueEntry) -> Result<i64, DbError> {
        let entry = entry.clone();
        self.exec(move |conn| queue_push_sync(conn, &entry)).await
    }

    /// Atomically claim the next pending entry. Returns None if queue is empty.
    pub async fn queue_claim_next(&self) -> Result<Option<QueueEntry>, DbError> {
        self.exec(queue_claim_sync).await
    }

    /// Mark an entry as done.
    pub async fn queue_mark_done(&self, id: i64) -> Result<(), DbError> {
        let ts = now_ms();
        self.exec(move |conn| {
            conn.execute(
                "UPDATE queue SET status = 'done', processed_at = ?1 WHERE id = ?2",
                rusqlite::params![ts as i64, id],
            )?;
            Ok(())
        })
        .await
    }

    /// Mark an entry as failed with an error message.
    pub async fn queue_mark_failed(&self, id: i64, error: &str) -> Result<(), DbError> {
        let error = error.to_string();
        let ts = now_ms();
        self.exec(move |conn| {
            conn.execute(
                "UPDATE queue SET status = 'failed', error_msg = ?1, processed_at = ?2 WHERE id = ?3",
                rusqlite::params![error, ts as i64, id],
            )?;
            Ok(())
        })
        .await
    }

    /// Crash recovery: reset any 'processing' entries back to 'pending'.
    /// Returns the number of requeued entries.
    pub async fn queue_requeue_stale(&self) -> Result<usize, DbError> {
        self.exec(|conn| {
            let count = conn.execute(
                "UPDATE queue SET status = 'pending' WHERE status = 'processing'",
                [],
            )?;
            Ok(count)
        })
        .await
    }

    /// Count pending entries.
    pub async fn queue_pending_count(&self) -> Result<usize, DbError> {
        self.exec(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM queue WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )?;
            Ok(count as usize)
        })
        .await
    }
}

fn queue_push_sync(conn: &Connection, entry: &QueueEntry) -> Result<i64, DbError> {
    conn.execute(
        "INSERT INTO queue (channel, sender_id, sender_name, session_id, content, reply_to, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            entry.channel,
            entry.sender_id,
            entry.sender_name,
            entry.session_id,
            entry.content,
            entry.reply_to,
            entry.status.as_str(),
            entry.created_at as i64,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn queue_claim_sync(conn: &Connection) -> Result<Option<QueueEntry>, DbError> {
    let tx = conn.unchecked_transaction()?;
    let result = tx.query_row(
        "SELECT id, channel, sender_id, sender_name, session_id, content, reply_to, status, error_msg, created_at, processed_at
         FROM queue WHERE status = 'pending' ORDER BY created_at ASC LIMIT 1",
        [],
        |row| {
            Ok(QueueEntry {
                id: Some(row.get(0)?),
                channel: row.get(1)?,
                sender_id: row.get(2)?,
                sender_name: row.get(3)?,
                session_id: row.get(4)?,
                content: row.get(5)?,
                reply_to: row.get(6)?,
                status: QueueStatus::from_str(&row.get::<_, String>(7)?),
                error_msg: row.get(8)?,
                created_at: row.get::<_, i64>(9)? as u64,
                processed_at: row.get::<_, Option<i64>>(10)?.map(|v| v as u64),
            })
        },
    );
    match result {
        Ok(mut entry) => {
            tx.execute(
                "UPDATE queue SET status = 'processing' WHERE id = ?1",
                rusqlite::params![entry.id.unwrap()],
            )?;
            tx.commit()?;
            entry.status = QueueStatus::Processing;
            Ok(Some(entry))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            tx.commit()?;
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

impl QueueEntry {
    /// Create a new pending queue entry.
    pub fn new(channel: &str, sender_id: &str, session_id: &str, content: &str) -> Self {
        Self {
            id: None,
            channel: channel.to_string(),
            sender_id: sender_id.to_string(),
            sender_name: None,
            session_id: session_id.to_string(),
            content: content.to_string(),
            reply_to: None,
            status: QueueStatus::Pending,
            error_msg: None,
            created_at: now_ms(),
            processed_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_push_and_claim() {
        let db = Db::open_memory().unwrap();
        let entry = QueueEntry::new("telegram", "user1", "tg-123", "hello");
        let id = db.queue_push(&entry).await.unwrap();
        assert!(id > 0);

        let claimed = db.queue_claim_next().await.unwrap().unwrap();
        assert_eq!(claimed.id, Some(id));
        assert_eq!(claimed.content, "hello");
        assert_eq!(claimed.status, QueueStatus::Processing);

        // No more pending
        let next = db.queue_claim_next().await.unwrap();
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn test_mark_done() {
        let db = Db::open_memory().unwrap();
        let entry = QueueEntry::new("tg", "u1", "s1", "msg");
        let id = db.queue_push(&entry).await.unwrap();
        db.queue_claim_next().await.unwrap();
        db.queue_mark_done(id).await.unwrap();

        let pending = db.queue_pending_count().await.unwrap();
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn test_mark_failed() {
        let db = Db::open_memory().unwrap();
        let entry = QueueEntry::new("tg", "u1", "s1", "msg");
        let id = db.queue_push(&entry).await.unwrap();
        db.queue_claim_next().await.unwrap();
        db.queue_mark_failed(id, "something broke").await.unwrap();
    }

    #[tokio::test]
    async fn test_requeue_stale() {
        let db = Db::open_memory().unwrap();
        let entry = QueueEntry::new("tg", "u1", "s1", "msg");
        db.queue_push(&entry).await.unwrap();
        db.queue_claim_next().await.unwrap(); // now 'processing'

        let requeued = db.queue_requeue_stale().await.unwrap();
        assert_eq!(requeued, 1);

        // Should be claimable again
        let reclaimed = db.queue_claim_next().await.unwrap();
        assert!(reclaimed.is_some());
    }

    #[tokio::test]
    async fn test_fifo_ordering() {
        let db = Db::open_memory().unwrap();
        db.queue_push(&QueueEntry::new("tg", "u1", "s1", "first"))
            .await
            .unwrap();
        db.queue_push(&QueueEntry::new("tg", "u1", "s1", "second"))
            .await
            .unwrap();

        let first = db.queue_claim_next().await.unwrap().unwrap();
        assert_eq!(first.content, "first");
        let second = db.queue_claim_next().await.unwrap().unwrap();
        assert_eq!(second.content, "second");
    }
}
