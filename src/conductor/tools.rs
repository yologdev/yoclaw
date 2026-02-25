use crate::db::Db;
use yoagent::types::*;

/// Tool for searching the agent's long-term memory via FTS5 (with temporal decay).
pub struct MemorySearchTool {
    db: Db,
}

impl MemorySearchTool {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AgentTool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn label(&self) -> &str {
        "Search Memory"
    }

    fn description(&self) -> &str {
        "Search the agent's long-term memory. Results are ranked by relevance with temporal decay \
         (task memories fade faster than preferences/decisions). Returns category and importance metadata."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query for finding relevant memories"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let query = params["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'query' parameter".into()))?;
        let limit = params["limit"].as_u64().unwrap_or(10) as usize;

        let results = self
            .db
            .memory_search(query, limit)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let text = if results.is_empty() {
            format!("No memories found for '{}'.", query)
        } else {
            results
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let tags = m.tags.as_deref().unwrap_or("");
                    let key = m
                        .key
                        .as_ref()
                        .map(|k| format!(" (key: {})", k))
                        .unwrap_or_default();
                    format!(
                        "{}. [{}|{}|imp:{}]{} {}",
                        i + 1,
                        m.category,
                        tags,
                        m.importance,
                        key,
                        m.content
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({ "count": results.len() }),
        })
    }
}

/// Tool for storing information in the agent's long-term memory.
pub struct MemoryStoreTool {
    db: Db,
}

impl MemoryStoreTool {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AgentTool for MemoryStoreTool {
    fn name(&self) -> &str {
        "memory_store"
    }

    fn label(&self) -> &str {
        "Store Memory"
    }

    fn description(&self) -> &str {
        "Save information to long-term memory with optional category and importance. Categories: \
         fact, preference, decision, event, task, reflection. Importance: 1-10 (higher = more important, \
         less likely to be pruned). Decisions never decay; tasks decay in ~7 days; preferences persist ~90 days."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The information to remember"
                },
                "key": {
                    "type": "string",
                    "description": "Optional unique key for direct lookup and upsert (e.g. 'user_name', 'preferred_language')"
                },
                "tags": {
                    "type": "string",
                    "description": "Optional comma-separated tags for categorization (e.g. 'preference,user')"
                },
                "category": {
                    "type": "string",
                    "description": "Memory category: fact, preference, decision, event, task, reflection (default: fact)",
                    "enum": ["fact", "preference", "decision", "event", "task", "reflection"]
                },
                "importance": {
                    "type": "integer",
                    "description": "Importance score 1-10 (default: 5). Higher = more important, less likely to be pruned."
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let content = params["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'content' parameter".into()))?;
        let key = params["key"].as_str();
        let tags = params["tags"].as_str();
        let category = params["category"].as_str().unwrap_or("fact");
        let importance = params["importance"].as_i64().unwrap_or(5) as i32;

        self.db
            .memory_store_with_meta(key, content, tags, Some("agent"), category, importance)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;

        let msg = match key {
            Some(k) => format!(
                "Stored {} memory (importance: {}) with key '{}'.",
                category, importance, k
            ),
            None => format!("Stored {} memory (importance: {}).", category, importance),
        };

        Ok(ToolResult {
            content: vec![Content::Text { text: msg }],
            details: serde_json::json!({}),
        })
    }
}

/// Tool that lets the agent send a message to the user mid-task via progress events.
/// The message is delivered immediately through the channel adapter, NOT stored in tape.
pub struct SendMessageTool;

#[async_trait::async_trait]
impl AgentTool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn label(&self) -> &str {
        "Send Message"
    }

    fn description(&self) -> &str {
        "Send a message to the user immediately without waiting for the full response. \
         Use this to provide progress updates, ask follow-up questions during long tasks, \
         or deliver partial results. The message is delivered in real-time."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to send to the user immediately"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let message = params["message"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("Missing 'message' parameter".into()))?;

        // Emit via progress callback — this will be routed to the channel adapter
        if let Some(ref on_progress) = ctx.on_progress {
            on_progress(message.to_string());
        }

        Ok(ToolResult {
            content: vec![Content::Text {
                text: "Message sent.".to_string(),
            }],
            details: serde_json::json!({}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn test_ctx() -> ToolContext {
        ToolContext {
            tool_call_id: "test".to_string(),
            tool_name: "test".to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: None,
        }
    }

    #[tokio::test]
    async fn test_memory_store_and_search() {
        let db = Db::open_memory().unwrap();
        let store = MemoryStoreTool::new(db.clone());
        let search = MemorySearchTool::new(db);

        // Store
        let result = store
            .execute(
                serde_json::json!({"content": "User prefers dark mode", "key": "theme", "tags": "preference"}),
                test_ctx(),
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("Stored"));

        // Search
        let result = search
            .execute(serde_json::json!({"query": "dark mode"}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("dark mode"));
    }

    #[tokio::test]
    async fn test_send_message_tool_with_progress() {
        let tool = SendMessageTool;
        let progress_msgs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let msgs_clone = progress_msgs.clone();

        let ctx = ToolContext {
            tool_call_id: "tc-1".to_string(),
            tool_name: "send_message".to_string(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: Some(std::sync::Arc::new(move |text: String| {
                msgs_clone.lock().unwrap().push(text);
            })),
        };

        let result = tool
            .execute(serde_json::json!({"message": "Processing step 1..."}), ctx)
            .await
            .unwrap();

        assert!(content_text(&result.content[0]).contains("Message sent"));
        let captured = progress_msgs.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], "Processing step 1...");
    }

    #[tokio::test]
    async fn test_send_message_tool_without_progress() {
        let tool = SendMessageTool;
        // No on_progress callback — should still succeed without error
        let result = tool
            .execute(serde_json::json!({"message": "Hello"}), test_ctx())
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("Message sent"));
    }

    #[tokio::test]
    async fn test_send_message_tool_missing_param() {
        let tool = SendMessageTool;
        let result = tool.execute(serde_json::json!({}), test_ctx()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_memory_store_with_category() {
        let db = Db::open_memory().unwrap();
        let store = MemoryStoreTool::new(db.clone());

        let result = store
            .execute(
                serde_json::json!({
                    "content": "Always use bun instead of npm",
                    "category": "decision",
                    "importance": 9
                }),
                test_ctx(),
            )
            .await
            .unwrap();
        assert!(content_text(&result.content[0]).contains("decision"));
        assert!(content_text(&result.content[0]).contains("9"));
    }
}

/// Helper: extract text from Content (test-only).
#[cfg(test)]
fn content_text(c: &Content) -> &str {
    match c {
        Content::Text { text } => text,
        _ => "",
    }
}
