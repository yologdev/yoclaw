use super::{now_ms, Db, DbError};

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub id: Option<i64>,
    pub session_id: Option<String>,
    pub event_type: String,
    pub tool_name: Option<String>,
    pub detail: Option<String>,
    pub tokens_used: u64,
    pub timestamp: u64,
}

impl Db {
    /// Log an audit event.
    pub async fn audit_log(
        &self,
        session_id: Option<&str>,
        event_type: &str,
        tool_name: Option<&str>,
        detail: Option<&str>,
        tokens_used: u64,
    ) -> Result<(), DbError> {
        let session_id = session_id.map(|s| s.to_string());
        let event_type = event_type.to_string();
        let tool_name = tool_name.map(|s| s.to_string());
        let detail = detail.map(|s| s.to_string());
        let ts = now_ms();
        self.exec(move |conn| {
            conn.execute(
                "INSERT INTO audit (session_id, event_type, tool_name, detail, tokens_used, timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    session_id,
                    event_type,
                    tool_name,
                    detail,
                    tokens_used as i64,
                    ts as i64,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Query audit entries, optionally filtered by session.
    pub async fn audit_query(
        &self,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AuditEntry>, DbError> {
        let session_id = session_id.map(|s| s.to_string());
        self.exec(move |conn| {
            let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match &session_id {
                Some(sid) => (
                    "SELECT id, session_id, event_type, tool_name, detail, tokens_used, timestamp
                     FROM audit WHERE session_id = ?1 ORDER BY timestamp DESC LIMIT ?2",
                    vec![
                        Box::new(sid.clone()) as Box<dyn rusqlite::types::ToSql>,
                        Box::new(limit as i64),
                    ],
                ),
                None => (
                    "SELECT id, session_id, event_type, tool_name, detail, tokens_used, timestamp
                     FROM audit ORDER BY timestamp DESC LIMIT ?1",
                    vec![Box::new(limit as i64) as Box<dyn rusqlite::types::ToSql>],
                ),
            };
            let mut stmt = conn.prepare(sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok(AuditEntry {
                        id: Some(row.get(0)?),
                        session_id: row.get(1)?,
                        event_type: row.get(2)?,
                        tool_name: row.get(3)?,
                        detail: row.get(4)?,
                        tokens_used: row.get::<_, i64>(5)? as u64,
                        timestamp: row.get::<_, i64>(6)? as u64,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Sum token usage for today (since midnight UTC).
    pub async fn audit_token_usage_today(&self) -> Result<u64, DbError> {
        self.exec(|conn| {
            let today_start = today_start_ms();
            let total: i64 = conn.query_row(
                "SELECT COALESCE(SUM(tokens_used), 0) FROM audit WHERE timestamp >= ?1",
                rusqlite::params![today_start as i64],
                |r| r.get(0),
            )?;
            Ok(total as u64)
        })
        .await
    }
}

/// Milliseconds since epoch at start of today (UTC).
fn today_start_ms() -> u64 {
    let now = chrono::Utc::now();
    let today = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
    today.and_utc().timestamp_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_log_and_query() {
        let db = Db::open_memory().unwrap();
        db.audit_log(Some("s1"), "tool_call", Some("bash"), Some("ls -la"), 100)
            .await
            .unwrap();
        db.audit_log(Some("s1"), "tool_call", Some("read_file"), None, 50)
            .await
            .unwrap();
        db.audit_log(Some("s2"), "denied", Some("shell"), Some("rm -rf /"), 0)
            .await
            .unwrap();

        let all = db.audit_query(None, 100).await.unwrap();
        assert_eq!(all.len(), 3);

        let s1 = db.audit_query(Some("s1"), 100).await.unwrap();
        assert_eq!(s1.len(), 2);
    }

    #[tokio::test]
    async fn test_token_usage_today() {
        let db = Db::open_memory().unwrap();
        db.audit_log(Some("s1"), "usage", None, None, 1000)
            .await
            .unwrap();
        db.audit_log(Some("s1"), "usage", None, None, 500)
            .await
            .unwrap();

        let total = db.audit_token_usage_today().await.unwrap();
        assert_eq!(total, 1500);
    }
}
