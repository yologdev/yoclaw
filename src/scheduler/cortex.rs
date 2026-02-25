//! Cortex maintenance tasks: memory deduplication, stale cleanup, consolidation,
//! session indexing, and daily briefing generation.

use super::AgentRunConfig;
use crate::db::{now_ms, Db, DbError};
use yoagent::types::{AgentMessage, Content, Message};

/// Run all cortex maintenance tasks. Returns a summary string.
pub async fn run_maintenance(db: &Db, agent_config: &AgentRunConfig) -> Result<String, DbError> {
    let mut actions = Vec::new();

    // 1. Stale memory cleanup: entries not accessed in 90+ days with low importance
    let stale_cleaned = cleanup_stale_memories(db).await?;
    if stale_cleaned > 0 {
        actions.push(format!("cleaned {} stale memories", stale_cleaned));
    }

    // 2. Memory deduplication: entries with identical content
    let deduped = deduplicate_memories(db).await?;
    if deduped > 0 {
        actions.push(format!("removed {} duplicate memories", deduped));
    }

    // 3. Memory consolidation: extract durable facts from recent conversations
    match consolidate_memories(db, agent_config).await {
        Ok(count) => {
            if count > 0 {
                actions.push(format!("consolidated {} new memories", count));
            }
        }
        Err(e) => {
            tracing::warn!("Memory consolidation failed: {}", e);
        }
    }

    // 4. Session indexing: summarize recent sessions into searchable entries
    match index_recent_sessions(db, agent_config).await {
        Ok(count) => {
            if count > 0 {
                actions.push(format!("indexed {} sessions", count));
            }
        }
        Err(e) => {
            tracing::warn!("Session indexing failed: {}", e);
        }
    }

    if actions.is_empty() {
        Ok("no maintenance needed".to_string())
    } else {
        Ok(actions.join(", "))
    }
}

/// Remove memory entries not accessed in 90+ days with importance <= 3.
async fn cleanup_stale_memories(db: &Db) -> Result<usize, DbError> {
    let now = now_ms();
    let ninety_days_ms: u64 = 90 * 24 * 60 * 60 * 1000;
    let cutoff = now.saturating_sub(ninety_days_ms) as i64;

    db.exec(move |conn| {
        // Clean up vector embeddings before deleting memories
        #[cfg(feature = "semantic")]
        {
            if crate::db::vector::vec_table_exists(conn) {
                let mut stmt = conn.prepare(
                    "SELECT id FROM memory WHERE importance <= 3
                     AND (last_accessed IS NOT NULL AND last_accessed < ?1)
                     AND category != 'decision'",
                )?;
                let ids: Vec<i64> = stmt
                    .query_map(rusqlite::params![cutoff], |r| r.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                for id in &ids {
                    crate::db::vector::vec_delete(conn, *id).ok();
                }
            }
        }

        let deleted = conn.execute(
            "DELETE FROM memory WHERE importance <= 3
             AND (last_accessed IS NOT NULL AND last_accessed < ?1)
             AND category != 'decision'",
            rusqlite::params![cutoff],
        )?;
        Ok(deleted)
    })
    .await
}

/// Remove exact duplicate memory entries (keep the most recently updated).
async fn deduplicate_memories(db: &Db) -> Result<usize, DbError> {
    db.exec(|conn| {
        // Clean up vector embeddings before deleting duplicate memories
        #[cfg(feature = "semantic")]
        {
            if crate::db::vector::vec_table_exists(conn) {
                let mut stmt = conn.prepare(
                    "SELECT id FROM memory WHERE id NOT IN (
                        SELECT MAX(id) FROM memory GROUP BY content
                    )",
                )?;
                let ids: Vec<i64> = stmt
                    .query_map([], |r| r.get(0))?
                    .filter_map(|r| r.ok())
                    .collect();
                for id in &ids {
                    crate::db::vector::vec_delete(conn, *id).ok();
                }
            }
        }

        let deleted = conn.execute(
            "DELETE FROM memory WHERE id NOT IN (
                SELECT MAX(id) FROM memory GROUP BY content
            )",
            [],
        )?;
        Ok(deleted)
    })
    .await
}

/// Extract durable facts from recent conversations and store them as memories.
/// Looks at sessions updated in the last 24 hours that haven't been consolidated yet.
async fn consolidate_memories(
    db: &Db,
    agent_config: &AgentRunConfig,
) -> Result<usize, anyhow::Error> {
    // Get sessions updated in the last 24 hours
    let sessions = db.tape_list_sessions().await?;
    let now = now_ms();
    let one_day_ms = 24 * 60 * 60 * 1000;
    let cutoff = now.saturating_sub(one_day_ms);

    let recent: Vec<_> = sessions
        .into_iter()
        .filter(|s| s.updated_at >= cutoff && s.message_count >= 4)
        .collect();

    if recent.is_empty() {
        return Ok(0);
    }

    // Check which sessions have already been consolidated (via state table)
    let mut to_consolidate = Vec::new();
    for session in &recent {
        let sid = session.session_id.clone();
        let key = format!("cortex_consolidated:{}", sid);
        let already_done = db
            .exec(move |conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM state WHERE key = ?1",
                    rusqlite::params![key],
                    |r| r.get(0),
                )?;
                Ok(count > 0)
            })
            .await?;
        if !already_done {
            to_consolidate.push(session.clone());
        }
    }

    if to_consolidate.is_empty() {
        return Ok(0);
    }

    let mut total_stored = 0;

    for session in to_consolidate.iter().take(3) {
        // Limit to 3 sessions per run
        let messages = db.tape_load_messages(&session.session_id).await?;
        if messages.is_empty() {
            continue;
        }

        // Build a summary of the conversation for the LLM
        let conversation_text = extract_conversation_text(&messages, 3000);
        if conversation_text.is_empty() {
            continue;
        }

        let prompt = format!(
            "Analyze this conversation and extract 1-3 durable facts worth remembering long-term. \
             For each fact, output one line in the format: FACT: <the fact>\n\
             Only include facts that are genuinely useful to remember (user preferences, decisions, \
             project details, important context). Skip trivial or ephemeral information.\n\
             If nothing is worth remembering, output: NONE\n\n\
             Conversation:\n{}",
            conversation_text
        );

        match super::run_ephemeral_prompt(
            agent_config,
            "You extract key facts from conversations. Be concise. Output only FACT: lines or NONE.",
            &prompt,
        )
        .await
        {
            Ok(response) => {
                let facts: Vec<&str> = response
                    .lines()
                    .filter_map(|line| line.strip_prefix("FACT: "))
                    .collect();

                for fact in &facts {
                    if !fact.trim().is_empty() {
                        db.memory_store_with_meta(
                            None,
                            fact.trim(),
                            None,
                            Some(&format!("cortex:{}", session.session_id)),
                            "fact",
                            6, // medium-high importance
                        )
                        .await?;
                        total_stored += 1;
                    }
                }

                // Mark session as consolidated
                let key = format!("cortex_consolidated:{}", session.session_id);
                let ts = now_ms() as i64;
                db.exec(move |conn| {
                    conn.execute(
                        "INSERT OR REPLACE INTO state (key, value) VALUES (?1, ?2)",
                        rusqlite::params![key, ts.to_string()],
                    )?;
                    Ok(())
                })
                .await?;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to consolidate session '{}': {}",
                    session.session_id,
                    e
                );
            }
        }
    }

    Ok(total_stored)
}

/// Summarize recent sessions into searchable memory entries (category: reflection).
async fn index_recent_sessions(
    db: &Db,
    agent_config: &AgentRunConfig,
) -> Result<usize, anyhow::Error> {
    let sessions = db.tape_list_sessions().await?;
    let now = now_ms();
    let one_day_ms = 24 * 60 * 60 * 1000;
    let cutoff = now.saturating_sub(one_day_ms);

    let recent: Vec<_> = sessions
        .into_iter()
        .filter(|s| s.updated_at >= cutoff && s.message_count >= 2)
        .collect();

    if recent.is_empty() {
        return Ok(0);
    }

    let mut indexed = 0;

    for session in recent.iter().take(5) {
        let key = format!("session_index:{}", session.session_id);

        // Skip if already indexed
        let already = db
            .exec({
                let key = key.clone();
                move |conn| {
                    let count: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM state WHERE key = ?1",
                        rusqlite::params![key],
                        |r| r.get(0),
                    )?;
                    Ok(count > 0)
                }
            })
            .await?;
        if already {
            continue;
        }

        let messages = db.tape_load_messages(&session.session_id).await?;
        if messages.is_empty() {
            continue;
        }

        let conversation_text = extract_conversation_text(&messages, 2000);
        if conversation_text.is_empty() {
            continue;
        }

        let prompt = format!(
            "Summarize this conversation in 1-2 sentences. Focus on the topic and outcome.\n\n{}",
            conversation_text
        );

        match super::run_ephemeral_prompt(
            agent_config,
            "You summarize conversations concisely. Output a brief summary only.",
            &prompt,
        )
        .await
        {
            Ok(summary) => {
                let content = format!("Session {} summary: {}", session.session_id, summary.trim());
                db.memory_store_with_meta(
                    Some(&key),
                    &content,
                    None,
                    Some("cortex:indexer"),
                    "reflection",
                    4,
                )
                .await?;

                // Mark as indexed
                let ts = now_ms() as i64;
                db.exec({
                    let key = key.clone();
                    move |conn| {
                        conn.execute(
                            "INSERT OR REPLACE INTO state (key, value) VALUES (?1, ?2)",
                            rusqlite::params![key, ts.to_string()],
                        )?;
                        Ok(())
                    }
                })
                .await?;

                indexed += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to index session '{}': {}", session.session_id, e);
            }
        }
    }

    Ok(indexed)
}

/// Extract readable text from conversation messages, truncated to max_chars.
fn extract_conversation_text(messages: &[AgentMessage], max_chars: usize) -> String {
    let mut text = String::new();

    for msg in messages {
        let (role, content) = match msg {
            AgentMessage::Llm(Message::User { content, .. }) => ("User", content),
            AgentMessage::Llm(Message::Assistant { content, .. }) => ("Assistant", content),
            _ => continue,
        };

        for c in content {
            if let Content::Text { text: t } = c {
                let line = format!("{}: {}\n", role, t);
                if text.len() + line.len() > max_chars {
                    return text;
                }
                text.push_str(&line);
            }
        }
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::AgentRunConfig;

    fn test_agent_config() -> AgentRunConfig {
        AgentRunConfig {
            provider: "anthropic".to_string(),
            model: "mock".to_string(),
            api_key: "test-key".to_string(),
        }
    }

    #[tokio::test]
    async fn test_cleanup_stale_memories() {
        let db = Db::open_memory().unwrap();

        // Insert a low-importance memory with old access time
        let old_ts = (now_ms() - 100 * 24 * 60 * 60 * 1000) as i64; // 100 days ago
        db.exec(move |conn| {
            conn.execute(
                "INSERT INTO memory (content, source, category, importance, last_accessed, created_at, updated_at)
                 VALUES ('old data', 'test', 'fact', 2, ?1, ?1, ?1)",
                rusqlite::params![old_ts],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Insert a high-importance memory with old access time (should NOT be cleaned)
        db.exec(move |conn| {
            conn.execute(
                "INSERT INTO memory (content, source, category, importance, last_accessed, created_at, updated_at)
                 VALUES ('important data', 'test', 'fact', 8, ?1, ?1, ?1)",
                rusqlite::params![old_ts],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let cleaned = cleanup_stale_memories(&db).await.unwrap();
        assert_eq!(cleaned, 1);

        // Verify the important one remains
        let count = db
            .exec(|conn| {
                let c: i64 = conn.query_row("SELECT COUNT(*) FROM memory", [], |r| r.get(0))?;
                Ok(c)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_deduplicate_memories() {
        let db = Db::open_memory().unwrap();
        let ts = now_ms() as i64;

        // Insert duplicate entries
        for _ in 0..3 {
            db.exec(move |conn| {
                conn.execute(
                    "INSERT INTO memory (content, source, created_at, updated_at)
                     VALUES ('duplicate content', 'test', ?1, ?1)",
                    rusqlite::params![ts],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        }

        // Insert a unique entry
        db.exec(move |conn| {
            conn.execute(
                "INSERT INTO memory (content, source, created_at, updated_at)
                 VALUES ('unique content', 'test', ?1, ?1)",
                rusqlite::params![ts],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let deduped = deduplicate_memories(&db).await.unwrap();
        assert_eq!(deduped, 2); // 3 duplicates → 1 kept, 2 removed

        let count = db
            .exec(|conn| {
                let c: i64 = conn.query_row("SELECT COUNT(*) FROM memory", [], |r| r.get(0))?;
                Ok(c)
            })
            .await
            .unwrap();
        assert_eq!(count, 2); // 1 unique + 1 kept duplicate
    }

    #[tokio::test]
    async fn test_run_maintenance_no_work() {
        let db = Db::open_memory().unwrap();
        let agent = test_agent_config();
        let summary = run_maintenance(&db, &agent).await.unwrap();
        assert_eq!(summary, "no maintenance needed");
    }

    #[tokio::test]
    async fn test_extract_conversation_text() {
        use yoagent::types::{Content, Message, StopReason, Usage};

        let messages = vec![
            AgentMessage::Llm(Message::user("Hello, how are you?")),
            AgentMessage::Llm(Message::Assistant {
                content: vec![Content::Text {
                    text: "I'm doing well!".into(),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 123,
                error_message: None,
            }),
        ];

        let text = extract_conversation_text(&messages, 1000);
        assert!(text.contains("User: Hello, how are you?"));
        assert!(text.contains("Assistant: I'm doing well!"));
    }

    #[tokio::test]
    async fn test_extract_conversation_text_truncation() {
        let messages = vec![AgentMessage::Llm(Message::user(
            "A very long message that should be truncated",
        ))];

        let text = extract_conversation_text(&messages, 20);
        assert!(text.len() <= 60); // slightly over 20 due to "User: " prefix on first line
    }
}
