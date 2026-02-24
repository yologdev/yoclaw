use super::{now_ms, Db, DbError};
use rusqlite::Connection;
use yoagent::AgentMessage;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub message_count: usize,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Db {
    /// Save (upsert) the full message list for a session.
    pub async fn tape_save_messages(
        &self,
        session_id: &str,
        messages: &[AgentMessage],
    ) -> Result<(), DbError> {
        let session_id = session_id.to_string();
        let json = serde_json::to_string(messages)?;
        let count = messages.len();
        let ts = now_ms();
        self.exec(move |conn| tape_save_sync(conn, &session_id, &json, count, ts))
            .await
    }

    /// Load messages for a session. Returns empty vec if session not found.
    pub async fn tape_load_messages(&self, session_id: &str) -> Result<Vec<AgentMessage>, DbError> {
        let session_id = session_id.to_string();
        self.exec(move |conn| tape_load_sync(conn, &session_id))
            .await
    }

    /// List all sessions.
    pub async fn tape_list_sessions(&self) -> Result<Vec<SessionInfo>, DbError> {
        self.exec(tape_list_sync).await
    }
}

fn tape_save_sync(
    conn: &Connection,
    session_id: &str,
    json: &str,
    count: usize,
    ts: u64,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO tape (session_id, messages_json, message_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(session_id) DO UPDATE SET
             messages_json = excluded.messages_json,
             message_count = excluded.message_count,
             updated_at = excluded.updated_at",
        rusqlite::params![session_id, json, count as i64, ts as i64],
    )?;
    Ok(())
}

fn tape_load_sync(conn: &Connection, session_id: &str) -> Result<Vec<AgentMessage>, DbError> {
    let mut stmt = conn.prepare("SELECT messages_json FROM tape WHERE session_id = ?1")?;
    let result = stmt.query_row(rusqlite::params![session_id], |row| {
        let json: String = row.get(0)?;
        Ok(json)
    });
    match result {
        Ok(json) => {
            let messages: Vec<AgentMessage> = serde_json::from_str(&json)?;
            Ok(messages)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn tape_list_sync(conn: &Connection) -> Result<Vec<SessionInfo>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT session_id, message_count, created_at, updated_at FROM tape ORDER BY updated_at DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SessionInfo {
                session_id: row.get(0)?,
                message_count: row.get::<_, i64>(1)? as usize,
                created_at: row.get::<_, i64>(2)? as u64,
                updated_at: row.get::<_, i64>(3)? as u64,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoagent::types::{Content, Message, StopReason, Usage};

    fn sample_messages() -> Vec<AgentMessage> {
        vec![
            AgentMessage::Llm(Message::user("Hello")),
            AgentMessage::Llm(Message::Assistant {
                content: vec![Content::Text {
                    text: "Hi there!".into(),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 123,
                error_message: None,
            }),
        ]
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let db = Db::open_memory().unwrap();
        let msgs = sample_messages();
        db.tape_save_messages("session-1", &msgs).await.unwrap();

        let loaded = db.tape_load_messages("session-1").await.unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let db = Db::open_memory().unwrap();
        let loaded = db.tape_load_messages("no-such-session").await.unwrap();
        assert!(loaded.is_empty());
    }

    #[tokio::test]
    async fn test_upsert() {
        let db = Db::open_memory().unwrap();
        let msgs1 = vec![AgentMessage::Llm(Message::user("first"))];
        db.tape_save_messages("s1", &msgs1).await.unwrap();

        let msgs2 = sample_messages();
        db.tape_save_messages("s1", &msgs2).await.unwrap();

        let loaded = db.tape_load_messages("s1").await.unwrap();
        assert_eq!(loaded.len(), 2); // replaced, not appended
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let db = Db::open_memory().unwrap();
        db.tape_save_messages("a", &sample_messages())
            .await
            .unwrap();
        db.tape_save_messages("b", &sample_messages())
            .await
            .unwrap();

        let sessions = db.tape_list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].message_count, 2);
    }
}
